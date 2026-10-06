//! Runs servers (PLAN.md §5.6). Each server has a task of its own, which
//! installs it, starts it, reads its console and stops it. The rest of Core
//! asks things of that task and reads what it has seen.
//!
//! The owner's condition for the Docker socket (PLAN.md §13) is kept here and
//! in `homewarp-runtime`: the only containers touched are those named after a
//! server of this Homewarp, and the only network is its own.

use std::{
    collections::{HashMap, VecDeque},
    io::ErrorKind,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    pin::pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use futures_util::StreamExt;
use homewarp_runtime::{
    Engine, InstallScript, Network, Port, Protocol, Server as Spec, ServerDir, strip_ansi,
};
use homewarp_template::{Replacement, Stop, Template, config, substitute};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, mpsc};
use utoipa::ToSchema;

use crate::servers;

/// The bridge servers sit on. Its subnet is outside what Docker hands out by itself.
const NETWORK: &str = "homewarp-br";
const SUBNET: &str = "10.213.80.0/24";
/// The home machine's own address on that bridge.
const GATEWAY: &str = "10.213.80.1";
/// The user servers run as: deliberately not one that exists on the host.
const USER: u32 = 4857;
/// How much of a console is kept for a page that opens later.
const KEPT_LINES: usize = 500;
/// How far a page may fall behind before it is started again from where things stand.
const BACKLOG: usize = 1024;
/// How many times a crashed server is started again before it is left alone.
const RESTARTS: u32 = 3;
/// How long the first of those waits. Each one after waits three times as long.
const FIRST_WAIT: Duration = Duration::from_secs(5);
/// A server that stayed up this long was not in a round of crashes.
const STEADY: Duration = Duration::from_secs(60);

/// What a server is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Installing,
    /// The install script failed, or Homewarp stopped while it ran.
    InstallFailed,
    Offline,
    Starting,
    Running,
    Stopping,
    /// It ended without being asked to, and not cleanly.
    Crashed,
}

impl State {
    /// Whether the server has no process, and none on its way.
    pub(crate) fn is_idle(self) -> bool {
        matches!(self, Self::Offline | Self::Crashed | Self::InstallFailed)
    }

    /// For the sentence "This server is ...".
    pub(crate) fn in_words(self) -> &'static str {
        match self {
            Self::Installing => "being installed",
            Self::InstallFailed => "not installed",
            Self::Offline => "offline",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
            Self::Crashed => "stopped after a crash",
        }
    }
}

/// What a person can ask of a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Power {
    Start,
    Stop,
    /// Ends it at once, without the chance to save that `stop` gives.
    Kill,
    /// Runs the install script again, after it failed.
    Install,
}

/// How much of the machine a running server is using.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, ToSchema)]
pub(crate) struct Usage {
    /// Of one core: 150 is a core and a half.
    cpu_percent: f32,
    memory_bytes: u64,
    /// The most it may use before it is stopped.
    memory_limit_bytes: u64,
}

/// What a page that follows a server is sent, over its socket.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Event {
    /// Where things stand. It comes first, and again for a page that has
    /// fallen too far behind to be caught up line by line.
    Snapshot {
        state: State,
        /// The number of the first of `lines`.
        first: u64,
        lines: Vec<String>,
        usage: Option<Usage>,
    },
    /// One more line of the console.
    Line {
        number: u64,
        text: String,
    },
    State {
        state: State,
    },
    /// About once a second while the server runs.
    Usage {
        usage: Usage,
    },
}

/// A server as the database has it, with its template read.
#[derive(Clone)]
pub(crate) struct Definition {
    pub(crate) id: i64,
    pub(crate) uuid: String,
    pub(crate) template: Template,
    pub(crate) image: String,
    pub(crate) memory_mb: u32,
    pub(crate) cpu_percent: u32,
    pub(crate) port: u16,
    pub(crate) variables: Vec<(String, String)>,
    pub(crate) eula: bool,
    pub(crate) installed: bool,
}

impl Definition {
    /// What a `{{...}}` in the startup command or in a config file stands for.
    fn lookup(&self, name: &str) -> Option<String> {
        match name {
            "SERVER_MEMORY" | "server.build.memory" => Some(self.memory_mb.to_string()),
            "SERVER_IP" | "server.build.default.ip" | "server.allocations.default.ip" => {
                Some("0.0.0.0".to_owned())
            }
            "SERVER_PORT" | "server.build.default.port" | "server.allocations.default.port" => {
                Some(self.port.to_string())
            }
            // Where a server finds the home machine, and through it the ports
            // that other servers publish there.
            "config.docker.interface" | "config.docker.network.interface" => {
                Some(GATEWAY.to_owned())
            }
            other => {
                // Pterodactyl's way of naming a variable, Pelican's, and the plain one.
                let variable = other
                    .strip_prefix("server.build.env.")
                    .or_else(|| other.strip_prefix("server.environment."))
                    .or_else(|| other.strip_prefix("env."))
                    .unwrap_or(other);
                self.variables
                    .iter()
                    .find(|(env, _)| env == variable)
                    .map(|(_, value)| value.clone())
            }
        }
    }
}

/// A console line as a terminal would leave it: without escape codes, and only
/// what follows the last carriage return, which is how a progress meter redraws.
fn shown(line: &str) -> String {
    strip_ansi(line.rsplit('\r').next().unwrap_or(line))
}

/// What a server's task has seen, for the pages that ask.
struct Seen {
    state: State,
    lines: VecDeque<String>,
    /// How many lines there have been, those no longer kept among them.
    count: u64,
    usage: Option<Usage>,
}

/// What a server's task has seen, and the way it tells the pages that follow
/// the server. It tells them while it still holds the lock, so that a page is
/// told things in the order they happened and misses nothing after it joins.
#[derive(Clone)]
struct Watch {
    seen: Arc<Mutex<Seen>>,
    events: broadcast::Sender<Event>,
}

impl Watch {
    fn new(state: State) -> Self {
        let seen = Seen {
            state,
            lines: VecDeque::new(),
            count: 0,
            usage: None,
        };
        Self {
            seen: Arc::new(Mutex::new(seen)),
            events: broadcast::channel(BACKLOG).0,
        }
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> State {
        self.seen().state
    }

    fn set(&self, state: State) {
        let mut seen = self.seen();
        seen.state = state;
        // What is not running uses nothing.
        if !matches!(state, State::Starting | State::Running | State::Stopping) {
            seen.usage = None;
        }
        // An error here says that no page is following, which is the usual case.
        let _ = self.events.send(Event::State { state });
    }

    /// Adds a line to the console, dropping the oldest once it is full.
    fn say(&self, line: impl Into<String>) {
        let text = line.into();
        let mut seen = self.seen();
        if seen.lines.len() == KEPT_LINES {
            seen.lines.pop_front();
        }
        seen.lines.push_back(text.clone());
        let number = seen.count;
        seen.count += 1;
        let _ = self.events.send(Event::Line { number, text });
    }

    fn measure(&self, usage: Usage) {
        self.seen().usage = Some(usage);
        let _ = self.events.send(Event::Usage { usage });
    }

    /// Where things stand, and everything from here on, with nothing between.
    fn follow(&self) -> (Event, broadcast::Receiver<Event>) {
        let seen = self.seen();
        let snapshot = Event::Snapshot {
            state: seen.state,
            first: seen.count - seen.lines.len() as u64,
            lines: seen.lines.iter().cloned().collect(),
            usage: seen.usage,
        };
        (snapshot, self.events.subscribe())
    }
}

/// What reaches a server's task.
enum Asked {
    Power(Power),
    /// A line typed into its console.
    Typed(String),
}

/// The two ends other code holds of a server's task.
struct Live {
    watch: Watch,
    asked: mpsc::Sender<Asked>,
    /// What the server is made of. Its task reads this afresh each time it
    /// starts the server, so a change made while it is down counts from then.
    made_of: Arc<Mutex<Definition>>,
}

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

fn current(made_of: &Mutex<Definition>) -> Definition {
    lock(made_of).clone()
}

/// Every server's task, and the Docker daemon they share.
pub struct Runtime {
    engine: Engine,
    data: PathBuf,
    db: SqlitePool,
    servers: Mutex<HashMap<i64, Live>>,
}

impl Runtime {
    /// Reaches Docker and takes up every server in the database: one found
    /// running stays running and is watched again.
    ///
    /// Docker reads the paths it is given on the host, so `data` has to be the
    /// same path there as it is here.
    pub async fn start(data: &Path, db: SqlitePool) -> anyhow::Result<Arc<Self>> {
        let engine = Engine::connect()?;
        engine
            .ensure_network(&Network {
                name: NETWORK.to_owned(),
                subnet: SUBNET.to_owned(),
            })
            .await
            .context("reaching Docker")?;
        let runtime = Arc::new(Self {
            engine,
            data: std::fs::canonicalize(data)?,
            db,
            servers: Mutex::default(),
        });
        for server in servers::definitions(&runtime.db).await? {
            runtime.watch_over(server, false);
        }
        Ok(runtime)
    }

    /// Takes on a server that has just been made: installs it, then starts it.
    pub(crate) fn add(self: &Arc<Self>, server: Definition) {
        self.watch_over(server, true);
    }

    pub(crate) fn state(&self, id: i64) -> Option<State> {
        Some(self.servers().get(&id)?.watch.state())
    }

    /// A server's state and what is kept of its console.
    pub(crate) fn seen(&self, id: i64) -> Option<(State, Vec<String>)> {
        let servers = self.servers();
        let seen = servers.get(&id)?.watch.seen();
        Some((seen.state, seen.lines.iter().cloned().collect()))
    }

    /// Passes a request on to a server's task. `Err(None)` if there is no such
    /// server; `Err(Some(state))` if the state it is in rules the request out.
    pub(crate) fn ask(&self, id: i64, power: Power) -> Result<(), Option<State>> {
        let servers = self.servers();
        let live = servers.get(&id).ok_or(None)?;
        let now = live.watch.state();
        let next = match (power, now) {
            (Power::Start, State::Offline | State::Crashed) => State::Starting,
            (Power::Stop, State::Starting | State::Running) => State::Stopping,
            (Power::Kill, State::Starting | State::Running | State::Stopping) => State::Stopping,
            (Power::Install, State::InstallFailed) => State::Installing,
            _ => return Err(Some(now)),
        };
        live.asked
            .try_send(Asked::Power(power))
            .map_err(|_| Some(now))?;
        // Changed here and not when the task gets to it, so that a second click
        // finds the first one already counted.
        live.watch.set(next);
        Ok(())
    }

    /// Types a line into a server's console. The errors are those of [`Runtime::ask`].
    pub(crate) fn type_in(&self, id: i64, line: String) -> Result<(), Option<State>> {
        let servers = self.servers();
        let live = servers.get(&id).ok_or(None)?;
        let now = live.watch.state();
        if !matches!(now, State::Starting | State::Running) {
            return Err(Some(now));
        }
        live.asked
            .try_send(Asked::Typed(line))
            .map_err(|_| Some(now))
    }

    /// Changes what a server that is not running is made of. The errors are
    /// those of [`Runtime::ask`].
    pub(crate) fn change(&self, id: i64, mut new: Definition) -> Result<(), Option<State>> {
        let servers = self.servers();
        let live = servers.get(&id).ok_or(None)?;
        let now = live.watch.state();
        if !now.is_idle() {
            return Err(Some(now));
        }
        let mut made_of = lock(&live.made_of);
        // Whether it is installed is the task's to say, not the form's.
        new.installed = made_of.installed;
        *made_of = new;
        Ok(())
    }

    /// Where a server stands, and everything that happens to it from here on.
    pub(crate) fn follow(&self, id: i64) -> Option<(Event, broadcast::Receiver<Event>)> {
        Some(self.servers().get(&id)?.watch.follow())
    }

    /// Forgets a server that is not running and deletes what it left: its
    /// containers and its files. False if it is running, and so was left alone.
    pub(crate) async fn remove(&self, id: i64, uuid: &str) -> anyhow::Result<bool> {
        {
            let mut servers = self.servers();
            let busy = servers
                .get(&id)
                .is_some_and(|live| !live.watch.state().is_idle());
            if busy {
                return Ok(false);
            }
            // Dropping its end of the channel is what ends the server's task.
            servers.remove(&id);
        }
        self.engine.forget(uuid).await?;
        for kept in ["servers", "install"] {
            match tokio::fs::remove_dir_all(self.data.join(kept).join(uuid)).await {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    return Err(anyhow::Error::new(error).context("deleting the server's files"));
                }
                _ => {}
            }
        }
        Ok(true)
    }

    fn servers(&self) -> MutexGuard<'_, HashMap<i64, Live>> {
        self.servers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn watch_over(self: &Arc<Self>, server: Definition, fresh: bool) {
        let watch = Watch::new(match (fresh, server.installed) {
            (true, _) => State::Installing,
            (false, false) => State::InstallFailed,
            (false, true) => State::Offline,
        });
        let (asked, inbox) = mpsc::channel(16);
        let id = server.id;
        let made_of = Arc::new(Mutex::new(server));
        self.servers().insert(
            id,
            Live {
                watch: watch.clone(),
                asked,
                made_of: Arc::clone(&made_of),
            },
        );
        tokio::spawn(Arc::clone(self).run(made_of, watch, inbox, fresh));
    }

    /// A server's task. It ends when the server is removed.
    async fn run(
        self: Arc<Self>,
        made_of: Arc<Mutex<Definition>>,
        watch: Watch,
        mut inbox: mpsc::Receiver<Asked>,
        fresh: bool,
    ) {
        let server = current(&made_of);
        let spec = self.spec(&server);
        if fresh {
            if self.install(&made_of, &watch).await {
                self.keep_serving(&made_of, &watch, &mut inbox, false).await;
            }
        } else if !server.installed {
            watch.say("Homewarp stopped while this server was being installed.");
        } else {
            match self.engine.is_running(&spec).await {
                Ok(true) => self.keep_serving(&made_of, &watch, &mut inbox, true).await,
                Ok(false) => {}
                Err(error) => {
                    watch.say(format!("Homewarp could not ask Docker about it: {error}"));
                }
            }
        }
        while let Some(asked) = inbox.recv().await {
            match asked {
                Asked::Power(Power::Install) if !current(&made_of).installed => {
                    if self.install(&made_of, &watch).await {
                        self.keep_serving(&made_of, &watch, &mut inbox, false).await;
                    }
                }
                Asked::Power(Power::Start) if current(&made_of).installed => {
                    self.keep_serving(&made_of, &watch, &mut inbox, false).await;
                }
                // Asked of a server that had ended by the time this was read.
                _ => {}
            }
        }
    }

    /// Runs the template's install script. True if the server is installed now.
    async fn install(&self, made_of: &Mutex<Definition>, watch: &Watch) -> bool {
        let server = current(made_of);
        let spec = &self.spec(&server);
        watch.set(State::Installing);
        let installed = async {
            ServerDir::open(&spec.dir, USER, USER)?;
            let has_chown = match &server.template.install {
                Some(install) => {
                    self.fetch(&install.image, watch).await?;
                    let scratch = self.data.join("install").join(&server.uuid);
                    let script = InstallScript {
                        image: &install.image,
                        entrypoint: &install.entrypoint,
                        script: &install.script,
                        scratch: &scratch,
                    };
                    let code = self
                        .engine
                        .install(spec, &script, |line| watch.say(shown(line)))
                        .await?;
                    ensure!(code == 0, "the install script ended with exit code {code}");
                    &install.image
                }
                None => {
                    self.fetch(&spec.image, watch).await?;
                    &spec.image
                }
            };
            // The script ran as root, and so did what made the directory.
            self.engine.own_files(spec, has_chown).await?;
            sqlx::query("UPDATE servers SET installed = 1 WHERE id = ?")
                .bind(server.id)
                .execute(&self.db)
                .await?;
            anyhow::Ok(())
        }
        .await;
        match installed {
            Ok(()) => {
                lock(made_of).installed = true;
                watch.say("Installed.");
                watch.set(State::Offline);
                true
            }
            Err(error) => {
                watch.say(format!("Installing failed: {error:#}"));
                watch.set(State::InstallFailed);
                false
            }
        }
    }

    /// Starts the server, or takes over one found running, and stays with it
    /// until it has ended. True if it ended in a crash.
    async fn serve(
        &self,
        server: &Definition,
        spec: &Spec,
        watch: &Watch,
        inbox: &mut mpsc::Receiver<Asked>,
        found_running: bool,
    ) -> bool {
        let ended = async {
            let mut console = if found_running {
                watch.set(State::Running);
                watch.say("Homewarp started again while this server ran. What it printed before that is not here.");
                self.engine.attach(spec).await?
            } else {
                watch.set(State::Starting);
                self.fetch(&spec.image, watch).await?;
                self.prepare(server, spec, watch)?;
                self.engine.create(spec).await?;
                let console = self.engine.attach(spec).await?;
                self.engine.start(spec).await?;
                console
            };
            // Fused, because it ends before the server's last lines have been read.
            let mut usage = pin!(self.engine.usage(spec).fuse());
            let mut told_to_stop = false;
            loop {
                tokio::select! {
                    Some(now) = usage.next() => watch.measure(Usage {
                        cpu_percent: now.cpu_percent,
                        memory_bytes: now.memory_bytes,
                        memory_limit_bytes: now.memory_limit_bytes,
                    }),
                    line = console.next_line() => {
                        let Some(line) = line? else { break };
                        let line = shown(&line);
                        let done = &server.template.done;
                        if watch.state() == State::Starting && done.iter().any(|done| line.contains(done.as_str())) {
                            watch.set(State::Running);
                        }
                        watch.say(line);
                    }
                    asked = inbox.recv() => match asked {
                        Some(Asked::Typed(line)) => console.send(&line).await?,
                        Some(Asked::Power(Power::Stop)) if !told_to_stop => {
                            told_to_stop = true;
                            watch.set(State::Stopping);
                            match &server.template.stop {
                                Stop::Command(command) => console.send(command).await?,
                                Stop::Signal(signal) => self.engine.signal(spec, signal).await?,
                            }
                        }
                        Some(Asked::Power(Power::Kill)) => {
                            told_to_stop = true;
                            watch.set(State::Stopping);
                            self.engine.signal(spec, "SIGKILL").await?;
                        }
                        Some(_) => {}
                        // The server is being removed. That is refused while it
                        // runs, so this is not reached; if it ever is, end it.
                        None => {
                            told_to_stop = true;
                            self.engine.signal(spec, "SIGKILL").await?;
                            break;
                        }
                    },
                }
            }
            let code = self.engine.wait(spec).await?;
            self.engine.remove(spec).await?;
            anyhow::Ok((code, told_to_stop))
        }
        .await;
        match ended {
            Ok((code, told_to_stop)) if told_to_stop || code == 0 => {
                watch.set(State::Offline);
                false
            }
            Ok((code, _)) => {
                watch.say(format!(
                    "The server stopped by itself, with exit code {code}."
                ));
                watch.set(State::Crashed);
                true
            }
            // Not a crash of the server's: the same would happen again at once.
            Err(error) => {
                watch.say(format!("Homewarp could not run this server: {error:#}"));
                // Whatever was made of it must not be left running unwatched.
                let _ = self.engine.remove(spec).await;
                watch.set(State::Crashed);
                false
            }
        }
    }

    /// Runs the server and, when it crashes, runs it again: a little later
    /// each time, and not for ever. A server that had stayed up for a while
    /// before it crashed starts the count afresh.
    async fn keep_serving(
        &self,
        made_of: &Mutex<Definition>,
        watch: &Watch,
        inbox: &mut mpsc::Receiver<Asked>,
        mut found_running: bool,
    ) {
        let mut crashes = 0;
        loop {
            // Read afresh: it may have been changed while the server was down.
            let server = current(made_of);
            let spec = self.spec(&server);
            let began = Instant::now();
            if !self
                .serve(&server, &spec, watch, inbox, found_running)
                .await
            {
                return;
            }
            found_running = false;
            if began.elapsed() >= STEADY {
                crashes = 0;
            }
            crashes += 1;
            if crashes > RESTARTS {
                watch.say(format!(
                    "That is {crashes} crashes one after another. Homewarp will not start it again by itself."
                ));
                return;
            }
            let wait = FIRST_WAIT * 3u32.pow(crashes - 1);
            watch.say(format!(
                "Homewarp will start it again in {} seconds.",
                wait.as_secs()
            ));
            let mut waited = pin!(tokio::time::sleep(wait));
            loop {
                tokio::select! {
                    () = &mut waited => break,
                    asked = inbox.recv() => match asked {
                        // Started by hand in the meantime: the same, sooner.
                        Some(Asked::Power(Power::Start)) => break,
                        Some(_) => {}
                        // Removed while it waited.
                        None => return,
                    },
                }
            }
        }
    }

    /// Pulls an image the daemon lacks, and says so first: it can take minutes.
    async fn fetch(&self, image: &str, watch: &Watch) -> anyhow::Result<()> {
        if !self.engine.has_image(image).await {
            watch.say(format!("Fetching {image}"));
        }
        Ok(self.engine.pull(image).await?)
    }

    /// Writes what the template wants in the server's files before a start.
    fn prepare(&self, server: &Definition, spec: &Spec, watch: &Watch) -> anyhow::Result<()> {
        let files = ServerDir::open(&spec.dir, USER, USER)?;
        for file in &server.template.config_files {
            let mut replacements = Vec::with_capacity(file.find.len());
            for replacement in &file.find {
                let value = substitute(&replacement.value, |name| server.lookup(name));
                if value.contains("{{") {
                    watch.say(format!(
                        "{}: {} was left as it is, for there is nothing to put in place of {value}.",
                        file.path, replacement.key
                    ));
                    continue;
                }
                replacements.push(Replacement {
                    value,
                    ..replacement.clone()
                });
            }
            let before = files.read_to_string(&file.path)?.unwrap_or_default();
            // A file that cannot be set up is said and passed over, as Wings
            // passes over it: the server may still do without.
            match config::patch(file.parser, &before, &replacements) {
                Ok(after) => files.write(&file.path, &after)?,
                Err(error) => watch.say(format!("{} was left as it is: {error}.", file.path)),
            }
        }
        if server.eula {
            files.write("eula.txt", "eula=true\n")?;
        }
        Ok(())
    }

    /// The server as Docker needs to know it.
    fn spec(&self, server: &Definition) -> Spec {
        let host_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        Spec {
            id: server.uuid.clone(),
            dir: self.data.join("servers").join(&server.uuid),
            image: server.image.clone(),
            startup: substitute(&server.template.startup, |name| server.lookup(name)),
            variables: server.variables.clone(),
            memory_mb: server.memory_mb,
            cpu_percent: server.cpu_percent,
            // An egg does not say which protocol its port speaks, so both.
            ports: [Protocol::Tcp, Protocol::Udp]
                .map(|protocol| Port {
                    host_ip,
                    port: server.port,
                    protocol,
                })
                .to_vec(),
            uid: USER,
            gid: USER,
            network: NETWORK.to_owned(),
            timezone: "UTC".to_owned(),
        }
    }
}
