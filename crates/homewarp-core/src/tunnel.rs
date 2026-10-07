//! Core's end of the tunnels (PLAN.md §5.3 to §5.5): for each VPS an interface
//! and a way back at home, enrolling it, and telling its Gate what to forward.
//!
//! One task does all of it, every few seconds, for each Gate in turn: sets the
//! kernel up as the Gate in the database needs it, sees a new Gate through to
//! keys of its own, tells the Gate which ports to forward if that has changed,
//! and otherwise asks how it is. The asking is not only for show. It is
//! traffic that goes unanswered when the Gate has lost its end of the tunnel,
//! and that is what makes WireGuard at home shake hands again within seconds
//! and not minutes. And the setting up is done every time round, because
//! whatever is undone between two rounds, by a reboot or by another program,
//! has to come back.
//!
//! A home can have several VPSes, each behind a tunnel of its own (PLAN.md
//! §11, Phase 8). A server is reached through one of them, which its owner
//! chooses; Core says which one it would choose, and why.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path as Folder, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homewarp_net::{
    Link, TUNNELS, apply, bring_up, has_keep_table, has_table, home_ruleset, interface,
    keep_ruleset, network_in_the_way, new_keypair, new_preshared_key, passed, probe_packets,
    route_replies, take_table_down, take_tunnel_down, tunnels_up,
};
use homewarp_proto::{
    Answer, Answering, Desired, Forward, Guard, JoinToken, Mode, Open, Probe, ProbeRequest,
    Protocol, Rotate, Rotated, Status, Through,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::{Row, SqlitePool};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, mpsc},
    time::timeout,
};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Owner, Problem, ProblemBody, SignedIn},
    audit, auth, panel,
    runtime::Runtime,
    servers::{self, PortProtocol, Published},
    settings,
};

/// Where the tunnels' addresses begin (PLAN.md §5.3, Addressing). Each tunnel
/// has four of them, of which the two in the middle are its ends.
const NETWORK: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 0);
/// The bridge servers sit on, which `runtime` makes.
const BRIDGE: &str = "homewarp-br";
/// Where a Gate's WireGuard listens unless another port is asked for.
const WG_PORT: u16 = 51820;
/// Where a Gate answers Core, on its tunnel address.
const API_PORT: u16 = 4857;
const EVERY: Duration = Duration::from_secs(10);
/// How many rounds in a row a Gate may go unanswered before it is written
/// down as having stopped answering: half a minute.
const LOST_AFTER: u32 = 3;
/// How often a VPS that has been handed its join token is looked for.
const EVERY_WHILE_WAITING: Duration = Duration::from_secs(2);
/// How long a join token counts (PLAN.md §5.5).
const JOIN_SECONDS: i64 = 15 * 60;
/// How long the Gate takes to put its new keys to use once it has agreed to.
const GATE_SWITCHES_IN: Duration = Duration::from_millis(900);
/// How long the Gate keeps a probe's port open, and how long home waits on it.
const PROBE_SECONDS: u8 = 8;
const PROBE_WAIT: Duration = Duration::from_secs(4);
/// How long the probe's listener waits to be connected to: longer than both of those.
const LISTEN_FOR: Duration = Duration::from_secs(14);
/// What the probe's listener prints: that it is there, and where a connection came from.
const LISTENING: &str = "listening";
const FROM: &str = "from ";
/// How often what passes through each tunnel is looked at, and how many such
/// looks are kept: five minutes of them, which is what the Network page draws.
const SAMPLE_EVERY: Duration = Duration::from_secs(2);
const SAMPLES: usize = 150;
/// The most characters in what a VPS is called.
const LONGEST_NAME: usize = 40;

const MISSING: Problem = Problem::NotFound("There is no such VPS.");

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_gates, enrol_gate))
        .routes(routes!(get_activity))
        .routes(routes!(rename_gate, disconnect_gate))
        .routes(routes!(check_gate))
}

/// The two ends of a tunnel, by its number: the Gate's address inside it, and home's.
fn ends(tunnel: u8) -> (Ipv4Addr, Ipv4Addr) {
    let first = u32::from(NETWORK) + 4 * u32::from(tunnel);
    (Ipv4Addr::from(first + 1), Ipv4Addr::from(first + 2))
}

/// Whether an address is a Gate's own, inside its tunnel. Where a Gate stands
/// in for whoever comes through it, that one address is everybody on the
/// internet at once.
pub(crate) fn is_gate(address: IpAddr) -> bool {
    (0..TUNNELS).any(|tunnel| address == IpAddr::V4(ends(tunnel).0))
}

/// What the two ends know each other by, and what Core shows the Gate.
#[derive(Clone, PartialEq, Eq)]
struct Keys {
    gate_public_key: String,
    preshared_key: String,
    token: String,
}

/// Whether servers see their players' own addresses, as the self-probe found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum PlayerAddresses {
    /// They do: a connection through the Gate arrived with its own address.
    Preserved,
    /// They see the Gate's, because replies could not be sent back otherwise.
    Hidden,
    /// It could not be found out, or has not been yet.
    Unchecked,
}

impl PlayerAddresses {
    fn as_str(self) -> &'static str {
        match self {
            Self::Preserved => "preserved",
            Self::Hidden => "hidden",
            Self::Unchecked => "unchecked",
        }
    }
}

/// A Gate as the database has it.
#[derive(Clone)]
struct Gate {
    id: i64,
    /// Which of home's tunnels leads to it.
    tunnel: u8,
    name: String,
    address: String,
    wg_port: u16,
    api_port: u16,
    private_key: String,
    keys: Keys,
    /// What the Gate will switch to, in the middle of a change of keys.
    next: Option<Keys>,
    /// Until the Gate has keys of its own: its join token and when that runs out.
    join: Option<(String, i64)>,
    mode: Mode,
    checked: PlayerAddresses,
    note: Option<String>,
}

impl Gate {
    /// Where it answers Core: its own end of its tunnel.
    fn at(&self) -> Ipv4Addr {
        ends(self.tunnel).0
    }

    /// Whether it was handed a join token that has run out since.
    fn expired(&self, now: i64) -> bool {
        self.join.as_ref().is_some_and(|(_, until)| *until <= now)
    }

    /// Whether it is enrolled, with keys of its own.
    fn connected(&self) -> bool {
        self.join.is_none()
    }
}

/// Why the Gate gave no answer that could be used.
#[derive(Debug, thiserror::Error)]
enum Unanswered {
    /// Nothing came back through the tunnel.
    #[error("the Gate could not be reached through the tunnel: {0}")]
    Unreachable(String),
    /// The Gate answered, with a refusal.
    #[error("the Gate refused: {1}")]
    Refused(u16, String),
}

/// What a Gate last said, and how long the way to it took.
#[derive(Clone, Default)]
struct Heard {
    status: Option<Status>,
    latency_ms: Option<u32>,
    /// Why it could not be asked, when it could not.
    problem: Option<String>,
}

/// What passed through a tunnel in a second: to the servers, and from them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, ToSchema)]
struct Rate {
    /// From players to servers, in bytes.
    received: u64,
    /// From servers to players, in bytes.
    sent: u64,
}

/// What Core keeps of a Gate between one round and the next, in memory only.
#[derive(Default)]
struct Live {
    /// Which of home's tunnels leads to it.
    tunnel: u8,
    heard: Heard,
    /// By which of its keys, and at what address, the kernel was last set up
    /// for it.
    up_for: Option<String>,
    /// What it was last told.
    told: Option<Desired>,
    /// Set when it has just been enrolled: the self-probe is due.
    check_due: bool,
    /// What it had counted through each port when it was last heard from.
    counted: BTreeMap<(u16, Protocol), u64>,
    /// How many rounds in a row it has not answered, once connected.
    unanswered: u32,
    /// Set once it has been written down that it stopped answering.
    lost: bool,
    /// What the kernel had counted through its tunnel when it was last looked at.
    passed: Option<(u64, u64, Instant)>,
    /// What passed through its tunnel, a look at a time, the newest last.
    samples: VecDeque<Rate>,
}

impl Live {
    /// Takes one more look at what the kernel has counted through the tunnel.
    /// A count that has gone down is an interface that was made anew, and
    /// says nothing of what passed.
    fn sample(&mut self, totals: Option<(u64, u64)>, at: Instant) {
        let rate = match (totals, self.passed) {
            (Some((received, sent)), Some((received_before, sent_before, before))) => {
                let seconds = at.duration_since(before).as_secs_f64().max(0.1);
                let each =
                    |now: u64, before: u64| (now.saturating_sub(before) as f64 / seconds) as u64;
                Rate {
                    received: each(received, received_before),
                    sent: each(sent, sent_before),
                }
            }
            _ => Rate::default(),
        };
        self.passed = totals.map(|(received, sent)| (received, sent, at));
        if self.samples.len() >= SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(rate);
    }
}

/// How a connection through the Gate arrived at home.
enum Seen {
    /// Whole, from this address.
    From(IpAddr),
    /// Its first packet came, and the reply never got back.
    NoWayBack,
    /// Nothing came through the tunnel at all.
    Nothing,
}

/// Core's end of the tunnels, and what it last heard from the other ends.
pub(crate) struct Tunnel {
    db: SqlitePool,
    /// What runs servers, and for the probe a listener where a server would be.
    runtime: Option<Arc<Runtime>>,
    /// One thing at a time is done to the tunnels: a round of keeping them, or
    /// something a page asked for.
    busy: tokio::sync::Mutex<()>,
    /// Rung when something has changed that should not wait for the next round.
    wake: Notify,
    /// What is kept of each Gate, by its id.
    live: Mutex<HashMap<i64, Live>>,
    /// The tunnels this Core has put in the kernel.
    up: Mutex<BTreeSet<u8>>,
    /// Where it is written down which those are, for the Core that is started
    /// after this one: a tunnel that is in the kernel and not in that file is
    /// another Homewarp's on the same machine, and is left alone.
    ours: PathBuf,
    /// For which tunnels, and which port of the panel's, home's table was last
    /// made. It is made anew when that changes, and put back if it has gone.
    table_for: Mutex<Option<String>>,
    /// Set once it has been said that servers could not be kept from the home network.
    keep_failed: AtomicBool,
    /// The port the panel is served on over TLS, here and on a VPS alike.
    /// None, written 0, where this Core has no door for it.
    panel_port: AtomicU16,
}

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs work that waits on the kernel or on another program off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, homewarp_net::Error> + Send + 'static,
) -> anyhow::Result<T> {
    Ok(tokio::task::spawn_blocking(work).await??)
}

/// A name or an address, as one IPv4 address and a port.
async fn find(address: &str, port: u16) -> anyhow::Result<SocketAddr> {
    tokio::net::lookup_host((address, port))
        .await
        .with_context(|| format!("looking up {address}"))?
        .find(SocketAddr::is_ipv4)
        .with_context(|| format!("{address} has no IPv4 address"))
}

impl Tunnel {
    pub(crate) fn new(db: SqlitePool, runtime: Option<Arc<Runtime>>, data: &Folder) -> Arc<Self> {
        Arc::new(Self {
            db,
            runtime,
            ours: data.join("tunnels"),
            busy: tokio::sync::Mutex::default(),
            wake: Notify::new(),
            live: Mutex::default(),
            up: Mutex::default(),
            table_for: Mutex::default(),
            keep_failed: AtomicBool::new(false),
            panel_port: AtomicU16::new(0),
        })
    }

    /// Says which port the panel is served on over TLS, if it is.
    pub(crate) fn panel_at(&self, port: Option<u16>) {
        self.panel_port.store(port.unwrap_or(0), Ordering::Relaxed);
    }

    /// The port a Gate forwards for the panel: the one it is served on over
    /// TLS, once it has been given a name to be reached by.
    async fn panel(&self) -> anyhow::Result<Option<u16>> {
        let port = self.panel_port.load(Ordering::Relaxed);
        if port == 0 {
            return Ok(None);
        }
        Ok(panel::name(&self.db).await?.map(|_| port))
    }

    /// One Gate, where it is connected and has keys of its own.
    async fn connected(&self, id: i64) -> anyhow::Result<Gate> {
        let gates = self.gates().await?;
        let found = gates.into_iter().find(|gate| gate.id == id);
        found
            .filter(Gate::connected)
            .context("That VPS is not connected")
    }

    /// Asks a Gate how the guard on its VPS stands, or to change it.
    pub(crate) async fn guard(
        &self,
        id: i64,
        method: &str,
        path: &str,
        open: Option<&Open>,
    ) -> anyhow::Result<Guard> {
        let gate = self.connected(id).await?;
        match ask(&gate, &gate.keys, method, path, open).await {
            Ok((guard, _)) => Ok(guard),
            Err(Unanswered::Refused(404 | 405, _)) => bail!(
                "The Gate on the VPS is older than this Homewarp and has no guard. Put the new one there"
            ),
            Err(error) => Err(error.into()),
        }
    }

    /// Has the answer to a certificate authority's question put where port 80
    /// of the VPS that `name` leads to serves it.
    ///
    /// Which VPS that is is the name's own affair, and may change. So every
    /// connected Gate is given the answer, and what is said of it is what the
    /// Gate said whose VPS the name leads to now, where that can be told.
    pub(crate) async fn answer(
        &self,
        name: &str,
        token: &str,
        answer: &str,
    ) -> anyhow::Result<Answering> {
        let gates = self.gates().await?;
        let leads_to: Vec<IpAddr> = match tokio::net::lookup_host((name, 80)).await {
            Ok(found) => found.map(|at| at.ip()).collect(),
            Err(_) => Vec::new(),
        };
        let asked = Answer {
            answer: answer.to_owned(),
        };
        let path = format!("/v1/challenge/{token}");
        let (mut of_name, mut any, mut refused) = (None, None, None);
        for gate in gates.iter().filter(|gate| gate.connected()) {
            match ask::<Answering, _>(gate, &gate.keys, "PUT", &path, Some(&asked)).await {
                Ok((answering, _)) => {
                    let here = find(&gate.address, 80)
                        .await
                        .is_ok_and(|at| leads_to.contains(&at.ip()));
                    if here {
                        of_name.get_or_insert(answering.clone());
                    }
                    any.get_or_insert(answering);
                }
                Err(Unanswered::Refused(404 | 405, _)) => {
                    refused = Some(anyhow::anyhow!(
                        "The Gate on the VPS is older than this Homewarp and cannot answer for a certificate. Put the new one there"
                    ));
                }
                Err(error) => refused = Some(error.into()),
            }
        }
        match (of_name.or(any), refused) {
            (Some(answering), _) => Ok(answering),
            (None, Some(refused)) => Err(refused),
            (None, None) => bail!(
                "No VPS is connected, and the panel's name leads to a VPS, where a certificate's question is answered"
            ),
        }
    }

    /// Has every Gate take such an answer away again.
    pub(crate) async fn unanswer(&self, token: &str) -> anyhow::Result<()> {
        let path = format!("/v1/challenge/{token}");
        let mut failed = None;
        for gate in self.gates().await?.iter().filter(|gate| gate.connected()) {
            if let Err(error) = ask::<(), ()>(gate, &gate.keys, "DELETE", &path, None).await {
                failed = Some(error);
            }
        }
        match failed {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    /// Has the next round begin now: a server's ports have changed, or a VPS
    /// is about to be enrolled.
    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    /// Keeps the tunnels as the database says they should be, for as long as
    /// Core runs, and looks every moment at what passes through each.
    pub(crate) fn keep(self: &Arc<Self>) {
        let tunnel = Arc::clone(self);
        tokio::spawn(async move {
            // Interfaces left by a Core that was stopped before it could take
            // them down, which the Gates in the database may no longer
            // account for: those it wrote down as its own, and no others.
            let written = tokio::fs::read_to_string(&tunnel.ours).await;
            let ours: BTreeSet<u8> = written
                .unwrap_or_default()
                .split_whitespace()
                .filter_map(|number| number.parse().ok())
                .collect();
            let found: BTreeSet<u8> = tunnels_up().into_iter().collect();
            let left: BTreeSet<u8> = found.intersection(&ours).copied().collect();
            if !left.is_empty() {
                *lock(&tunnel.table_for) = Some(String::new());
            }
            *lock(&tunnel.up) = left;
            loop {
                let waiting = tunnel.round().await;
                let pause = match waiting {
                    true => EVERY_WHILE_WAITING,
                    false => EVERY,
                };
                tokio::select! {
                    () = tokio::time::sleep(pause) => {}
                    () = tunnel.wake.notified() => {}
                }
            }
        });
        let tunnel = Arc::clone(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(SAMPLE_EVERY);
            loop {
                every.tick().await;
                let watched: Vec<(i64, u8)> = lock(&tunnel.live)
                    .iter()
                    .map(|(id, live)| (*id, live.tunnel))
                    .collect();
                for (id, number) in watched {
                    // Two small files the kernel keeps, read where they are.
                    let totals = passed(number);
                    if let Some(live) = lock(&tunnel.live).get_mut(&id) {
                        live.sample(totals, Instant::now());
                    }
                }
            }
        });
    }

    /// One round. True while a VPS that was handed a join token is awaited.
    async fn round(&self) -> bool {
        let _busy = self.busy.lock().await;
        // Servers are kept from the home network whether or not there is a
        // VPS. Looked at every round: a firewall that restarts takes the
        // table with it, as it takes the tunnels'.
        if self.runtime.is_some() {
            let kept = tokio::task::spawn_blocking(|| match has_keep_table() {
                true => Ok(()),
                false => apply(&keep_ruleset(BRIDGE)?),
            });
            if !matches!(kept.await, Ok(Ok(()))) && !self.keep_failed.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(
                    "servers could not be kept from the home network: nft refused the rules that do it"
                );
            }
        }
        let gates = match self.gates().await {
            Ok(gates) => gates,
            Err(error) => {
                for live in lock(&self.live).values_mut() {
                    live.heard.problem = Some(format!("{error:#}"));
                }
                return false;
            }
        };
        let now = auth::now();
        // A join token that has run out: home stops dialling with the key it carried.
        let wanted: Vec<Gate> = gates
            .into_iter()
            .filter(|gate| !gate.expired(now))
            .collect();
        {
            // What is kept of a Gate goes when the Gate does.
            let mut live = lock(&self.live);
            live.retain(|id, _| wanted.iter().any(|gate| gate.id == *id));
            for gate in &wanted {
                live.entry(gate.id).or_default().tunnel = gate.tunnel;
            }
        }
        self.clear(&wanted.iter().map(|gate| gate.tunnel).collect())
            .await;
        let mut waiting = false;
        for gate in wanted {
            waiting |= !gate.connected();
            self.attend(gate).await;
        }
        waiting
    }

    /// Writes down which tunnels are this Core's. One that could not be
    /// written down is found again by the Gate in the database that has it.
    async fn write_down(&self) {
        let ours: Vec<String> = lock(&self.up).iter().map(u8::to_string).collect();
        if let Err(error) = tokio::fs::write(&self.ours, ours.join(" ")).await {
            tracing::warn!("which tunnels are this Homewarp's could not be written down: {error}");
        }
    }

    /// Takes out of the kernel every tunnel of this Core's that no Gate has
    /// any more, and the table once there is no tunnel left.
    async fn clear(&self, wanted: &BTreeSet<u8>) {
        let gone: Vec<u8> = {
            let mut up = lock(&self.up);
            let gone = up.difference(wanted).copied().collect();
            up.retain(|tunnel| wanted.contains(tunnel));
            gone
        };
        let any = !gone.is_empty();
        if any {
            self.write_down().await;
            let _ = tokio::task::spawn_blocking(move || {
                gone.into_iter().for_each(take_tunnel_down);
            })
            .await;
        }
        if wanted.is_empty() {
            if lock(&self.table_for).take().is_some() {
                let _ = tokio::task::spawn_blocking(take_table_down).await;
            }
        } else if any {
            // The tunnels that are left are no longer what the table says.
            if let Err(error) = self.rules(None).await {
                tracing::warn!("home's rules could not be made anew: {error:#}");
            }
        }
    }

    /// One round with one Gate, and what came of it written down.
    async fn attend(&self, gate: Gate) {
        let (id, waiting, name) = (gate.id, !gate.connected(), gate.name.clone());
        let heard = match self.tend(gate).await {
            Ok((status, latency)) => {
                if let Err(error) = self.count(id, &status.traffic).await {
                    tracing::warn!("what went through the Gate could not be added up: {error:#}");
                }
                Heard {
                    status: Some(status),
                    latency_ms: Some(latency),
                    problem: None,
                }
            }
            // A VPS that has not run its command yet is no problem to report.
            Err(error)
                if waiting
                    && matches!(
                        error.downcast_ref::<Unanswered>(),
                        Some(Unanswered::Unreachable(_))
                    ) =>
            {
                Heard::default()
            }
            Err(error) => Heard {
                problem: Some(format!("{error:#}")),
                ..Heard::default()
            },
        };
        // Written down when it stops answering and when it answers again, so
        // that the owner is told of a VPS that is down. Not at the first round
        // without an answer: a tunnel misses one now and then.
        let mut said = None;
        {
            let mut live = lock(&self.live);
            let live = live.entry(id).or_default();
            if !waiting {
                if heard.status.is_some() {
                    live.unanswered = 0;
                    if std::mem::take(&mut live.lost) {
                        said = Some(("gate.back", name));
                    }
                } else {
                    live.unanswered += 1;
                    if live.unanswered == LOST_AFTER && !std::mem::replace(&mut live.lost, true) {
                        let why = heard.problem.clone().unwrap_or_default();
                        said = Some(("gate.lost", format!("{name} ({why})")));
                    }
                }
            }
            live.heard = heard;
        }
        if let Some((what, detail)) = said {
            audit::record_by_homewarp(&self.db, None, what, &detail).await;
        }
    }

    /// Adds what has gone through each port since the Gate was last heard
    /// from to the count of the hour it is now.
    ///
    /// The Gate counts from when it last set its rules, so a count that has
    /// gone down has started again and is all new. The first count this Core
    /// sees of a port is where it counts on from, and is not added: how much
    /// of it went through while no Core was listening is not known.
    async fn count(&self, gate: i64, traffic: &[Through]) -> anyhow::Result<()> {
        let hour = auth::now() / 3600;
        let more: Vec<(u16, &str, u64)> = {
            let mut live = lock(&self.live);
            let live = live.entry(gate).or_default();
            // Only the ports this Core asked to have forwarded. A Gate is not
            // taken at its word for which those are: one that named every port
            // there is would be given a row for each, every hour.
            let asked: Vec<(u16, Protocol)> = live
                .told
                .as_ref()
                .map(|told| {
                    let forwards = told.forwards.iter();
                    forwards
                        .map(|forward| (forward.port, forward.protocol))
                        .collect()
                })
                .unwrap_or_default();
            traffic
                .iter()
                .filter(|now| asked.contains(&(now.port, now.protocol)))
                .filter_map(|now| {
                    let more = match live.counted.insert((now.port, now.protocol), now.bytes) {
                        None => 0,
                        Some(before) if now.bytes >= before => now.bytes - before,
                        Some(_) => now.bytes,
                    };
                    let protocol = match now.protocol {
                        Protocol::Tcp => "tcp",
                        Protocol::Udp => "udp",
                    };
                    (more > 0).then_some((now.port, protocol, more))
                })
                .collect()
        };
        for (port, protocol, bytes) in more {
            sqlx::query(
                "INSERT INTO traffic (gate_id, port, protocol, hour, bytes) VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT (gate_id, port, protocol, hour)
                 DO UPDATE SET bytes = bytes + excluded.bytes",
            )
            .bind(gate)
            .bind(port)
            .bind(protocol)
            .bind(hour)
            .bind(i64::try_from(bytes).unwrap_or(i64::MAX))
            .execute(&self.db)
            .await?;
        }
        // A week of hours is kept, of which a day is shown.
        sqlx::query("DELETE FROM traffic WHERE hour < ?")
            .bind(hour - 7 * 24)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    /// Every Gate, the first that was connected first.
    async fn gates(&self) -> anyhow::Result<Vec<Gate>> {
        let rows = sqlx::query(
            "SELECT id, tunnel, name, address, wg_port, api_port, private_key, gate_public_key,
                    preshared_key, token, next_public_key, next_preshared_key, next_token,
                    join_token, join_expires_at, mode, checked, note
             FROM gates ORDER BY id",
        )
        .fetch_all(&self.db)
        .await?;
        let mut gates = Vec::with_capacity(rows.len());
        for row in rows {
            type Text = Option<String>;
            let next: (Text, Text, Text) = (
                row.try_get("next_public_key")?,
                row.try_get("next_preshared_key")?,
                row.try_get("next_token")?,
            );
            let next = match next {
                (Some(gate_public_key), Some(preshared_key), Some(token)) => Some(Keys {
                    gate_public_key,
                    preshared_key,
                    token,
                }),
                _ => None,
            };
            let join: (Text, Option<i64>) =
                (row.try_get("join_token")?, row.try_get("join_expires_at")?);
            gates.push(Gate {
                id: row.try_get("id")?,
                tunnel: row.try_get::<i64, _>("tunnel")?.try_into()?,
                name: row.try_get("name")?,
                address: row.try_get("address")?,
                wg_port: row.try_get::<i64, _>("wg_port")?.try_into()?,
                api_port: row.try_get::<i64, _>("api_port")?.try_into()?,
                private_key: row.try_get("private_key")?,
                keys: Keys {
                    gate_public_key: row.try_get("gate_public_key")?,
                    preshared_key: row.try_get("preshared_key")?,
                    token: row.try_get("token")?,
                },
                next,
                join: join.0.zip(join.1),
                mode: match row.try_get::<String, _>("mode")?.as_str() {
                    "nat" => Mode::Nat,
                    _ => Mode::Transparent,
                },
                checked: match row.try_get::<String, _>("checked")?.as_str() {
                    "preserved" => PlayerAddresses::Preserved,
                    "hidden" => PlayerAddresses::Hidden,
                    _ => PlayerAddresses::Unchecked,
                },
                note: row.try_get("note")?,
            });
        }
        Ok(gates)
    }

    /// One round with a Gate: the kernel, its keys if it is new, then what it
    /// forwards. Returns what it said and how many milliseconds away it is.
    async fn tend(&self, gate: Gate) -> anyhow::Result<(Status, u32)> {
        self.set_up(&gate, &gate.keys).await?;
        let gate = match gate.join {
            Some(_) => self.enrol(gate).await?,
            None => gate,
        };
        let mut heard = self.tell(&gate, gate.mode).await?;
        let due = lock(&self.live)
            .get_mut(&gate.id)
            .is_some_and(|live| std::mem::take(&mut live.check_due));
        if due {
            match self.check(&gate).await {
                Ok(mode) => heard = self.tell(&gate, mode).await?,
                // The tunnel is up all the same. What could not be found out
                // is said where its answer would have been, and the page has
                // a button to try again.
                Err(error) => {
                    sqlx::query("UPDATE gates SET note = ? WHERE id = ?")
                        .bind(format!("The check could not be made: {error:#}."))
                        .bind(gate.id)
                        .execute(&self.db)
                        .await?;
                }
            }
        }
        Ok(heard)
    }

    /// Home's end of one tunnel in the kernel: the interface, dialling the
    /// Gate; the way back for replies; and the rules. Done again it changes
    /// nothing, and it is done every round.
    async fn set_up(&self, gate: &Gate, keys: &Keys) -> anyhow::Result<()> {
        let endpoint = find(&gate.address, gate.wg_port).await?;
        let number = gate.tunnel;
        let link = Link {
            interface: interface(number),
            private_key: gate.private_key.clone(),
            listen_port: 0,
            address: ends(number).1,
            peer_public_key: keys.gate_public_key.clone(),
            preshared_key: keys.preshared_key.clone(),
            // Players come from anywhere, and their packets come in by this link.
            peer_allowed: (Ipv4Addr::UNSPECIFIED, 0),
            peer_endpoint: Some(endpoint),
        };
        blocking(move || {
            bring_up(&link)?;
            route_replies(number)
        })
        .await?;
        if lock(&self.up).insert(number) {
            self.write_down().await;
        }
        let which = format!("{endpoint} {}", keys.gate_public_key);
        {
            let mut live = lock(&self.live);
            let live = live.entry(gate.id).or_default();
            live.tunnel = number;
            if live.up_for.as_deref() != Some(which.as_str()) {
                live.up_for = Some(which);
                // A Gate at another address, or with other keys, has not been told anything.
                live.told = None;
            }
        }
        self.rules(None).await
    }

    /// The tunnels that are to be up: every Gate's but one whose join token has run out.
    async fn tunnels(&self) -> anyhow::Result<Vec<u8>> {
        let numbers: Vec<(i64,)> = sqlx::query_as(
            "SELECT tunnel FROM gates WHERE join_expires_at IS NULL OR join_expires_at > ?
             ORDER BY tunnel",
        )
        .bind(auth::now())
        .fetch_all(&self.db)
        .await?;
        let numbers = numbers.into_iter();
        Ok(numbers
            .filter_map(|(tunnel,)| u8::try_from(tunnel).ok())
            .collect())
    }

    /// Home's table, for every tunnel there is to be. Made once for what it
    /// says, so that a newer Core's rules take the place of an older one's,
    /// and put back if it has gone. `probe` is a tunnel and a port to count
    /// what arrives for, while its VPS is looked at.
    async fn rules(&self, probe: Option<(u8, u16)>) -> anyhow::Result<()> {
        let tunnels = self.tunnels().await?;
        if tunnels.is_empty() {
            return Ok(());
        }
        // The rules differ by whether the panel has a name: its port is let
        // in from the tunnels only then.
        let panel = self.panel().await?;
        let which = format!("{tunnels:?} {panel:?}");
        let again = probe.is_none() && lock(&self.table_for).as_deref() == Some(which.as_str());
        blocking(move || {
            if !again || !has_table() {
                apply(&home_ruleset(BRIDGE, &tunnels, probe, panel)?)?;
            }
            Ok(())
        })
        .await?;
        // With a probe in it, it is not the table that is to stay.
        *lock(&self.table_for) = Some(match probe {
            None => which,
            Some(_) => String::new(),
        });
        Ok(())
    }

    /// Sees a new Gate through to keys of its own (PLAN.md §5.5): what its
    /// join token carried has travelled, and after this none of it counts.
    ///
    /// In two steps, so that there is no moment at which a lost answer leaves
    /// the two ends with different keys for good. The Gate first makes its
    /// keys and goes on with the old ones; home writes the new ones down; only
    /// then is the Gate told to switch. Returns the Gate as it is afterwards.
    async fn enrol(&self, gate: Gate) -> anyhow::Result<Gate> {
        let next = match &gate.next {
            Some(next) => next.clone(),
            None => {
                let preshared_key = new_preshared_key();
                let asked = Rotate {
                    preshared_key: preshared_key.clone(),
                };
                let (made, _): (Rotated, _) =
                    ask(&gate, &gate.keys, "POST", "/v1/rotate", Some(&asked)).await?;
                let next = Keys {
                    gate_public_key: made.public_key,
                    preshared_key,
                    token: made.token,
                };
                sqlx::query(
                    "UPDATE gates SET next_public_key = ?, next_preshared_key = ?, next_token = ?
                     WHERE id = ?",
                )
                .bind(&next.gate_public_key)
                .bind(&next.preshared_key)
                .bind(&next.token)
                .bind(gate.id)
                .execute(&self.db)
                .await?;
                next
            }
        };
        match ask::<(), ()>(&gate, &gate.keys, "POST", "/v1/rotate/commit", None).await {
            Ok(_) => tokio::time::sleep(GATE_SWITCHES_IN).await,
            // The Gate has no new keys to switch to: it was started again
            // after it made them. They are asked for again the next time round.
            Err(Unanswered::Refused(409, _)) => {
                sqlx::query(
                    "UPDATE gates
                     SET next_public_key = NULL, next_preshared_key = NULL, next_token = NULL
                     WHERE id = ?",
                )
                .bind(gate.id)
                .execute(&self.db)
                .await?;
                bail!("The Gate was started again while its keys were being changed.");
            }
            Err(refused @ Unanswered::Refused(..)) => return Err(refused.into()),
            // Not reached with the old keys. It may have switched already,
            // and its answer been lost on the way: so the new ones are tried.
            Err(unreachable @ Unanswered::Unreachable(_)) => {
                self.set_up(&gate, &next).await?;
                if ask::<Status, ()>(&gate, &next, "GET", "/v1/status", None)
                    .await
                    .is_err()
                {
                    self.set_up(&gate, &gate.keys).await?;
                    return Err(unreachable.into());
                }
            }
        }
        sqlx::query(
            "UPDATE gates
             SET gate_public_key = ?, preshared_key = ?, token = ?,
                 next_public_key = NULL, next_preshared_key = NULL, next_token = NULL,
                 join_token = NULL, join_expires_at = NULL
             WHERE id = ?",
        )
        .bind(&next.gate_public_key)
        .bind(&next.preshared_key)
        .bind(&next.token)
        .bind(gate.id)
        .execute(&self.db)
        .await?;
        // A server that is reached through no VPS yet is reached through this one.
        sqlx::query("UPDATE servers SET gate_id = ? WHERE gate_id IS NULL")
            .bind(gate.id)
            .execute(&self.db)
            .await?;
        let gate = Gate {
            keys: next,
            next: None,
            join: None,
            ..gate
        };
        self.set_up(&gate, &gate.keys).await?;
        tracing::info!("The Gate at {} has keys of its own now.", gate.address);
        lock(&self.live).entry(gate.id).or_default().check_due = true;
        Ok(gate)
    }

    /// Tells the Gate what to forward and in which mode, if it has not been
    /// told just that already, and asks how it is if it has. What it forwards
    /// are the ports of the servers that are reached through it.
    async fn tell(&self, gate: &Gate, mode: Mode) -> anyhow::Result<(Status, u32)> {
        let mut forwards = Vec::new();
        for published in servers::published(&self.db).await? {
            if published.gate_id != Some(gate.id) {
                continue;
            }
            for protocol in published.protocol.each() {
                forwards.push(Forward {
                    port: published.port,
                    protocol: *protocol,
                });
            }
        }
        // And the panel's own, as one more: where its door is, at home. Every
        // Gate forwards it, so that its name may lead to any of them.
        if let Some(port) = self.panel().await? {
            forwards.push(Forward {
                port,
                protocol: Protocol::Tcp,
            });
        }
        let rate = Some(settings::new_connections(&self.db).await?);
        let told = lock(&self.live)
            .get(&gate.id)
            .and_then(|live| live.told.clone());
        let same = |told: &Desired| {
            told.forwards == forwards && told.mode == mode && told.new_per_second == rate
        };
        if let Some(told) = told.filter(same) {
            let heard: (Status, _) =
                ask::<_, ()>(gate, &gate.keys, "GET", "/v1/status", None).await?;
            // A Gate that has forgotten what it was told, a new VPS say, is told again.
            if heard.0.generation == told.generation {
                return Ok(heard);
            }
        }
        let desired = Desired {
            generation: auth::now().unsigned_abs(),
            mode,
            forwards,
            new_per_second: rate,
        };
        let heard = ask(gate, &gate.keys, "PUT", "/v1/state", Some(&desired)).await?;
        lock(&self.live).entry(gate.id).or_default().told = Some(desired);
        Ok(heard)
    }

    /// The self-probe (PLAN.md §5.3): finds out whether a server at home sees
    /// its players' own addresses, falls back to the Gate standing in for them
    /// if replies cannot be sent back otherwise, and writes down which it is.
    /// Returns the mode the Gate is to be in.
    async fn check(&self, gate: &Gate) -> anyhow::Result<Mode> {
        self.tell(gate, Mode::Transparent).await?;
        let (mode, checked, note) = match self.probe(gate).await? {
            Seen::From(from) if from != IpAddr::V4(gate.at()) => {
                (Mode::Transparent, PlayerAddresses::Preserved, None)
            }
            Seen::From(_) => (
                Mode::Transparent,
                PlayerAddresses::Unchecked,
                Some(
                    "The Gate stood in for the test connection when it was told not to.".to_owned(),
                ),
            ),
            Seen::Nothing => (
                Mode::Transparent,
                PlayerAddresses::Unchecked,
                Some(format!(
                    "A test connection to {} did not come back through the tunnel, so this could not be checked. A firewall in front of the VPS may be closed for the port it tried.",
                    gate.address
                )),
            ),
            Seen::NoWayBack => {
                self.tell(gate, Mode::Nat).await?;
                match self.probe(gate).await? {
                    Seen::From(_) => (
                        Mode::Nat,
                        PlayerAddresses::Hidden,
                        Some(
                            "Replies to players cannot leave this machine through the tunnel, so the Gate stands in for them: servers see its address and not their players' own."
                                .to_owned(),
                        ),
                    ),
                    _ => (
                        Mode::Transparent,
                        PlayerAddresses::Unchecked,
                        Some(
                            "Connections arrive from the Gate, but replies do not get back to it. Something on this machine is in their way."
                                .to_owned(),
                        ),
                    ),
                }
            }
        };
        sqlx::query("UPDATE gates SET mode = ?, checked = ?, note = ? WHERE id = ?")
            .bind(match mode {
                Mode::Transparent => "transparent",
                Mode::Nat => "nat",
            })
            .bind(checked.as_str())
            .bind(note)
            .bind(gate.id)
            .execute(&self.db)
            .await?;
        Ok(mode)
    }

    /// One connection through the Gate and back home, and how it arrived.
    ///
    /// It has to come the way a player's does, or it says nothing about
    /// players. So the listener is this program, run once in a container on
    /// the servers' bridge with a port published as a server's is, and the
    /// Gate sends a port of the VPS's public address there for a few seconds.
    /// Core then connects to that, out through the home's own line as a player
    /// would come, and the listener says what address the connection came from.
    async fn probe(&self, gate: &Gate) -> anyhow::Result<Seen> {
        let runtime = self.runtime.as_ref().context(
            "Homewarp cannot reach Docker, and the check has to listen where a server would",
        )?;
        // A port of this machine that nothing has, let go of at once for Docker to publish.
        let port = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?
            .local_addr()?
            .port();
        self.rules(Some((gate.tunnel, port))).await?;
        let (said, mut lines) = mpsc::unbounded_channel::<String>();
        let listening = runtime.listen_once(port, move |line| {
            // An error here says that nobody is waiting for it any more.
            let _ = said.send(line.trim().to_owned());
        });
        let dialling = async {
            // Not before the listener is there to be found.
            loop {
                match lines.recv().await {
                    Some(line) if line == LISTENING => break,
                    Some(_) => {}
                    None => bail!("the listener ended before it had begun"),
                }
            }
            let asked = ProbeRequest {
                home_port: port,
                seconds: PROBE_SECONDS,
            };
            let (opened, _): (Probe, _) =
                ask(gate, &gate.keys, "POST", "/v1/probe", Some(&asked)).await?;
            let public = find(&gate.address, opened.port).await?;
            // Held until the listener has spoken, so that the connection is
            // still open when the listener gets to it.
            let _dialled = timeout(PROBE_WAIT, TcpStream::connect(public)).await;
            while let Ok(Some(line)) = timeout(PROBE_WAIT, lines.recv()).await {
                if let Some(from) = line.strip_prefix(FROM) {
                    return Ok(from.parse::<IpAddr>().ok());
                }
            }
            anyhow::Ok(None)
        };
        let (listened, arrived) = tokio::join!(listening, dialling);
        // Counted before the rules that count it are replaced by the usual ones.
        let packets = blocking(probe_packets).await;
        self.rules(None).await?;
        listened?;
        Ok(match (arrived?, packets?) {
            (Some(from), _) => Seen::From(from),
            (None, 0) => Seen::Nothing,
            (None, _) => Seen::NoWayBack,
        })
    }

    /// Each connected Gate as a server's owner would weigh it, for saying
    /// which a server is best reached through.
    fn weighed(&self, gates: &[Gate], ports: &[Published]) -> Vec<Weighed> {
        let live = lock(&self.live);
        let weighed = gates.iter().filter(|gate| gate.connected()).map(|gate| {
            let live = live.get(&gate.id);
            let status = live.and_then(|live| live.heard.status.as_ref());
            let on_it: BTreeSet<i64> = ports
                .iter()
                .filter(|port| port.gate_id == Some(gate.id))
                .map(|port| port.server_id)
                .collect();
            let now = live.and_then(|live| live.samples.back().copied());
            Weighed {
                id: gate.id,
                reachable: status.is_some(),
                latency_ms: live.and_then(|live| live.heard.latency_ms),
                hidden: gate.checked == PlayerAddresses::Hidden,
                load_percent: status.and_then(|status| status.load_percent),
                servers: on_it.len(),
                bytes_per_second: now.map_or(0, |now| now.received + now.sent),
            }
        });
        weighed.collect()
    }
}

/// A connected Gate, as what is known of it bears on which VPS a server is
/// best reached through.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Weighed {
    id: i64,
    reachable: bool,
    /// How long the way from home to it and back takes.
    latency_ms: Option<u32>,
    /// Whether servers behind it see its address and not their players' own.
    hidden: bool,
    load_percent: Option<u32>,
    /// How many servers are reached through it already.
    servers: usize,
    /// What passes through its tunnel now, both ways together.
    bytes_per_second: u64,
}

impl Weighed {
    /// What counts against it, as one number: the lower, the better. A
    /// millisecond of the way there is the measure. A VPS that hides players'
    /// addresses is chosen only when no other will do; a busy one, and one
    /// that carries much already, lose to one that is as near and idle.
    fn against(&self) -> u64 {
        let megabits = self.bytes_per_second * 8 / 1_000_000;
        u64::from(self.latency_ms.unwrap_or(500))
            + u64::from(self.load_percent.unwrap_or(0)) / 2
            + megabits
            + 5 * self.servers as u64
            + if self.hidden { 10_000 } else { 0 }
    }

    /// The same, in the words a person would give for it.
    fn why(&self) -> String {
        let mut why = Vec::new();
        if let Some(latency) = self.latency_ms {
            why.push(format!("{latency} ms from home"));
        }
        if let Some(load) = self.load_percent {
            why.push(format!("{load} % busy"));
        }
        why.push(match self.servers {
            0 => "no server on it yet".to_owned(),
            1 => "1 server on it".to_owned(),
            servers => format!("{servers} servers on it"),
        });
        why.join(", ")
    }
}

/// The VPS a server is best reached through, of those that answer: the one
/// with the least against it, and the one connected first where two are even.
fn best(weighed: &[Weighed]) -> Option<&Weighed> {
    weighed
        .iter()
        .filter(|gate| gate.reachable)
        .min_by_key(|gate| (gate.against(), gate.id))
}

/// The VPS a new server is reached through unless another is asked for: the
/// best of those that answer, or the first connected if none answers just now.
/// None where no VPS is connected.
pub(crate) async fn chosen(state: &AppState) -> anyhow::Result<Option<i64>> {
    let (gates, ports) = tokio::try_join!(state.tunnel.gates(), servers::published(&state.db))?;
    let weighed = state.tunnel.weighed(&gates, &ports);
    Ok(best(&weighed).or(weighed.first()).map(|gate| gate.id))
}

/// Whether a server may be reached through the VPS with this id: it has to be
/// one that is connected.
pub(crate) async fn is_connected(state: &AppState, id: i64) -> anyhow::Result<bool> {
    let gates = state.tunnel.gates().await?;
    Ok(gates.iter().any(|gate| gate.id == id && gate.connected()))
}

/// What the probe's container runs (`homewarp probe-listen <port>`): listens
/// once where a server would, and says where the connection it gets came from.
pub async fn probe_listen(port: u16) -> anyhow::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).await?;
    println!("{LISTENING}");
    if let Ok(Ok((_, from))) = timeout(LISTEN_FOR, listener.accept()).await {
        println!("{FROM}{}", from.ip());
    }
    Ok(())
}

/// One request to a Gate, through its tunnel, and how many milliseconds the
/// way there and back took. Its API is a handful of small JSON answers from a
/// program of ours, so this speaks just enough HTTP for that and brings no
/// client library with it.
async fn ask<T: DeserializeOwned, B: Serialize>(
    gate: &Gate,
    keys: &Keys,
    method: &str,
    path: &str,
    body: Option<&B>,
) -> Result<(T, u32), Unanswered> {
    let unreachable = |error: &dyn std::fmt::Display| Unanswered::Unreachable(error.to_string());
    let body = match body {
        Some(body) => serde_json::to_vec(body).map_err(|error| unreachable(&error))?,
        None => Vec::new(),
    };
    let talk = async {
        let began = Instant::now();
        let mut stream = TcpStream::connect(SocketAddr::from((gate.at(), gate.api_port))).await?;
        // Connecting is one trip there and back, and nothing else.
        let latency = began.elapsed();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: gate\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            keys.token,
            body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&body).await?;
        let mut answer = Vec::new();
        stream.take(1 << 20).read_to_end(&mut answer).await?;
        std::io::Result::Ok((answer, latency))
    };
    let (answer, latency) = timeout(Duration::from_secs(5), talk)
        .await
        .map_err(|_| unreachable(&"it did not answer"))?
        .map_err(|error| unreachable(&error))?;
    let answer = String::from_utf8_lossy(&answer);
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .ok_or_else(|| unreachable(&"what it answered was not HTTP"))?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| unreachable(&"what it answered was not HTTP"))?;
    if !(200..300).contains(&status) {
        return Err(Unanswered::Refused(status, body.trim().to_owned()));
    }
    // An answer with nothing in it is read as JSON's nothing.
    let body = match body.trim() {
        "" => "null",
        body => body,
    };
    let said = serde_json::from_str(body).map_err(|error| unreachable(&error))?;
    Ok((said, u32::try_from(latency.as_millis()).unwrap_or(u32::MAX)))
}

/// Where a Gate stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
enum GateState {
    /// A VPS has been handed its command, and has not been heard from yet.
    Waiting,
    /// The command was not run in time. It takes a new one.
    Expired,
    /// A Gate is enrolled. Whether it answers just now is `reachable`.
    Connected,
}

/// A port of a server, which the Gate of its VPS forwards.
#[derive(Serialize, ToSchema)]
struct ForwardedPort {
    port: u16,
    protocol: PortProtocol,
    server_id: i64,
    /// The server's name.
    server: String,
    /// The VPS the server is reached through. None while it is reached on the
    /// home network only.
    gate_id: Option<i64>,
    /// What that VPS's Gate counted through it in the last day, both ways
    /// together. Nothing where no Gate has counted.
    traffic_bytes: i64,
}

/// What went through each port of each Gate in the last day, as the hours of it add up.
async fn through(db: &SqlitePool) -> anyhow::Result<HashMap<(i64, u16), i64>> {
    let counted: Vec<(i64, u16, i64)> = sqlx::query_as(
        "SELECT gate_id, port, SUM(bytes) FROM traffic WHERE hour > ? GROUP BY gate_id, port",
    )
    .bind(auth::now() / 3600 - 24)
    .fetch_all(db)
    .await?;
    let counted = counted.into_iter();
    Ok(counted
        .map(|(gate, port, bytes)| ((gate, port), bytes))
        .collect())
}

/// A VPS, and how the tunnel to it is doing.
#[derive(Serialize, ToSchema)]
struct GateView {
    id: i64,
    /// What its owner calls it. Its address, until it is called something.
    name: String,
    state: GateState,
    /// The VPS's public address: what players type.
    address: String,
    /// While the VPS is awaited: what to run on it, as root. Where this
    /// Homewarp knows where its releases are, the command fetches the Gate
    /// first; otherwise it takes the Gate to be on the VPS already.
    command: Option<String>,
    /// When that command stops counting, in Unix seconds.
    expires_at: Option<i64>,
    /// Whether the Gate answered through the tunnel when it was last asked.
    reachable: bool,
    /// How long the way to the Gate and back takes, in milliseconds.
    latency_ms: Option<u32>,
    /// How long ago the Gate last heard from home over the tunnel.
    handshake_age_seconds: Option<u64>,
    /// Whether servers see their players' own addresses.
    player_addresses: PlayerAddresses,
    /// What the check of that found, where it needs saying.
    note: Option<String>,
    /// Why the Gate could not be reached, when it could not.
    problem: Option<String>,
    /// The version of the Gate program on the VPS.
    version: Option<String>,
    /// How busy the VPS is: 100 is every processor it has in use. Nothing
    /// from a Gate that does not say.
    load_percent: Option<u32>,
    /// How many servers are reached through it.
    servers: usize,
    /// What passes through its tunnel in a second, now: from players to
    /// servers, and from servers to players.
    received_bytes_per_second: u64,
    sent_bytes_per_second: u64,
    /// What its Gate counted through the ports of servers in the last day,
    /// both ways together.
    traffic_bytes: i64,
}

/// The VPS a server is best reached through, and what speaks for it.
#[derive(Serialize, ToSchema)]
struct Recommended {
    gate_id: i64,
    /// As a line under the choice: "23 ms from home, 12 % busy, 1 server on it".
    why: String,
}

/// Every VPS, and every port players reach a server by.
#[derive(Serialize, ToSchema)]
struct Network {
    /// The VPSes, the first that was connected first.
    gates: Vec<GateView>,
    /// Which of them a server is best reached through, where more than
    /// nothing is known. A new server is reached through it unless another
    /// is asked for.
    recommended: Option<Recommended>,
    /// Every port of every server.
    ports: Vec<ForwardedPort>,
}

/// What it takes to start connecting a VPS.
#[derive(Deserialize, ToSchema)]
struct NewGate {
    /// The VPS's public IPv4 address, or a name for it.
    address: String,
    /// What to call it. Its address, if nothing is given.
    name: Option<String>,
    /// The UDP port its WireGuard is to listen on. 51820 if none is given.
    wg_port: Option<u16>,
}

/// What can be changed of a VPS that is connected.
#[derive(Deserialize, ToSchema)]
struct GateSettings {
    /// What to call it.
    name: String,
}

/// What passed through one VPS's tunnel, a look at a time.
#[derive(Serialize, ToSchema)]
struct GateActivity {
    id: i64,
    /// The newest last. Fewer than the whole of it for a tunnel that has not
    /// been up that long.
    samples: Vec<Rate>,
}

/// What passes through the tunnels now, and has for the last few minutes.
#[derive(Serialize, ToSchema)]
struct Activity {
    /// How many seconds lie between one sample and the next.
    every_seconds: u64,
    /// How many samples the whole of it is.
    most: usize,
    gates: Vec<GateActivity>,
}

/// A name for a VPS as it is kept: trimmed, and the address where none is given.
fn named(name: Option<&str>, address: &str) -> Result<String, Problem> {
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    let name = name.unwrap_or(address);
    if name.chars().count() > LONGEST_NAME || name.chars().any(char::is_control) {
        return Err(Problem::Invalid(
            "A VPS's name is 40 characters at the most.".into(),
        ));
    }
    Ok(name.to_owned())
}

async fn view(state: &AppState) -> Result<Network, Problem> {
    // None of the reads depends on another, so none waits for another.
    let (gates, ports, through) = tokio::try_join!(
        state.tunnel.gates(),
        servers::published(&state.db),
        through(&state.db)
    )?;
    let now = auth::now();
    let weighed = state.tunnel.weighed(&gates, &ports);
    let recommended = best(&weighed).map(|gate| Recommended {
        gate_id: gate.id,
        why: gate.why(),
    });
    let live = lock(&state.tunnel.live);
    let gates = gates
        .into_iter()
        .map(|gate| {
            let live = live.get(&gate.id);
            let heard = live.map(|live| live.heard.clone()).unwrap_or_default();
            let status = heard.status;
            let rate = live
                .and_then(|live| live.samples.back().copied())
                .unwrap_or_default();
            let weighed = weighed.iter().find(|weighed| weighed.id == gate.id);
            let waits = gate.join.as_ref().filter(|(_, until)| *until > now);
            GateView {
                id: gate.id,
                state: match &gate.join {
                    Some((_, until)) if *until <= now => GateState::Expired,
                    Some(_) => GateState::Waiting,
                    None => GateState::Connected,
                },
                command: waits.map(|(token, _)| match &state.releases {
                    // One line that fetches the Gate, checks it and enrols the VPS.
                    Some(releases) => {
                        format!("curl -fsSL {releases}/install-gate.sh | sh -s -- {token}")
                    }
                    // The Gate is taken to be on the VPS already.
                    None => format!("homewarp-gate join {token}"),
                }),
                expires_at: gate.join.as_ref().map(|(_, until)| *until),
                reachable: status.is_some(),
                latency_ms: heard.latency_ms,
                handshake_age_seconds: status
                    .as_ref()
                    .and_then(|status| status.handshake_age_seconds),
                player_addresses: gate.checked,
                note: gate.note,
                problem: heard.problem,
                version: status.as_ref().map(|status| status.version.clone()),
                load_percent: status.as_ref().and_then(|status| status.load_percent),
                servers: weighed.map_or(0, |weighed| weighed.servers),
                received_bytes_per_second: rate.received,
                sent_bytes_per_second: rate.sent,
                traffic_bytes: through
                    .iter()
                    .filter(|((of, _), _)| *of == gate.id)
                    .map(|(_, bytes)| *bytes)
                    .sum(),
                name: gate.name,
                address: gate.address,
            }
        })
        .collect();
    let ports = ports
        .into_iter()
        .map(|published| ForwardedPort {
            traffic_bytes: published
                .gate_id
                .and_then(|gate| through.get(&(gate, published.port)).copied())
                .unwrap_or(0),
            port: published.port,
            protocol: published.protocol,
            server_id: published.server_id,
            server: published.server,
            gate_id: published.gate_id,
        })
        .collect();
    Ok(Network {
        gates,
        recommended,
        ports,
    })
}

/// Every VPS, how the tunnel to each is doing, and every port they forward.
/// The one request the Network page needs, and the one the sidebar's mark
/// asks again.
#[utoipa::path(
    get,
    path = "/api/v1/gates",
    responses(
        (status = OK, body = Network),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn get_gates(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
) -> Result<Json<Network>, Problem> {
    let mut view = view(&state).await?;
    // A command enrols a VPS, and the ports are every server's: both are the
    // owner's. Any account may know where players reach the servers it is in.
    if !who.owner {
        for gate in &mut view.gates {
            gate.command = None;
        }
        view.ports.clear();
    }
    Ok(Json(view))
}

/// What passes through each VPS's tunnel now, and has for the last few
/// minutes: what the Network page draws. Home counts it, at its own end of
/// each tunnel, so nothing is asked of a VPS for it.
#[utoipa::path(
    get,
    path = "/api/v1/gates/activity",
    responses(
        (status = OK, body = Activity),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = FORBIDDEN, body = ProblemBody, description = "The account is not the owner's."),
    )
)]
async fn get_activity(State(state): State<AppState>, _: Owner) -> Json<Activity> {
    let live = lock(&state.tunnel.live);
    let mut gates: Vec<GateActivity> = live
        .iter()
        .map(|(id, live)| GateActivity {
            id: *id,
            samples: live.samples.iter().copied().collect(),
        })
        .collect();
    gates.sort_by_key(|gate| gate.id);
    Json(Activity {
        every_seconds: SAMPLE_EVERY.as_secs(),
        most: SAMPLES,
        gates,
    })
}

/// Starts connecting a VPS: makes the keys of a tunnel to it and answers with
/// the one command to run there. The command counts for a quarter of an hour.
/// Core dials the VPS from now until then, and the tunnel is up within a few
/// seconds of the command being run; asking for the VPSes again shows it.
///
/// A VPS that was being connected and is not yet is given up for this one.
/// Those that are connected stay as they are.
#[utoipa::path(
    post,
    path = "/api/v1/gates",
    request_body = NewGate,
    responses(
        (status = CREATED, body = Network, description = "The command is ready, and the VPS is awaited."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "That VPS is connected already, or there is no room for one more."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The address, the name or the port will not do."),
    )
)]
async fn enrol_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(new): Json<NewGate>,
) -> Result<(StatusCode, Json<Network>), Problem> {
    let invalid = |sentence: &'static str| Err(Problem::Invalid(sentence.into()));
    let address = new.address.trim();
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    if address.is_empty() || address.len() > 253 || !address.chars().all(allowed) {
        return invalid("A VPS's address is an IPv4 address or a name.");
    }
    let name = named(new.name.as_deref(), address)?;
    let wg_port = new.wg_port.unwrap_or(WG_PORT);
    if wg_port == 0 {
        return invalid("A port is a number from 1 to 65535.");
    }
    {
        let _busy = state.tunnel.busy.lock().await;
        let gates = state.tunnel.gates().await?;
        let connected: Vec<&Gate> = gates.iter().filter(|gate| gate.connected()).collect();
        // A Gate that is in use is not replaced by a slip of the hand.
        if connected
            .iter()
            .any(|gate| gate.address.eq_ignore_ascii_case(address))
        {
            return Err(Problem::Conflict(
                "That VPS is connected already. Disconnect it before connecting it again.".into(),
            ));
        }
        let Some(number) =
            (0..TUNNELS).find(|number| connected.iter().all(|gate| gate.tunnel != *number))
        else {
            return Err(Problem::Conflict(
                format!("{TUNNELS} VPSes are connected, which is as many as there can be.").into(),
            ));
        };

        // A tunnel has addresses of its own. A machine that is already on a
        // network with the same ones would lose that network to the tunnel,
        // or the tunnel to it, and neither would say why.
        let (gate_address, home_address) = ends(number);
        let network = Ipv4Addr::from(u32::from(gate_address) & !3);
        if let Ok(Some(taken)) =
            tokio::task::spawn_blocking(move || network_in_the_way((network, 30))).await
        {
            return Err(Problem::Conflict(
                format!(
                    "This machine is already on a network that uses the addresses the tunnel needs ({network}/30): {taken}. A VPS cannot be connected until that network uses others."
                )
                .into(),
            ));
        }

        let (private_key, home_public_key) = new_keypair();
        // The Gate's first key. Home keeps the public half, and the VPS is
        // handed the private one inside the token.
        let (gate_private_key, gate_public_key) = new_keypair();
        let preshared_key = new_preshared_key();
        let token = auth::new_token();
        let now = auth::now();
        let join = JoinToken {
            private_key: gate_private_key,
            home_public_key,
            preshared_key: preshared_key.clone(),
            token: token.clone(),
            wg_port,
            api_port: API_PORT,
            gate_address,
            home_address,
            expires_at: (now + JOIN_SECONDS).unsigned_abs(),
        };
        // One VPS is connected at a time: what was begun and not finished
        // makes way, and its tunnel is taken down in the round that follows.
        sqlx::query("DELETE FROM gates WHERE join_token IS NOT NULL")
            .execute(&state.db)
            .await?;
        sqlx::query(
            "INSERT INTO gates
                 (tunnel, name, address, wg_port, api_port, private_key, gate_public_key,
                  preshared_key, token, join_token, join_expires_at, mode, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'transparent', ?)",
        )
        .bind(number)
        .bind(&name)
        .bind(address)
        .bind(wg_port)
        .bind(API_PORT)
        .bind(private_key)
        .bind(gate_public_key)
        .bind(preshared_key)
        .bind(token)
        .bind(join.encode())
        .bind(now + JOIN_SECONDS)
        .bind(now)
        .execute(&state.db)
        .await?;
    }
    state.tunnel.wake();
    audit::record(&state.db, &who, None, "gate.connect", address).await;
    Ok((StatusCode::CREATED, Json(view(&state).await?)))
}

/// Changes what a VPS is called.
#[utoipa::path(
    put,
    path = "/api/v1/gates/{id}",
    params(("id" = i64, Path, description = "The VPS's id.")),
    request_body = GateSettings,
    responses(
        (status = OK, body = Network),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such VPS."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The name will not do."),
    )
)]
async fn rename_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
    Json(settings): Json<GateSettings>,
) -> Result<Json<Network>, Problem> {
    let gates = state.tunnel.gates().await?;
    let gate = gates.iter().find(|gate| gate.id == id).ok_or(MISSING)?;
    let name = named(Some(&settings.name), &gate.address)?;
    sqlx::query("UPDATE gates SET name = ? WHERE id = ?")
        .bind(&name)
        .bind(id)
        .execute(&state.db)
        .await?;
    audit::record(&state.db, &who, None, "gate.rename", &name).await;
    Ok(Json(view(&state).await?))
}

/// Finds out again whether servers behind a VPS see their players' own
/// addresses, and puts its Gate in the mode that works. It takes a few
/// seconds, during which new connections through it may be turned away.
#[utoipa::path(
    post,
    path = "/api/v1/gates/{id}/check",
    params(("id" = i64, Path, description = "The VPS's id.")),
    responses(
        (status = OK, body = Network),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "The VPS is not connected, or it cannot be reached."),
    )
)]
async fn check_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
) -> Result<Json<Network>, Problem> {
    let name = {
        let tunnel = &state.tunnel;
        let _busy = tunnel.busy.lock().await;
        let gates = tunnel.gates().await?;
        let found = gates.into_iter().find(|gate| gate.id == id);
        let Some(gate) = found.filter(Gate::connected) else {
            return Err(Problem::Conflict("That VPS is not connected.".into()));
        };
        let checked = async {
            tunnel.set_up(&gate, &gate.keys).await?;
            let mode = tunnel.check(&gate).await?;
            tunnel.tell(&gate, mode).await
        };
        match checked.await {
            Ok((status, latency)) => {
                lock(&tunnel.live).entry(id).or_default().heard = Heard {
                    status: Some(status),
                    latency_ms: Some(latency),
                    problem: None,
                };
            }
            Err(error) => {
                return Err(Problem::Conflict(
                    format!("That could not be checked: {error:#}.").into(),
                ));
            }
        }
        gate.name
    };
    audit::record(&state.db, &who, None, "gate.check", &name).await;
    Ok(Json(view(&state).await?))
}

/// Forgets a VPS and takes home's end of the tunnel to it down. The servers
/// that were reached through it are reached through the best of the VPSes
/// that are left, or on the home network only if none is. The VPS keeps its
/// Gate program until `homewarp-gate leave` is run there.
#[utoipa::path(
    delete,
    path = "/api/v1/gates/{id}",
    params(("id" = i64, Path, description = "The VPS's id.")),
    responses(
        (status = NO_CONTENT, description = "The VPS is not connected any more."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = NOT_FOUND, body = ProblemBody, description = "There is no such VPS."),
    )
)]
async fn disconnect_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
    Path(id): Path<i64>,
) -> Result<StatusCode, Problem> {
    let tunnel = &state.tunnel;
    let address = {
        let _busy = tunnel.busy.lock().await;
        let gates = tunnel.gates().await?;
        let gate = gates.iter().find(|gate| gate.id == id).ok_or(MISSING)?;
        // The Gate is told to forward nothing more, if it can still be told.
        let up = lock(&tunnel.up).contains(&gate.tunnel);
        if up && gate.connected() {
            let nothing = Desired {
                generation: auth::now().unsigned_abs(),
                ..Desired::default()
            };
            let _ = ask::<Status, _>(gate, &gate.keys, "PUT", "/v1/state", Some(&nothing)).await;
        }
        // Its servers go to the best of the others, before it goes itself.
        let ports = servers::published(&state.db).await?;
        let others: Vec<Gate> = gates
            .iter()
            .filter(|other| other.id != id)
            .cloned()
            .collect();
        let weighed = tunnel.weighed(&others, &ports);
        let next = best(&weighed).or(weighed.first()).map(|gate| gate.id);
        sqlx::query("UPDATE servers SET gate_id = ? WHERE gate_id = ?")
            .bind(next)
            .bind(id)
            .execute(&state.db)
            .await?;
        sqlx::query("DELETE FROM gates WHERE id = ?")
            .bind(id)
            .execute(&state.db)
            .await?;
        lock(&tunnel.live).remove(&id);
        let left = others.iter().filter(|other| !other.expired(auth::now()));
        tunnel
            .clear(&left.map(|other| other.tunnel).collect())
            .await;
        gate.address.clone()
    };
    // The Gates that are left are told of the servers that came to them.
    tunnel.wake();
    audit::record(&state.db, &who, None, "gate.disconnect", &address).await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr},
        time::{Duration, Instant},
    };

    use homewarp_proto::{Desired, Forward, Protocol, Through};

    use super::{Live, Rate, SAMPLES, Tunnel, Weighed, best, ends, is_gate, lock};
    use crate::db;

    #[tokio::test]
    async fn what_a_gate_counted_is_added_up_for_the_ports_it_was_asked_to_forward_and_no_others() {
        let files = tempfile::tempdir().unwrap();
        let db = db::open(&files.path().join("homewarp.db")).await.unwrap();
        sqlx::query(
            "INSERT INTO gates (id, tunnel, name, address, wg_port, api_port, private_key,
                 gate_public_key, preshared_key, token, mode, created_at)
             VALUES (7, 0, 'a', 'a', 1, 1, 'k', 'k', 'k', 't', 'transparent', 0)",
        )
        .execute(&db)
        .await
        .unwrap();
        let tunnel = Tunnel::new(db.clone(), None, files.path());
        lock(&tunnel.live).entry(7).or_default().told = Some(Desired {
            generation: 1,
            forwards: vec![Forward {
                port: 25565,
                protocol: Protocol::Tcp,
            }],
            ..Desired::default()
        });
        let through = |port, bytes| Through {
            port,
            protocol: Protocol::Tcp,
            bytes,
        };
        // The first count of a port is where counting starts from.
        tunnel.count(7, &[through(25565, 1000)]).await.unwrap();
        // A Gate that says a great deal went through ports nobody asked it for.
        let mut said: Vec<Through> = (1..=2000).map(|port| through(port, 5000)).collect();
        said.push(through(25565, 1400));
        tunnel.count(7, &said).await.unwrap();
        tunnel.count(7, &said).await.unwrap();
        let rows: Vec<(i64, u16, i64)> = sqlx::query_as("SELECT gate_id, port, bytes FROM traffic")
            .fetch_all(&db)
            .await
            .unwrap();
        assert_eq!(rows, [(7, 25565, 400)]);
    }

    #[test]
    fn each_tunnel_has_two_ends_of_its_own_and_a_gates_end_is_known_for_one() {
        assert_eq!(
            ends(0),
            (Ipv4Addr::new(10, 213, 77, 1), Ipv4Addr::new(10, 213, 77, 2))
        );
        assert_eq!(
            ends(3),
            (
                Ipv4Addr::new(10, 213, 77, 13),
                Ipv4Addr::new(10, 213, 77, 14)
            )
        );
        assert!(is_gate(IpAddr::V4(Ipv4Addr::new(10, 213, 77, 1))));
        assert!(is_gate(IpAddr::V4(Ipv4Addr::new(10, 213, 77, 29))));
        // Home's own end, a ninth tunnel there is not, and a player.
        assert!(!is_gate(IpAddr::V4(Ipv4Addr::new(10, 213, 77, 2))));
        assert!(!is_gate(IpAddr::V4(Ipv4Addr::new(10, 213, 77, 33))));
        assert!(!is_gate(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9))));
    }

    #[test]
    fn what_passes_through_a_tunnel_is_what_the_kernel_counted_since_the_last_look() {
        let mut live = Live::default();
        let began = Instant::now();
        let after = |seconds| began + Duration::from_secs(seconds);
        // The first look has nothing before it, and no tunnel has nothing to count.
        live.sample(Some((1000, 500)), began);
        live.sample(Some((5000, 1500)), after(2));
        live.sample(None, after(4));
        live.sample(Some((10, 10)), after(6));
        // An interface that was made anew counts from nothing again.
        live.sample(Some((4, 4)), after(8));
        let rate = |received, sent| Rate { received, sent };
        assert_eq!(
            Vec::from(live.samples.clone()),
            [
                rate(0, 0),
                rate(2000, 500),
                rate(0, 0),
                rate(0, 0),
                rate(0, 0)
            ]
        );
        for look in 0..2 * SAMPLES as u64 {
            live.sample(Some((look, look)), after(10 + look));
        }
        assert_eq!(live.samples.len(), SAMPLES);
    }

    #[test]
    fn the_vps_a_server_is_best_reached_through_is_the_near_one_that_is_not_busy() {
        let gate = |id, latency_ms| Weighed {
            id,
            reachable: true,
            latency_ms: Some(latency_ms),
            hidden: false,
            load_percent: Some(0),
            servers: 0,
            bytes_per_second: 0,
        };
        let near = gate(1, 20);
        let far = gate(2, 90);
        assert_eq!(best(&[far.clone(), near.clone()]).unwrap().id, 1);
        // Near and worked hard loses to a little further and idle.
        let busy = Weighed {
            load_percent: Some(180),
            servers: 4,
            ..near.clone()
        };
        assert_eq!(best(&[busy, far.clone()]).unwrap().id, 2);
        // One that hides players' addresses is taken only if it is all there is.
        let hides = Weighed {
            hidden: true,
            ..near.clone()
        };
        assert_eq!(best(&[hides.clone(), far.clone()]).unwrap().id, 2);
        assert_eq!(best(std::slice::from_ref(&hides)).unwrap().id, 1);
        // One that does not answer is not recommended, however near it was.
        let down = Weighed {
            reachable: false,
            ..near.clone()
        };
        assert_eq!(best(&[down.clone(), far]).unwrap().id, 2);
        assert_eq!(best(&[down]), None);
        // Even, the one that was connected first.
        assert_eq!(best(&[gate(5, 30), gate(3, 30)]).unwrap().id, 3);
        assert_eq!(best(&[]), None);
        assert_eq!(
            Weighed {
                load_percent: Some(12),
                servers: 1,
                ..near
            }
            .why(),
            "20 ms from home, 12 % busy, 1 server on it"
        );
    }
}
