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
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    pin::pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use anyhow::{Context, ensure};
use futures_util::StreamExt;
use homewarp_runtime::{
    Console, Engine, InstallScript, Listener, Network, Port, Protocol, Server as Spec, ServerDir,
    strip_ansi,
};
use homewarp_template::{Parser, Replacement, Stop, Template, config, substitute};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::{broadcast, mpsc};
use utoipa::ToSchema;

use crate::{
    audit,
    minecraft::{self, Joined, Players},
    mods,
    servers::{self, ExtraPort, PortProtocol},
    settings,
};

/// The bridge servers sit on. Its subnet is outside what Docker hands out by itself.
const NETWORK: &str = "homewarp-br";
const SUBNET: &str = "10.213.80.0/24";
/// The home machine's own address on that bridge.
const GATEWAY: &str = "10.213.80.1";
/// The user servers run as: deliberately not one that exists on the host.
pub(crate) const USER: u32 = 4857;
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
/// How often a running server is asked who is on it.
const ASK_EVERY: Duration = Duration::from_secs(15);
/// How many times a server that has begun to run may fail to say before it is
/// taken for one that does not speak Minecraft, and asked no more.
const UNANSWERED: u32 = 4;

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
    /// Its files are being put back from a backup. It has no process, and is
    /// not started, changed or removed until that is done.
    Restoring,
    /// Stopped by Homewarp because nobody was on it. Something small listens
    /// in its place, and it is started again when a player joins.
    Asleep,
}

impl State {
    /// Whether the server has no process, and none on its way.
    pub(crate) fn is_idle(self) -> bool {
        matches!(
            self,
            Self::Offline | Self::Crashed | Self::InstallFailed | Self::Asleep
        )
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
            Self::Restoring => "having a backup put back",
            Self::Asleep => "asleep",
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
    /// What arrives for it over the network in a second, and what it sends:
    /// players for the most part, and whatever else it talks to.
    received_bytes_per_second: u64,
    sent_bytes_per_second: u64,
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
        players: Option<Players>,
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
    /// Who is on it, where the server says: a few times a minute while it
    /// runs, and nothing once it does not.
    Players {
        players: Option<Players>,
    },
}

/// A server as the database has it, with its template read.
#[derive(Clone)]
pub(crate) struct Definition {
    pub(crate) id: i64,
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) template: Template,
    pub(crate) image: String,
    pub(crate) memory_mb: u32,
    pub(crate) cpu_percent: u32,
    pub(crate) port: u16,
    /// Which protocols that port is open for.
    pub(crate) protocol: PortProtocol,
    /// The further ports the server has.
    pub(crate) ports: Vec<ExtraPort>,
    pub(crate) variables: Vec<(String, String)>,
    pub(crate) eula: bool,
    pub(crate) installed: bool,
    /// How many minutes it may run with nobody on it before it is put to
    /// sleep. 0 is never.
    pub(crate) sleep_minutes: u32,
    /// Whether it was asleep when Homewarp last wrote that down: read when
    /// Homewarp starts, and nowhere after.
    pub(crate) asleep: bool,
    /// Whether players are told that it is offline while it is stopped.
    pub(crate) says_offline: bool,
}

/// How something stands in for a server that has no process.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// For one that is asleep: a player who joins wakes it.
    Wakes,
    /// For one that is stopped: players are told so, and that is all.
    Stays,
}

/// What stands in for a server in this state, if anything does. Only a
/// server of Minecraft's has anything stand in for it: the stand-in speaks
/// that game's language and no other.
fn standing(state: State, server: &Definition) -> Option<Standing> {
    match state {
        State::Asleep => Some(Standing::Wakes),
        State::Offline | State::Crashed
            if server.says_offline
                && server.installed
                && server.protocol != PortProtocol::Udp
                && mods::fits(&server.template) =>
        {
            Some(Standing::Stays)
        }
        _ => None,
    }
}

/// So many minutes, as a sentence says it.
fn minutes(count: u32) -> String {
    match count {
        1 => "a minute".to_owned(),
        count => format!("{count} minutes"),
    }
}

/// The container that listens in place of a server that is asleep.
fn stand_in_name(uuid: &str) -> String {
    format!("homewarp-{uuid}-standin")
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

/// Where a server's files are kept, under the directory Homewarp keeps everything in.
pub(crate) fn files_at(data: &Path, uuid: &str) -> PathBuf {
    data.join("servers").join(uuid)
}

/// The id of the container this process is in, read off the list of what is
/// mounted in it: Docker mounts a container's `hostname` and `hosts` from a
/// directory that is named after it.
fn container_id(mounts: &str) -> Option<&str> {
    mounts.split("/containers/").skip(1).find_map(|rest| {
        let id = rest.get(..64)?;
        (id.bytes().all(|c| c.is_ascii_hexdigit()) && rest[64..].starts_with('/')).then_some(id)
    })
}

/// A console line as a terminal would leave it: without escape codes, and only
/// what follows the last carriage return, which is how a progress meter redraws.
fn shown(line: &str) -> String {
    strip_ansi(line.rsplit('\r').next().unwrap_or(line))
}

/// Writes what a server's template wants in its files: each of its config
/// files with the server's settings put in, and the EULA where it was agreed
/// to. What could not be done is told to `say`, for the server's console.
fn set_up(
    files: &ServerDir,
    server: &Definition,
    mut say: impl FnMut(String),
) -> anyhow::Result<()> {
    for file in &server.template.config_files {
        let mut replacements = Vec::with_capacity(file.find.len());
        for replacement in &file.find {
            let value = substitute(&replacement.value, |name| server.lookup(name));
            if value.contains("{{") {
                say(format!(
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
        let before = match files.read_to_string(&file.path) {
            Ok(before) => before,
            Err(error) => match not_settings(&error) {
                Some(why) => {
                    say(format!("{} was left as it is: {why}.", file.path));
                    continue;
                }
                None => return Err(error.into()),
            },
        };
        // The `file` parser changes lines that are there, and a file that is
        // not there has none. Wings makes it all the same, empty, and a server
        // that writes its settings out only where it finds no file then never
        // does: Velocity came up on a port of its own choosing that way.
        if before.is_none() && file.parser == Parser::File {
            say(format!(
                "{} is not there yet, and is left for the server to make. Its settings are put in from the next start.",
                file.path
            ));
            continue;
        }
        // A file that cannot be set up is said and passed over, as Wings
        // passes over it: the server may still do without.
        match config::patch(file.parser, &before.unwrap_or_default(), &replacements) {
            Ok(after) => write_settings(files, &file.path, &after, &mut say)?,
            Err(error) => say(format!("{} was left as it is: {error}.", file.path)),
        }
    }
    if server.eula {
        write_settings(files, "eula.txt", "eula=true\n", &mut say)?;
    }
    Ok(())
}

/// Why what is at a path cannot be a file of settings, if that is why it
/// could not be read or written. A server's folder is the server's to fill,
/// and what it put where its settings belong is said and passed over, as
/// settings that cannot be made sense of are: it does not hold up a start.
fn not_settings(error: &std::io::Error) -> Option<&'static str> {
    match error.kind() {
        ErrorKind::FileTooLarge => Some("it is longer than a file of settings is"),
        ErrorKind::InvalidData => Some("it is not text"),
        ErrorKind::InvalidInput | ErrorKind::IsADirectory => Some("it is not an ordinary file"),
        _ => None,
    }
}

/// Writes one of a server's files of settings, or says why it was left.
fn write_settings(
    files: &ServerDir,
    path: &str,
    text: &str,
    mut say: impl FnMut(String),
) -> anyhow::Result<()> {
    match files.write(path, text) {
        Ok(()) => Ok(()),
        Err(error) => match not_settings(&error) {
            Some(why) => {
                say(format!("{path} was left as it is: {why}."));
                Ok(())
            }
            None => Err(error.into()),
        },
    }
}

/// What a server's task has seen, for the pages that ask.
struct Seen {
    state: State,
    lines: VecDeque<String>,
    /// How many lines there have been, those no longer kept among them.
    count: u64,
    usage: Option<Usage>,
    players: Option<Players>,
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
            players: None,
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
        // What is not running uses nothing, and has nobody on it.
        if !matches!(state, State::Starting | State::Running | State::Stopping) {
            seen.usage = None;
            seen.players = None;
        }
        // An error here says that no page is following, which is the usual case.
        let _ = self.events.send(Event::State { state });
    }

    /// Starting, if it was asleep. False if it was anything else by now, and
    /// so left as it was.
    fn wake(&self) -> bool {
        let mut seen = self.seen();
        if seen.state != State::Asleep {
            return false;
        }
        seen.state = State::Starting;
        let _ = self.events.send(Event::State {
            state: State::Starting,
        });
        true
    }

    /// Offline, if it was asleep: for a sleep that nothing can stand in for.
    fn give_up_sleep(&self) {
        let mut seen = self.seen();
        if seen.state == State::Asleep {
            seen.state = State::Offline;
            let _ = self.events.send(Event::State {
                state: State::Offline,
            });
        }
    }

    /// Who the server says is on it. Told to the pages only when it changes.
    fn count(&self, players: Option<Players>) {
        let mut seen = self.seen();
        if seen.players != players {
            seen.players.clone_from(&players);
            let _ = self.events.send(Event::Players { players });
        }
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
            players: seen.players.clone(),
        };
        (snapshot, self.events.subscribe())
    }
}

/// A server held out of use while its files are replaced. It is as it was
/// once this is dropped.
pub(crate) struct Hold {
    watch: Watch,
    was: State,
}

impl Hold {
    /// Says in the server's console how the work it is held for is going.
    pub(crate) fn say(&self, line: impl Into<String>) {
        self.watch.say(line);
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.watch.set(self.was);
    }
}

/// What reaches a server's task.
enum Asked {
    Power(Power),
    /// A line typed into its console.
    Typed(String),
    /// What it is made of was changed. That matters to a server that is
    /// asleep, whose port something is listening on in its place.
    Changed,
}

/// How a server's run came to its end.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ended {
    Stopped,
    Crashed,
    /// Stopped by Homewarp, because nobody had been on it for long enough.
    Slept,
}

/// Asks a running server who is on it, every so often, until this is dropped.
struct Asking(tokio::task::JoinHandle<()>);

impl Drop for Asking {
    fn drop(&mut self) {
        self.0.abort();
    }
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
    /// Where servers look names up. Read each time a server is started.
    resolvers: Mutex<Vec<String>>,
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
        let resolvers = settings::resolvers(&db).await?;
        let runtime = Arc::new(Self {
            engine,
            data: std::fs::canonicalize(data)?,
            db,
            servers: Mutex::default(),
            resolvers: Mutex::new(resolvers),
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

    /// Who a running server says is on it. None for one that does not say.
    pub(crate) fn players(&self, id: i64) -> Option<Players> {
        self.servers().get(&id)?.watch.seen().players.clone()
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
            (Power::Start, State::Offline | State::Crashed | State::Asleep) => State::Starting,
            (Power::Stop, State::Starting | State::Running) => State::Stopping,
            // Stopped already. What ends is its sleep: nothing wakes it after this.
            (Power::Stop, State::Asleep) => State::Offline,
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
        // Not delivered to a task that is busy, which reads it afresh anyway.
        let _ = live.asked.try_send(Asked::Changed);
        Ok(())
    }

    /// Takes a server that has no process out of use: it shows as being
    /// restored, and is not started, changed or removed, until what is
    /// returned is dropped. The errors are those of [`Runtime::ask`].
    pub(crate) fn hold(&self, id: i64) -> Result<Hold, Option<State>> {
        let servers = self.servers();
        let live = servers.get(&id).ok_or(None)?;
        let was = live.watch.state();
        if !was.is_idle() {
            return Err(Some(was));
        }
        live.watch.set(State::Restoring);
        Ok(Hold {
            watch: live.watch.clone(),
            was,
        })
    }

    /// Changes where servers look names up. It counts from a server's next start.
    pub(crate) fn set_resolvers(&self, resolvers: Vec<String>) {
        *lock(&self.resolvers) = resolvers;
    }

    /// Where a server stands, and everything that happens to it from here on.
    pub(crate) fn follow(&self, id: i64) -> Option<(Event, broadcast::Receiver<Event>)> {
        Some(self.servers().get(&id)?.watch.follow())
    }

    /// Forgets a server that is not running and deletes what it left: its
    /// containers, its files and its backups. False if it is running, and so
    /// was left alone.
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
        for kept in ["servers", "install", "backups"] {
            match tokio::fs::remove_dir_all(self.data.join(kept).join(uuid)).await {
                Err(error) if error.kind() != ErrorKind::NotFound => {
                    return Err(anyhow::Error::new(error).context("deleting the server's files"));
                }
                _ => {}
            }
        }
        Ok(true)
    }

    /// Runs this very program once in a container that stands where a server
    /// stands, on the servers' bridge with `port` published as a server's
    /// is, to listen there and say where a connection came from
    /// ([`crate::tunnel::probe_listen`]). Each line it prints goes to `on_line`.
    pub(crate) async fn listen_once(
        &self,
        port: u16,
        on_line: impl FnMut(&str),
    ) -> anyhow::Result<()> {
        let image = self.own_image().await?;
        let program = std::env::current_exe().context("finding this program")?;
        let listener = Listener {
            name: "homewarp-probe",
            network: NETWORK,
            image: &image,
            command: vec![
                program.to_string_lossy().into_owned(),
                "probe-listen".to_owned(),
                port.to_string(),
            ],
            port,
            user: USER,
        };
        let code = self.engine.listen(&listener, on_line).await?;
        ensure!(code == 0, "the listener ended with exit code {code}");
        Ok(())
    }

    /// How the container this Core runs in was made: the image it was asked
    /// for by name, and the labels on it. None where Core runs in no container.
    pub(crate) async fn own_making(
        &self,
    ) -> anyhow::Result<Option<(String, HashMap<String, String>)>> {
        let mounts = tokio::fs::read_to_string("/proc/self/mountinfo")
            .await
            .context("reading what is mounted here")?;
        match container_id(&mounts) {
            Some(id) => Ok(self.engine.made_as(id).await?),
            None => Ok(None),
        }
    }

    /// Starts a program beside everything else that goes on by itself, from
    /// an image that is fetched first if it is not here.
    pub(crate) async fn run_apart(
        &self,
        apart: &homewarp_runtime::Apart<'_>,
    ) -> anyhow::Result<()> {
        self.engine
            .pull(apart.image)
            .await
            .with_context(|| format!("fetching the image {}", apart.image))?;
        Ok(self.engine.run_apart(apart).await?)
    }

    /// The image this Core runs from: the one image that is sure to be here,
    /// and to have this program in it.
    async fn own_image(&self) -> anyhow::Result<String> {
        if let Ok(image) = std::env::var("HOMEWARP_IMAGE") {
            return Ok(image);
        }
        let mounts = tokio::fs::read_to_string("/proc/self/mountinfo")
            .await
            .context("reading what is mounted here")?;
        let id = container_id(&mounts).context(
            "Homewarp does not seem to run in a container, and was not told which image it could listen from (HOMEWARP_IMAGE)",
        )?;
        self.engine
            .image_of(id)
            .await?
            .context("Docker does not know the container Homewarp runs in")
    }

    fn servers(&self) -> MutexGuard<'_, HashMap<i64, Live>> {
        self.servers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn watch_over(self: &Arc<Self>, server: Definition, fresh: bool) {
        let watch = Watch::new(match (fresh, server.installed) {
            (true, _) => State::Installing,
            (false, false) => State::InstallFailed,
            (false, true) if server.asleep => State::Asleep,
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
        while let Some(asked) = self.next(&made_of, &watch, &mut inbox).await {
            match asked {
                Asked::Power(Power::Install) if !current(&made_of).installed => {
                    if self.install(&made_of, &watch).await {
                        self.keep_serving(&made_of, &watch, &mut inbox, false).await;
                    }
                }
                Asked::Power(Power::Start) if current(&made_of).installed => {
                    self.keep_serving(&made_of, &watch, &mut inbox, false).await;
                }
                // Asked of one that was asleep, most likely: it is to stay down.
                Asked::Power(Power::Stop) => self.write_down_sleep(server.id, false).await,
                // Asked of a server that had ended by the time this was read.
                _ => {}
            }
        }
    }

    /// What is asked of a server that is not running, when it is asked.
    /// Meanwhile something may stand in for it on its port: for one that is
    /// asleep, where a player who joins is as good as somebody asking for a
    /// start, and for one that is stopped and is to say so.
    async fn next(
        &self,
        made_of: &Mutex<Definition>,
        watch: &Watch,
        inbox: &mut mpsc::Receiver<Asked>,
    ) -> Option<Asked> {
        let mut changes = watch.events.subscribe();
        // The state in which what says "offline" could not be had. It is not
        // tried again until the server is in another.
        let mut given_up: Option<State> = None;
        loop {
            let state = watch.state();
            if given_up != Some(state) {
                given_up = None;
            }
            let server = current(made_of);
            let Some(how) = standing(state, &server).filter(|_| given_up.is_none()) else {
                let asked = tokio::select! {
                    asked = inbox.recv() => Some(asked),
                    _ = changes.recv() => None,
                };
                match asked {
                    // Something changed: it may be asleep again, once what
                    // held it has let go, or be told to say that it is offline.
                    None | Some(Some(Asked::Changed)) => continue,
                    Some(other) => return other,
                }
            };
            tokio::select! {
                ended = self.stand_in(&server, how) => match (how, ended) {
                    (Standing::Wakes, Ok(Some(joined))) => {
                        // Not while a backup is being put back: the player was
                        // told to come again, and by then it may be asleep again.
                        if watch.wake() {
                            watch.say(format!(
                                "{} joined from {}. Waking up.",
                                joined.name, joined.from
                            ));
                            let detail = format!("{} from {}", joined.name, joined.from);
                            audit::record_by_homewarp(&self.db, Some(server.id), "server.wake", &detail)
                                .await;
                            return Some(Asked::Power(Power::Start));
                        }
                    }
                    (Standing::Wakes, Ok(None)) => {
                        watch.say("What listened in its place while it slept has ended. It is offline now.");
                        watch.give_up_sleep();
                        self.write_down_sleep(server.id, false).await;
                    }
                    (Standing::Wakes, Err(error)) => {
                        watch.say(format!(
                            "Homewarp could not listen in its place while it slept, so it is offline now: {error:#}"
                        ));
                        watch.give_up_sleep();
                        self.write_down_sleep(server.id, false).await;
                    }
                    (Standing::Stays, ended) => {
                        let why = match ended {
                            Ok(_) => "what did has ended".to_owned(),
                            Err(error) => format!("{error:#}"),
                        };
                        watch.say(format!("Nothing tells players that it is offline: {why}."));
                        given_up = Some(state);
                    }
                },
                asked = inbox.recv() => {
                    // Its port is wanted back, by the server or by a new stand-in.
                    let _ = self.engine.end_listener(&stand_in_name(&server.uuid)).await;
                    match asked {
                        Some(Asked::Changed) => continue,
                        other => return other,
                    }
                }
            }
        }
    }

    /// Has this very program listen where a server that has no process would
    /// ([`crate::minecraft::stand_in`]), to its end, and says who joined if it
    /// ended because a player did.
    async fn stand_in(&self, server: &Definition, how: Standing) -> anyhow::Result<Option<Joined>> {
        let image = self.own_image().await?;
        let program = std::env::current_exe().context("finding this program")?;
        let name = stand_in_name(&server.uuid);
        let (listed, joining, mode) = match how {
            Standing::Wakes => (
                format!("{} is asleep. Join to wake it up.", server.name),
                format!("{} is waking up. Join again in a minute.", server.name),
                "wake",
            ),
            Standing::Stays => (
                format!("{} is offline.", server.name),
                format!("{} is offline.", server.name),
                "stay",
            ),
        };
        let listener = Listener {
            name: &name,
            network: NETWORK,
            image: &image,
            command: vec![
                program.to_string_lossy().into_owned(),
                "stand-in".to_owned(),
                server.port.to_string(),
                listed,
                joining,
                mode.to_owned(),
            ],
            port: server.port,
            user: USER,
        };
        let mut joined = None;
        let code = self
            .engine
            .listen(&listener, |line| {
                if let Some(player) = minecraft::joined(line) {
                    joined = Some(player);
                }
            })
            .await?;
        ensure!(code == 0, "it ended with exit code {code}");
        Ok(joined)
    }

    /// Writes down whether a server is asleep, for a Homewarp that starts
    /// again to find it so.
    async fn write_down_sleep(&self, id: i64, asleep: bool) {
        let written = sqlx::query("UPDATE servers SET asleep = ? WHERE id = ?")
            .bind(asleep)
            .bind(id)
            .execute(&self.db)
            .await;
        if let Err(error) = written {
            tracing::error!("whether server {id} is asleep could not be written down: {error}");
        }
    }

    /// Starts asking a server that has begun to run who is on it, if it is one
    /// of Minecraft's by what its template sets up. A server of another game
    /// is sent nothing: what would be the question to Minecraft is a few bytes
    /// of nonsense to it, and its port is its own. Nor is one whose port is
    /// for UDP alone, which is not how this is asked.
    async fn ask_after(
        &self,
        server: &Definition,
        spec: &Spec,
        count: &mpsc::Sender<Option<Players>>,
    ) -> Option<Asking> {
        if server.protocol == PortProtocol::Udp || !mods::fits(&server.template) {
            return None;
        }
        // On the servers' own bridge, where it listens whatever is published.
        let address = self.engine.address_of(spec).await.ok()??;
        let at = SocketAddr::new(address, server.port);
        let count = count.clone();
        Some(Asking(tokio::spawn(async move {
            loop {
                let found = minecraft::players(at).await.ok();
                if count.send(found).await.is_err() {
                    return;
                }
                tokio::time::sleep(ASK_EVERY).await;
            }
        })))
    }

    /// Tells a server to stop, the way its template says a server is told.
    async fn tell_to_stop(
        &self,
        server: &Definition,
        spec: &Spec,
        console: &mut Console,
    ) -> anyhow::Result<()> {
        match &server.template.stop {
            Stop::Command(command) => console.send(command).await?,
            Stop::Signal(signal) => self.engine.signal(spec, signal).await?,
        }
        Ok(())
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
    /// until it has ended. Says how it ended.
    async fn serve(
        &self,
        server: &Definition,
        spec: &Spec,
        watch: &Watch,
        inbox: &mut mpsc::Receiver<Asked>,
        found_running: bool,
    ) -> Ended {
        let ended = async {
            let mut console = if found_running {
                watch.set(State::Running);
                watch.say("Homewarp started again while this server ran. What it printed before that is not here.");
                self.engine.attach(spec).await?
            } else {
                watch.set(State::Starting);
                self.fetch(&spec.image, watch).await?;
                self.prepare(server, spec, watch).await?;
                self.engine.create(spec).await?;
                let console = self.engine.attach(spec).await?;
                self.engine.start(spec).await?;
                console
            };
            // Fused, because it ends before the server's last lines have been read.
            let mut usage = pin!(self.engine.usage(spec).fuse());
            let mut told_to_stop = false;
            // Who is on it: asked from when it runs, and no more once it is
            // plain that it does not say.
            let (count, mut counted) = mpsc::channel(1);
            let mut asking = match found_running {
                true => self.ask_after(server, spec, &count).await,
                false => None,
            };
            let mut answered = false;
            let mut unanswered = 0;
            let mut empty_since: Option<Instant> = None;
            let mut sleeping = false;
            loop {
                tokio::select! {
                    Some(now) = usage.next() => watch.measure(Usage {
                        cpu_percent: now.cpu_percent,
                        memory_bytes: now.memory_bytes,
                        memory_limit_bytes: now.memory_limit_bytes,
                        received_bytes_per_second: now.received_bytes_per_second,
                        sent_bytes_per_second: now.sent_bytes_per_second,
                    }),
                    line = console.next_line() => {
                        let Some(line) = line? else { break };
                        let line = shown(&line);
                        let done = &server.template.done;
                        if watch.state() == State::Starting && done.iter().any(|done| line.contains(done.as_str())) {
                            watch.set(State::Running);
                            asking = self.ask_after(server, spec, &count).await;
                        }
                        watch.say(line);
                    }
                    Some(found) = counted.recv() => {
                        if watch.state() != State::Running {
                            continue;
                        }
                        let Some(players) = found else {
                            // One that has answered before is busy, and is asked again.
                            unanswered += 1;
                            if !answered && unanswered >= UNANSWERED {
                                asking = None;
                            }
                            continue;
                        };
                        answered = true;
                        match players.online {
                            0 => {
                                empty_since.get_or_insert_with(Instant::now);
                            }
                            _ => empty_since = None,
                        }
                        watch.count(Some(players));
                        let patience = Duration::from_secs(u64::from(server.sleep_minutes) * 60);
                        let empty_for_long = empty_since.is_some_and(|since| since.elapsed() >= patience);
                        if server.sleep_minutes > 0 && empty_for_long {
                            sleeping = true;
                            told_to_stop = true;
                            asking = None;
                            watch.say(format!(
                                "Nobody has been on for {}. Homewarp is putting it to sleep, and wakes it when a player joins.",
                                minutes(server.sleep_minutes)
                            ));
                            watch.set(State::Stopping);
                            self.tell_to_stop(server, spec, &mut console).await?;
                        }
                    }
                    asked = inbox.recv() => match asked {
                        Some(Asked::Typed(line)) => console.send(&line).await?,
                        Some(Asked::Power(Power::Stop)) if !told_to_stop => {
                            told_to_stop = true;
                            watch.set(State::Stopping);
                            self.tell_to_stop(server, spec, &mut console).await?;
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
            drop(asking);
            let code = self.engine.wait(spec).await?;
            self.engine.remove(spec).await?;
            anyhow::Ok((code, told_to_stop, sleeping))
        }
        .await;
        match ended {
            Ok((_, _, true)) => {
                watch.set(State::Asleep);
                Ended::Slept
            }
            Ok((code, told_to_stop, _)) if told_to_stop || code == 0 => {
                watch.set(State::Offline);
                Ended::Stopped
            }
            Ok((code, ..)) => {
                watch.say(format!(
                    "The server stopped by itself, with exit code {code}."
                ));
                watch.set(State::Crashed);
                let detail = format!("exit code {code}");
                audit::record_by_homewarp(&self.db, Some(server.id), "server.crash", &detail).await;
                Ended::Crashed
            }
            // Not a crash of the server's: the same would happen again at once.
            Err(error) => {
                watch.say(format!("Homewarp could not run this server: {error:#}"));
                // Whatever was made of it must not be left running unwatched.
                let _ = self.engine.remove(spec).await;
                watch.set(State::Crashed);
                Ended::Stopped
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
            // Whatever it was before, it is not asleep while it runs.
            self.write_down_sleep(server.id, false).await;
            match self
                .serve(&server, &spec, watch, inbox, found_running)
                .await
            {
                Ended::Crashed => {}
                Ended::Stopped => return,
                Ended::Slept => {
                    self.write_down_sleep(server.id, true).await;
                    let detail = format!("nobody on for {}", minutes(server.sleep_minutes));
                    audit::record_by_homewarp(&self.db, Some(server.id), "server.sleep", &detail)
                        .await;
                    return;
                }
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
                let detail = format!("{crashes} crashes one after another");
                audit::record_by_homewarp(&self.db, Some(server.id), "server.gave_up", &detail)
                    .await;
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
    /// Off the async threads: it is work on files, and on files that are
    /// the server's own, which every other server's task should not wait on.
    async fn prepare(&self, server: &Definition, spec: &Spec, watch: &Watch) -> anyhow::Result<()> {
        let (server, dir, watch) = (server.clone(), spec.dir.clone(), watch.clone());
        tokio::task::spawn_blocking(move || {
            let files = ServerDir::open(&dir, USER, USER)?;
            set_up(&files, &server, |line| watch.say(line))
        })
        .await?
    }

    /// The server as Docker needs to know it.
    fn spec(&self, server: &Definition) -> Spec {
        let host_ip = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let first = ExtraPort {
            port: server.port,
            protocol: server.protocol,
        };
        let mut ports = Vec::new();
        for one in std::iter::once(&first).chain(&server.ports) {
            let protocols: &[Protocol] = match one.protocol {
                PortProtocol::Tcp => &[Protocol::Tcp],
                PortProtocol::Udp => &[Protocol::Udp],
                PortProtocol::Both => &[Protocol::Tcp, Protocol::Udp],
            };
            ports.extend(protocols.iter().map(|protocol| Port {
                host_ip,
                port: one.port,
                protocol: *protocol,
            }));
        }
        Spec {
            id: server.uuid.clone(),
            dir: files_at(&self.data, &server.uuid),
            image: server.image.clone(),
            startup: substitute(&server.template.startup, |name| server.lookup(name)),
            variables: server.variables.clone(),
            memory_mb: server.memory_mb,
            cpu_percent: server.cpu_percent,
            ports,
            uid: USER,
            gid: USER,
            network: NETWORK.to_owned(),
            timezone: "UTC".to_owned(),
            resolvers: lock(&self.resolvers).clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use homewarp_runtime::{ServerDir, running_as};

    use super::{Definition, container_id, set_up};
    use crate::servers::PortProtocol;

    /// A server on port 25590, made from an egg that sets two files up: one
    /// line by line, as Velocity's is, and one key by key.
    fn proxy() -> Definition {
        let files = serde_json::json!({
            "velocity.toml": {
                "parser": "file",
                "find": { "bind = ": "bind = \"0.0.0.0:{{server.build.default.port}}\"" },
            },
            "server.properties": {
                "parser": "properties",
                "find": { "server-port": "{{server.build.default.port}}" },
            },
        });
        let egg = serde_json::json!({
            "meta": { "version": "PTDL_v2" },
            "name": "Proxy",
            "docker_images": { "Java": "example.invalid/java:latest" },
            "startup": "java -jar proxy.jar",
            "config": {
                "files": files.to_string(),
                "startup": "{\"done\": \"Done\"}",
                "stop": "end",
            },
            "scripts": { "installation": {
                "script": null,
                "container": "example.invalid/installer",
                "entrypoint": "sh",
            } },
            "variables": [],
        });
        Definition {
            id: 1,
            uuid: "0b0e8a0c-0000-4000-8000-000000000001".to_owned(),
            name: "Lobby".to_owned(),
            template: homewarp_template::import(&egg.to_string()).unwrap(),
            image: "example.invalid/java:latest".to_owned(),
            memory_mb: 512,
            cpu_percent: 0,
            port: 25590,
            protocol: PortProtocol::Tcp,
            ports: Vec::new(),
            variables: Vec::new(),
            eula: false,
            installed: true,
            sleep_minutes: 0,
            asleep: false,
            says_offline: false,
        }
    }

    #[test]
    fn only_a_server_of_minecrafts_has_anything_stand_in_for_it() {
        use super::{Standing, State, standing};
        // The egg of `proxy` sets up a file that is Minecraft's.
        let quiet = proxy();
        assert!(standing(State::Asleep, &quiet) == Some(Standing::Wakes));
        // Stopped, nothing stands in for it unless its owner asked.
        for state in [State::Offline, State::Crashed] {
            assert!(standing(state, &quiet).is_none());
        }
        let telling = Definition {
            says_offline: true,
            ..proxy()
        };
        for state in [State::Offline, State::Crashed] {
            assert!(standing(state, &telling) == Some(Standing::Stays));
        }
        // Not while it has a process, is being installed, or is held.
        for state in [
            State::Running,
            State::Starting,
            State::Stopping,
            State::Installing,
            State::InstallFailed,
            State::Restoring,
        ] {
            assert!(standing(state, &telling).is_none());
        }
        // Nor one whose port is for UDP alone, or that was never installed.
        let udp = Definition {
            protocol: PortProtocol::Udp,
            ..telling.clone()
        };
        assert!(standing(State::Offline, &udp).is_none());
        let never = Definition {
            installed: false,
            ..telling.clone()
        };
        assert!(standing(State::Offline, &never).is_none());
        // Nor a server of another game, whatever was asked.
        let mut other = telling;
        other.template.config_files.clear();
        assert!(standing(State::Offline, &other).is_none());
    }

    #[test]
    fn what_a_server_left_where_its_settings_belong_does_not_hold_up_a_start() {
        let directory = tempfile::tempdir().unwrap();
        let (uid, gid) = running_as();
        let files = ServerDir::open(directory.path(), uid, gid).unwrap();
        let server = Definition {
            eula: true,
            ..proxy()
        };
        // Longer than any settings are, and a folder where a file belongs.
        let long = std::fs::File::create(directory.path().join("server.properties")).unwrap();
        long.set_len(homewarp_runtime::LARGEST_SETTINGS + 1)
            .unwrap();
        std::fs::create_dir(directory.path().join("velocity.toml")).unwrap();
        std::fs::create_dir(directory.path().join("eula.txt")).unwrap();
        let mut said = Vec::new();
        set_up(&files, &server, |line| said.push(line)).unwrap();
        said.sort();
        assert_eq!(
            said,
            [
                "eula.txt was left as it is: it is not an ordinary file.",
                "server.properties was left as it is: it is longer than a file of settings is.",
                "velocity.toml was left as it is: it is not an ordinary file.",
            ]
        );
        // And left it is: nothing of it was read in, and nothing written over it.
        let length = std::fs::metadata(directory.path().join("server.properties"))
            .unwrap()
            .len();
        assert_eq!(length, homewarp_runtime::LARGEST_SETTINGS + 1);
    }

    #[test]
    fn a_file_set_line_by_line_is_left_for_the_server_to_make() {
        let directory = tempfile::tempdir().unwrap();
        let (uid, gid) = running_as();
        let files = ServerDir::open(directory.path(), uid, gid).unwrap();
        let server = proxy();
        let mut said = Vec::new();
        set_up(&files, &server, |line| said.push(line)).unwrap();
        // Not made empty: a proxy that found it so would never write its own.
        assert_eq!(files.read_to_string("velocity.toml").unwrap(), None);
        assert!(
            said.iter()
                .any(|line| line.starts_with("velocity.toml is not there yet")),
            "{said:?}"
        );
        // One that is set key by key is made, with its keys in it, as before.
        let properties = files.read_to_string("server.properties").unwrap().unwrap();
        assert!(properties.contains("server-port=25590"), "{properties}");

        // Once the server has written its own, the line is the server's port.
        let made = "motd = \"A Velocity Server\"\nbind = \"0.0.0.0:25565\"\n";
        files.write("velocity.toml", made).unwrap();
        said.clear();
        set_up(&files, &server, |line| said.push(line)).unwrap();
        assert_eq!(
            files.read_to_string("velocity.toml").unwrap().unwrap(),
            "motd = \"A Velocity Server\"\nbind = \"0.0.0.0:25590\"\n"
        );
        assert!(said.is_empty(), "{said:?}");
    }

    #[test]
    fn finds_its_own_container_among_what_is_mounted() {
        let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let mounts = format!(
            "1525 1503 0:45 / /proc rw,nosuid - proc proc rw\n\
             1601 1503 8:2 /var/lib/docker/containers/{id}/hostname /etc/hostname rw,relatime - ext4 /dev/sda2 rw\n\
             1602 1503 8:2 /var/lib/docker/containers/{id}/hosts /etc/hosts rw,relatime - ext4 /dev/sda2 rw\n"
        );
        assert_eq!(container_id(&mounts), Some(id));
        // A machine that only has Docker on it, and a path that only looks the part.
        assert_eq!(container_id("36 1 8:2 / / rw - ext4 /dev/sda2 rw\n"), None);
        assert_eq!(
            container_id("1 1 8:2 /srv/containers/web/data /data rw - ext4 /dev/sda2 rw\n"),
            None
        );
    }
}
