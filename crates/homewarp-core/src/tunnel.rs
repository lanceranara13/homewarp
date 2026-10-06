//! Core's end of the tunnel (PLAN.md §5.3 to §5.5): the interface and the way
//! back at home, enrolling a VPS, and telling its Gate what to forward.
//!
//! One task does all of it, every few seconds: sets the kernel up as the Gate
//! in the database needs it, sees a new Gate through to keys of its own, tells
//! the Gate which ports to forward if that has changed, and otherwise asks how
//! it is. The asking is not only for show. It is traffic that goes unanswered
//! when the Gate has lost its end of the tunnel, and that is what makes
//! WireGuard at home shake hands again within seconds and not minutes. And
//! the setting up is done every time round, because whatever is undone
//! between two rounds, by a reboot or by another program, has to come back.

use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, bail};
use axum::{Json, extract::State, http::StatusCode};
use homewarp_net::{
    INTERFACE, Link, apply, bring_up, has_keep_table, has_table, home_ruleset, keep_ruleset,
    network_in_the_way, new_keypair, new_preshared_key, probe_packets, route_replies, take_down,
};
use homewarp_proto::{
    Answer, Answering, Desired, Forward, Guard, JoinToken, Mode, Open, Probe, ProbeRequest,
    Protocol, Rotate, Rotated, Status, Through,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sqlx::SqlitePool;
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
    servers::{self, PortProtocol},
    settings,
};

/// The two ends' addresses inside the tunnel (PLAN.md §5.3, Addressing).
pub(crate) const GATE: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 1);
const HOME: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 2);
/// The bridge servers sit on, which `runtime` makes.
const BRIDGE: &str = "homewarp-br";
/// Where a Gate's WireGuard listens unless another port is asked for.
const WG_PORT: u16 = 51820;
/// Where a Gate answers Core, on its tunnel address.
const API_PORT: u16 = 4857;
const EVERY: Duration = Duration::from_secs(10);
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

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(get_gate, enrol_gate, disconnect_gate))
        .routes(routes!(check_gate))
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

/// The Gate as the database has it.
#[derive(Clone)]
struct Gate {
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

/// What the Gate last said, and how long the way to it took.
#[derive(Clone, Default)]
struct Heard {
    status: Option<Status>,
    latency_ms: Option<u32>,
    /// Why it could not be asked, when it could not.
    problem: Option<String>,
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

/// Core's end of the tunnel, and what it last heard from the other.
pub(crate) struct Tunnel {
    db: SqlitePool,
    /// What runs servers, and for the probe a listener where a server would be.
    runtime: Option<Arc<Runtime>>,
    /// One thing at a time is done to the tunnel: a round of keeping it, or
    /// something a page asked for.
    busy: tokio::sync::Mutex<()>,
    /// Rung when something has changed that should not wait for the next round.
    wake: Notify,
    heard: Mutex<Heard>,
    /// Which Gate, by which of its keys, the kernel was last set up for. The
    /// table is replaced once for each and after that only if it has gone.
    up_for: Mutex<Option<String>>,
    /// What the Gate was last told.
    told: Mutex<Option<Desired>>,
    /// Set when a Gate has just been enrolled: the self-probe is due.
    check_due: AtomicBool,
    /// What the Gate had counted through each port when it was last heard from.
    counted: Mutex<BTreeMap<(u16, Protocol), u64>>,
    /// Set once it has been said that servers could not be kept from the home network.
    keep_failed: AtomicBool,
    /// The port the panel is served on over TLS, here and on the VPS alike.
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
    pub(crate) fn new(db: SqlitePool, runtime: Option<Arc<Runtime>>) -> Arc<Self> {
        Arc::new(Self {
            db,
            runtime,
            busy: tokio::sync::Mutex::default(),
            wake: Notify::new(),
            heard: Mutex::default(),
            up_for: Mutex::default(),
            told: Mutex::default(),
            check_due: AtomicBool::new(false),
            counted: Mutex::default(),
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

    /// The Gate, where one is connected and has keys of its own.
    async fn connected(&self) -> anyhow::Result<Gate> {
        self.gate()
            .await?
            .filter(|gate| gate.join.is_none())
            .context("No VPS is connected")
    }

    /// Asks the Gate how the guard on the VPS itself stands, or to change it.
    pub(crate) async fn guard(
        &self,
        method: &str,
        path: &str,
        open: Option<&Open>,
    ) -> anyhow::Result<Guard> {
        let gate = self.connected().await?;
        match ask(&gate, &gate.keys, method, path, open).await {
            Ok((guard, _)) => Ok(guard),
            Err(Unanswered::Refused(404 | 405, _)) => bail!(
                "The Gate on the VPS is older than this Homewarp and has no guard. Put the new one there"
            ),
            Err(error) => Err(error.into()),
        }
    }

    /// Has the Gate put the answer to a certificate authority's question
    /// where the VPS's port 80 serves it.
    pub(crate) async fn answer(&self, token: &str, answer: &str) -> anyhow::Result<Answering> {
        let gate = self.connected().await.context(
            "The panel's name leads to a VPS, where a certificate's question is answered",
        )?;
        let asked = Answer {
            answer: answer.to_owned(),
        };
        let path = format!("/v1/challenge/{token}");
        match ask(&gate, &gate.keys, "PUT", &path, Some(&asked)).await {
            Ok((answering, _)) => Ok(answering),
            Err(Unanswered::Refused(404 | 405, _)) => bail!(
                "The Gate on the VPS is older than this Homewarp and cannot answer for a certificate. Put the new one there"
            ),
            Err(error) => Err(error.into()),
        }
    }

    /// Has the Gate take such an answer away again.
    pub(crate) async fn unanswer(&self, token: &str) -> anyhow::Result<()> {
        let gate = self.connected().await?;
        let path = format!("/v1/challenge/{token}");
        ask::<(), ()>(&gate, &gate.keys, "DELETE", &path, None).await?;
        Ok(())
    }

    /// Has the next round begin now: a server's ports have changed, or a VPS
    /// is about to be enrolled.
    pub(crate) fn wake(&self) {
        self.wake.notify_one();
    }

    /// Keeps the tunnel as the database says it should be, for as long as Core runs.
    pub(crate) fn keep(self: &Arc<Self>) {
        let tunnel = Arc::clone(self);
        tokio::spawn(async move {
            // An interface left by a Core that was stopped before it could
            // take it down, with no Gate in the database to account for it.
            *lock(&tunnel.up_for) = Path::new("/sys/class/net")
                .join(INTERFACE)
                .exists()
                .then(String::new);
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
    }

    /// One round. True while a VPS that was handed a join token is awaited.
    async fn round(&self) -> bool {
        let _busy = self.busy.lock().await;
        // Servers are kept from the home network whether or not there is a
        // VPS. Looked at every round: a firewall that restarts takes the
        // table with it, as it takes the tunnel's.
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
        let gate = match self.gate().await {
            Ok(Some(gate)) => gate,
            Ok(None) => {
                self.take_down().await;
                return false;
            }
            Err(error) => {
                lock(&self.heard).problem = Some(format!("{error:#}"));
                return false;
            }
        };
        let waiting = gate.join.is_some();
        // A join token that has run out: home stops dialling with the key it carried.
        if gate
            .join
            .as_ref()
            .is_some_and(|(_, until)| *until <= auth::now())
        {
            self.take_down().await;
            return false;
        }
        let heard = match self.tend(gate).await {
            Ok((status, latency)) => {
                if let Err(error) = self.count(&status.traffic).await {
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
        *lock(&self.heard) = heard;
        waiting
    }

    /// Adds what has gone through each port since the Gate was last heard
    /// from to the count of the hour it is now.
    ///
    /// The Gate counts from when it last set its rules, so a count that has
    /// gone down has started again and is all new. The first count this Core
    /// sees of a port is where it counts on from, and is not added: how much
    /// of it went through while no Core was listening is not known.
    async fn count(&self, traffic: &[Through]) -> anyhow::Result<()> {
        let hour = auth::now() / 3600;
        // Only the ports this Core asked to have forwarded. A Gate is not
        // taken at its word for which those are: one that named every port
        // there is would be given a row for each, every hour.
        let asked: Vec<(u16, Protocol)> = lock(&self.told)
            .as_ref()
            .map(|told| {
                let forwards = told.forwards.iter();
                forwards
                    .map(|forward| (forward.port, forward.protocol))
                    .collect()
            })
            .unwrap_or_default();
        let more: Vec<(u16, &str, u64)> = {
            let mut counted = lock(&self.counted);
            traffic
                .iter()
                .filter(|now| asked.contains(&(now.port, now.protocol)))
                .filter_map(|now| {
                    let more = match counted.insert((now.port, now.protocol), now.bytes) {
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
                "INSERT INTO traffic (port, protocol, hour, bytes) VALUES (?, ?, ?, ?)
                 ON CONFLICT (port, protocol, hour) DO UPDATE SET bytes = bytes + excluded.bytes",
            )
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

    /// Takes home's end out of the kernel, if this Core put it there.
    async fn take_down(&self) {
        *lock(&self.heard) = Heard::default();
        *lock(&self.told) = None;
        if lock(&self.up_for).take().is_some() {
            let _ = tokio::task::spawn_blocking(take_down).await;
        }
    }

    async fn gate(&self) -> anyhow::Result<Option<Gate>> {
        type Text = Option<String>;
        type Row = (
            String,
            i64,
            i64,
            String,
            String,
            String,
            String,
            Text,
            Text,
            Text,
            Text,
            Option<i64>,
            String,
            String,
            Text,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT address, wg_port, api_port, private_key, gate_public_key, preshared_key, token,
                    next_public_key, next_preshared_key, next_token, join_token, join_expires_at,
                    mode, checked, note
             FROM gate WHERE id = 1",
        )
        .fetch_optional(&self.db)
        .await?;
        let Some((
            address,
            wg_port,
            api_port,
            private_key,
            gate_public_key,
            preshared_key,
            token,
            next_public_key,
            next_preshared_key,
            next_token,
            join_token,
            join_expires_at,
            mode,
            checked,
            note,
        )) = row
        else {
            return Ok(None);
        };
        let next = match (next_public_key, next_preshared_key, next_token) {
            (Some(gate_public_key), Some(preshared_key), Some(token)) => Some(Keys {
                gate_public_key,
                preshared_key,
                token,
            }),
            _ => None,
        };
        Ok(Some(Gate {
            address,
            wg_port: wg_port.try_into()?,
            api_port: api_port.try_into()?,
            private_key,
            keys: Keys {
                gate_public_key,
                preshared_key,
                token,
            },
            next,
            join: join_token.zip(join_expires_at),
            mode: match mode.as_str() {
                "nat" => Mode::Nat,
                _ => Mode::Transparent,
            },
            checked: match checked.as_str() {
                "preserved" => PlayerAddresses::Preserved,
                "hidden" => PlayerAddresses::Hidden,
                _ => PlayerAddresses::Unchecked,
            },
            note,
        }))
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
        if self.check_due.swap(false, Ordering::Relaxed) {
            match self.check(&gate).await {
                Ok(mode) => heard = self.tell(&gate, mode).await?,
                // The tunnel is up all the same. What could not be found out
                // is said where its answer would have been, and the page has
                // a button to try again.
                Err(error) => {
                    sqlx::query("UPDATE gate SET note = ?")
                        .bind(format!("The check could not be made: {error:#}."))
                        .execute(&self.db)
                        .await?;
                }
            }
        }
        Ok(heard)
    }

    /// Home's end in the kernel: the interface, dialling the Gate; the way
    /// back for replies; and the rules. Done again it changes nothing, and it
    /// is done every round.
    async fn set_up(&self, gate: &Gate, keys: &Keys) -> anyhow::Result<()> {
        let endpoint = find(&gate.address, gate.wg_port).await?;
        let link = Link {
            private_key: gate.private_key.clone(),
            listen_port: 0,
            address: HOME,
            peer_public_key: keys.gate_public_key.clone(),
            preshared_key: keys.preshared_key.clone(),
            // Players come from anywhere, and their packets come in by this link.
            peer_allowed: (Ipv4Addr::UNSPECIFIED, 0),
            peer_endpoint: Some(endpoint),
        };
        // The rules differ by whether the panel has a name: its port is let
        // in from the tunnel only then.
        let panel = self.panel().await?;
        let which = format!("{endpoint} {} {panel:?}", keys.gate_public_key);
        let again = lock(&self.up_for).as_deref() == Some(which.as_str());
        blocking(move || {
            bring_up(&link)?;
            route_replies()?;
            // Replaced once for each Gate, so that a newer Core's rules take
            // the place of an older one's, and put back if they have gone.
            if !again || !has_table() {
                apply(&home_ruleset(BRIDGE, None, panel)?)?;
            }
            Ok(())
        })
        .await?;
        if !again {
            *lock(&self.up_for) = Some(which);
            *lock(&self.told) = None;
        }
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
                    "UPDATE gate SET next_public_key = ?, next_preshared_key = ?, next_token = ?",
                )
                .bind(&next.gate_public_key)
                .bind(&next.preshared_key)
                .bind(&next.token)
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
                    "UPDATE gate
                     SET next_public_key = NULL, next_preshared_key = NULL, next_token = NULL",
                )
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
            "UPDATE gate
             SET gate_public_key = ?, preshared_key = ?, token = ?,
                 next_public_key = NULL, next_preshared_key = NULL, next_token = NULL,
                 join_token = NULL, join_expires_at = NULL",
        )
        .bind(&next.gate_public_key)
        .bind(&next.preshared_key)
        .bind(&next.token)
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
        self.check_due.store(true, Ordering::Relaxed);
        Ok(gate)
    }

    /// Tells the Gate what to forward and in which mode, if it has not been
    /// told just that already, and asks how it is if it has.
    async fn tell(&self, gate: &Gate, mode: Mode) -> anyhow::Result<(Status, u32)> {
        let mut forwards = Vec::new();
        for published in servers::published(&self.db).await? {
            for protocol in published.protocol.each() {
                forwards.push(Forward {
                    port: published.port,
                    protocol: *protocol,
                });
            }
        }
        // And the panel's own, as one more: where its door is, at home.
        if let Some(port) = self.panel().await? {
            forwards.push(Forward {
                port,
                protocol: Protocol::Tcp,
            });
        }
        let rate = Some(settings::new_connections(&self.db).await?);
        let told = lock(&self.told).clone();
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
        *lock(&self.told) = Some(desired);
        Ok(heard)
    }

    /// The self-probe (PLAN.md §5.3): finds out whether a server at home sees
    /// its players' own addresses, falls back to the Gate standing in for them
    /// if replies cannot be sent back otherwise, and writes down which it is.
    /// Returns the mode the Gate is to be in.
    async fn check(&self, gate: &Gate) -> anyhow::Result<Mode> {
        self.tell(gate, Mode::Transparent).await?;
        let (mode, checked, note) = match self.probe(gate).await? {
            Seen::From(from) if from != IpAddr::V4(GATE) => {
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
        sqlx::query("UPDATE gate SET mode = ?, checked = ?, note = ?")
            .bind(match mode {
                Mode::Transparent => "transparent",
                Mode::Nat => "nat",
            })
            .bind(checked.as_str())
            .bind(note)
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
        let panel = self.panel().await?;
        blocking(move || apply(&home_ruleset(BRIDGE, Some(port), panel)?)).await?;
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
        blocking(move || apply(&home_ruleset(BRIDGE, None, panel)?)).await?;
        listened?;
        Ok(match (arrived?, packets?) {
            (Some(from), _) => Seen::From(from),
            (None, 0) => Seen::Nothing,
            (None, _) => Seen::NoWayBack,
        })
    }
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

/// One request to the Gate, through the tunnel, and how many milliseconds the
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
        let mut stream = TcpStream::connect(SocketAddr::from((GATE, gate.api_port))).await?;
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
    /// No VPS is connected. Servers are reached on the home network only.
    None,
    /// A VPS has been handed its command, and has not been heard from yet.
    Waiting,
    /// The command was not run in time. It takes a new one.
    Expired,
    /// A Gate is enrolled. Whether it answers just now is `reachable`.
    Connected,
}

/// A port of a server, which a connected Gate forwards.
#[derive(Serialize, ToSchema)]
struct ForwardedPort {
    port: u16,
    protocol: PortProtocol,
    server_id: i64,
    /// The server's name.
    server: String,
    /// What the Gate counted through it in the last day, both ways together.
    /// Nothing where no Gate has counted.
    traffic_bytes: i64,
}

/// What went through each port in the last day, as the hours of it add up.
async fn through(db: &SqlitePool) -> anyhow::Result<HashMap<u16, i64>> {
    let counted: Vec<(u16, i64)> =
        sqlx::query_as("SELECT port, SUM(bytes) FROM traffic WHERE hour > ? GROUP BY port")
            .bind(auth::now() / 3600 - 24)
            .fetch_all(db)
            .await?;
    Ok(counted.into_iter().collect())
}

/// The Gate, and how the tunnel to it is doing.
#[derive(Serialize, ToSchema)]
struct GateView {
    state: GateState,
    /// The VPS's public address: what players type.
    address: Option<String>,
    /// While a VPS is awaited: what to run on it, as root. Where this
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
    /// Through the tunnel since the Gate last set it up, as the Gate counts.
    received_bytes: u64,
    sent_bytes: u64,
    /// Every port of every server: what a connected Gate forwards.
    ports: Vec<ForwardedPort>,
}

/// What it takes to start connecting a VPS.
#[derive(Deserialize, ToSchema)]
struct NewGate {
    /// The VPS's public IPv4 address, or a name for it.
    address: String,
    /// The UDP port its WireGuard is to listen on. 51820 if none is given.
    wg_port: Option<u16>,
}

async fn view(state: &AppState) -> Result<GateView, Problem> {
    // Neither read depends on the other, so neither waits for the other.
    let (gate, ports, through) = tokio::try_join!(
        state.tunnel.gate(),
        servers::published(&state.db),
        through(&state.db)
    )?;
    let heard = lock(&state.tunnel.heard).clone();
    let now = auth::now();
    let ports = ports
        .into_iter()
        .map(|published| ForwardedPort {
            port: published.port,
            protocol: published.protocol,
            server_id: published.server_id,
            server: published.server,
            traffic_bytes: through.get(&published.port).copied().unwrap_or(0),
        })
        .collect();
    let status = heard.status;
    Ok(GateView {
        state: match &gate {
            None => GateState::None,
            Some(gate) => match &gate.join {
                Some((_, until)) if *until <= now => GateState::Expired,
                Some(_) => GateState::Waiting,
                None => GateState::Connected,
            },
        },
        command: gate
            .as_ref()
            .and_then(|gate| gate.join.as_ref())
            .filter(|(_, until)| *until > now)
            .map(|(token, _)| match &state.releases {
                // One line that fetches the Gate, checks it and enrols the VPS.
                Some(releases) => {
                    format!("curl -fsSL {releases}/install-gate.sh | sh -s -- {token}")
                }
                // The Gate is taken to be on the VPS already.
                None => format!("homewarp-gate join {token}"),
            }),
        expires_at: gate
            .as_ref()
            .and_then(|gate| gate.join.as_ref())
            .map(|(_, until)| *until),
        reachable: status.is_some(),
        latency_ms: heard.latency_ms,
        handshake_age_seconds: status
            .as_ref()
            .and_then(|status| status.handshake_age_seconds),
        player_addresses: gate
            .as_ref()
            .map_or(PlayerAddresses::Unchecked, |gate| gate.checked),
        note: gate.as_ref().and_then(|gate| gate.note.clone()),
        problem: heard.problem,
        version: status.as_ref().map(|status| status.version.clone()),
        received_bytes: status.as_ref().map_or(0, |status| status.received_bytes),
        sent_bytes: status.as_ref().map_or(0, |status| status.sent_bytes),
        address: gate.map(|gate| gate.address),
        ports,
    })
}

/// The Gate, how the tunnel to it is doing, and every port it forwards. The
/// one request the Network page needs, and the one the Gate's pill asks again.
#[utoipa::path(
    get,
    path = "/api/v1/gate",
    responses(
        (status = OK, body = GateView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn get_gate(
    State(state): State<AppState>,
    SignedIn(who): SignedIn,
) -> Result<Json<GateView>, Problem> {
    let mut view = view(&state).await?;
    // The command enrols a VPS, and the ports are every server's: both are the
    // owner's. Any account may know where players reach the servers it is in.
    if !who.owner {
        view.command = None;
        view.ports.clear();
    }
    Ok(Json(view))
}

/// Starts connecting a VPS: makes the keys of a tunnel to it and answers with
/// the one command to run there. The command counts for a quarter of an hour.
/// Core dials the VPS from now until then, and the tunnel is up within a few
/// seconds of the command being run; asking for the Gate again shows it.
#[utoipa::path(
    post,
    path = "/api/v1/gate",
    request_body = NewGate,
    responses(
        (status = CREATED, body = GateView, description = "The command is ready, and the VPS is awaited."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "A Gate is connected already."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "The address or the port will not do."),
    )
)]
async fn enrol_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
    Json(new): Json<NewGate>,
) -> Result<(StatusCode, Json<GateView>), Problem> {
    let invalid = |sentence: &'static str| Err(Problem::Invalid(sentence.into()));
    let address = new.address.trim();
    let named = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    if address.is_empty() || address.len() > 253 || !address.chars().all(named) {
        return invalid("A VPS's address is an IPv4 address or a name.");
    }
    let wg_port = new.wg_port.unwrap_or(WG_PORT);
    if wg_port == 0 {
        return invalid("A port is a number from 1 to 65535.");
    }
    // A Gate that is in use is not replaced by a slip of the hand.
    if state
        .tunnel
        .gate()
        .await?
        .is_some_and(|gate| gate.join.is_none())
    {
        return Err(Problem::Conflict(
            "A VPS is connected already. Disconnect it before connecting another.".into(),
        ));
    }

    // The tunnel has addresses of its own. A machine that is already on a
    // network with the same ones would lose that network to the tunnel, or
    // the tunnel to it, and neither would say why.
    let tunnel = Ipv4Addr::from(u32::from(GATE) & !3);
    if let Ok(Some(taken)) =
        tokio::task::spawn_blocking(move || network_in_the_way((tunnel, 30))).await
    {
        return Err(Problem::Conflict(
            format!(
                "This machine is already on a network that uses the addresses the tunnel needs ({tunnel}/30): {taken}. A VPS cannot be connected until that network uses others."
            )
            .into(),
        ));
    }

    let (private_key, home_public_key) = new_keypair();
    // The Gate's first key. Home keeps the public half, and the VPS is handed
    // the private one inside the token.
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
        gate_address: GATE,
        home_address: HOME,
        expires_at: (now + JOIN_SECONDS).unsigned_abs(),
    };
    {
        let _busy = state.tunnel.busy.lock().await;
        sqlx::query(
            "INSERT OR REPLACE INTO gate
                 (id, address, wg_port, api_port, private_key, gate_public_key, preshared_key,
                  token, join_token, join_expires_at, mode, created_at)
             VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'transparent', ?)",
        )
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
        *lock(&state.tunnel.heard) = Heard::default();
    }
    state.tunnel.wake();
    audit::record(&state.db, &who, None, "gate.connect", address).await;
    Ok((StatusCode::CREATED, Json(view(&state).await?)))
}

/// Finds out again whether servers see their players' own addresses, and puts
/// the Gate in the mode that works. It takes a few seconds, during which new
/// connections may be turned away.
#[utoipa::path(
    post,
    path = "/api/v1/gate/check",
    responses(
        (status = OK, body = GateView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = CONFLICT, body = ProblemBody, description = "No Gate is connected, or it cannot be reached."),
    )
)]
async fn check_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<Json<GateView>, Problem> {
    {
        let tunnel = &state.tunnel;
        let _busy = tunnel.busy.lock().await;
        let Some(gate) = tunnel.gate().await?.filter(|gate| gate.join.is_none()) else {
            return Err(Problem::Conflict("No VPS is connected.".into()));
        };
        let checked = async {
            tunnel.set_up(&gate, &gate.keys).await?;
            let mode = tunnel.check(&gate).await?;
            tunnel.tell(&gate, mode).await
        };
        match checked.await {
            Ok((status, latency)) => {
                *lock(&tunnel.heard) = Heard {
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
    }
    audit::record(&state.db, &who, None, "gate.check", "").await;
    Ok(Json(view(&state).await?))
}

/// Forgets the Gate and takes home's end of the tunnel down. Servers go back
/// to being reached on the home network only. The VPS keeps its Gate program
/// until `homewarp-gate leave` is run there.
#[utoipa::path(
    delete,
    path = "/api/v1/gate",
    responses(
        (status = NO_CONTENT, description = "There is no Gate now."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn disconnect_gate(
    State(state): State<AppState>,
    Owner(who): Owner,
) -> Result<StatusCode, Problem> {
    let tunnel = &state.tunnel;
    let _busy = tunnel.busy.lock().await;
    // The Gate is told to forward nothing more, if it can still be told.
    let up = lock(&tunnel.up_for).is_some();
    if let Some(gate) = tunnel
        .gate()
        .await?
        .filter(|gate| up && gate.join.is_none())
    {
        let nothing = Desired {
            generation: auth::now().unsigned_abs(),
            ..Desired::default()
        };
        let _ = ask::<Status, _>(&gate, &gate.keys, "PUT", "/v1/state", Some(&nothing)).await;
    }
    sqlx::query("DELETE FROM gate").execute(&state.db).await?;
    tunnel.take_down().await;
    audit::record(&state.db, &who, None, "gate.disconnect", "").await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use homewarp_proto::{Desired, Forward, Protocol, Through};

    use super::{Tunnel, lock};
    use crate::db;

    #[tokio::test]
    async fn what_a_gate_counted_is_added_up_for_the_ports_it_was_asked_to_forward_and_no_others() {
        let files = tempfile::tempdir().unwrap();
        let db = db::open(&files.path().join("homewarp.db")).await.unwrap();
        let tunnel = Tunnel::new(db.clone(), None);
        *lock(&tunnel.told) = Some(Desired {
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
        tunnel.count(&[through(25565, 1000)]).await.unwrap();
        // A Gate that says a great deal went through ports nobody asked it for.
        let mut said: Vec<Through> = (1..=2000).map(|port| through(port, 5000)).collect();
        said.push(through(25565, 1400));
        tunnel.count(&said).await.unwrap();
        tunnel.count(&said).await.unwrap();
        let rows: Vec<(u16, i64)> = sqlx::query_as("SELECT port, bytes FROM traffic")
            .fetch_all(&db)
            .await
            .unwrap();
        assert_eq!(rows, [(25565, 400)]);
    }
}
