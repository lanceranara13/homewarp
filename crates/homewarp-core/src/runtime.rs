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
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use anyhow::{Context, ensure};
use homewarp_runtime::{
    Engine, InstallScript, Network, Port, Protocol, Server as Spec, ServerDir, strip_ansi,
};
use homewarp_template::{Parser, Stop, Template, properties, substitute};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::mpsc;
use utoipa::ToSchema;

use crate::servers;

/// The bridge servers sit on. Its subnet is outside what Docker hands out by itself.
const NETWORK: &str = "homewarp-br";
const SUBNET: &str = "10.213.80.0/24";
/// The user servers run as: deliberately not one that exists on the host.
const USER: u32 = 4857;
/// How much of a console is kept for a page that opens later.
const KEPT_LINES: usize = 500;

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

/// A server as the database has it, with its template read.
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
            other => {
                // Pterodactyl's way of naming a variable, Pelican's, and the plain one.
                let variable = other
                    .strip_prefix("server.build.env.")
                    .or_else(|| other.strip_prefix("server.environment."))
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
}

#[derive(Clone)]
struct Watch(Arc<Mutex<Seen>>);

impl Watch {
    fn new(state: State) -> Self {
        Self(Arc::new(Mutex::new(Seen {
            state,
            lines: VecDeque::new(),
        })))
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn state(&self) -> State {
        self.seen().state
    }

    fn set(&self, state: State) {
        self.seen().state = state;
    }

    /// Adds a line to the console, dropping the oldest once it is full.
    fn say(&self, line: impl Into<String>) {
        let mut seen = self.seen();
        if seen.lines.len() == KEPT_LINES {
            seen.lines.pop_front();
        }
        seen.lines.push_back(line.into());
    }
}

/// The two ends other code holds of a server's task.
struct Live {
    watch: Watch,
    power: mpsc::Sender<Power>,
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
        live.power.try_send(power).map_err(|_| Some(now))?;
        // Changed here and not when the task gets to it, so that a second click
        // finds the first one already counted.
        live.watch.set(next);
        Ok(())
    }

    /// Forgets a server that is not running and deletes what it left: its
    /// containers and its files. False if it is running, and so was left alone.
    pub(crate) async fn remove(&self, id: i64, uuid: &str) -> anyhow::Result<bool> {
        {
            let mut servers = self.servers();
            let busy = servers.get(&id).is_some_and(|live| {
                !matches!(
                    live.watch.state(),
                    State::Offline | State::Crashed | State::InstallFailed
                )
            });
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
        let (power, asked) = mpsc::channel(8);
        self.servers().insert(
            server.id,
            Live {
                watch: watch.clone(),
                power,
            },
        );
        tokio::spawn(Arc::clone(self).run(server, watch, asked, fresh));
    }

    /// A server's task. It ends when the server is removed.
    async fn run(
        self: Arc<Self>,
        mut server: Definition,
        watch: Watch,
        mut asked: mpsc::Receiver<Power>,
        fresh: bool,
    ) {
        let spec = self.spec(&server);
        if fresh {
            if self.install(&mut server, &spec, &watch).await {
                self.serve(&server, &spec, &watch, &mut asked, false).await;
            }
        } else if !server.installed {
            watch.say("Homewarp stopped while this server was being installed.");
        } else {
            match self.engine.is_running(&spec).await {
                Ok(true) => self.serve(&server, &spec, &watch, &mut asked, true).await,
                Ok(false) => {}
                Err(error) => {
                    watch.say(format!("Homewarp could not ask Docker about it: {error}"));
                }
            }
        }
        while let Some(power) = asked.recv().await {
            match power {
                Power::Install if !server.installed => {
                    if self.install(&mut server, &spec, &watch).await {
                        self.serve(&server, &spec, &watch, &mut asked, false).await;
                    }
                }
                Power::Start if server.installed => {
                    self.serve(&server, &spec, &watch, &mut asked, false).await;
                }
                // Asked of a server that had ended by the time this was read.
                _ => {}
            }
        }
    }

    /// Runs the template's install script. True if the server is installed now.
    async fn install(&self, server: &mut Definition, spec: &Spec, watch: &Watch) -> bool {
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
                server.installed = true;
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
    /// until it has ended.
    async fn serve(
        &self,
        server: &Definition,
        spec: &Spec,
        watch: &Watch,
        asked: &mut mpsc::Receiver<Power>,
        found_running: bool,
    ) {
        let ended = async {
            let mut console = if found_running {
                watch.set(State::Running);
                watch.say("Homewarp started again while this server ran. What it printed before that is not here.");
                self.engine.attach(spec).await?
            } else {
                watch.set(State::Starting);
                self.fetch(&spec.image, watch).await?;
                self.prepare(server, spec)?;
                self.engine.create(spec).await?;
                let console = self.engine.attach(spec).await?;
                self.engine.start(spec).await?;
                console
            };
            let mut told_to_stop = false;
            loop {
                tokio::select! {
                    line = console.next_line() => {
                        let Some(line) = line? else { break };
                        let line = shown(&line);
                        let done = &server.template.done;
                        if watch.state() == State::Starting && done.iter().any(|done| line.contains(done.as_str())) {
                            watch.set(State::Running);
                        }
                        watch.say(line);
                    }
                    power = asked.recv() => match power {
                        Some(Power::Stop) if !told_to_stop => {
                            told_to_stop = true;
                            watch.set(State::Stopping);
                            match &server.template.stop {
                                Stop::Command(command) => console.send(command).await?,
                                Stop::Signal(signal) => self.engine.signal(spec, signal).await?,
                            }
                        }
                        Some(Power::Kill) => {
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
            Ok((code, told_to_stop)) if told_to_stop || code == 0 => watch.set(State::Offline),
            Ok((code, _)) => {
                watch.say(format!(
                    "The server stopped by itself, with exit code {code}."
                ));
                watch.set(State::Crashed);
            }
            Err(error) => {
                watch.say(format!("Homewarp could not run this server: {error:#}"));
                // Whatever was made of it must not be left running unwatched.
                let _ = self.engine.remove(spec).await;
                watch.set(State::Crashed);
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
    fn prepare(&self, server: &Definition, spec: &Spec) -> anyhow::Result<()> {
        let files = ServerDir::open(&spec.dir, USER, USER)?;
        for file in &server.template.config_files {
            ensure!(
                file.parser == Parser::Properties,
                "{} is set up by a parser Homewarp does not have yet",
                file.path
            );
            let pairs = file
                .find
                .iter()
                .map(|(key, value)| {
                    let value = substitute(value, |name| server.lookup(name));
                    ensure!(
                        !value.contains("{{"),
                        "{}: nothing to put in place of {value}",
                        file.path
                    );
                    Ok((key.clone(), value))
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let before = files.read_to_string(&file.path)?.unwrap_or_default();
            files.write(&file.path, &properties::patch(&before, &pairs))?;
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
