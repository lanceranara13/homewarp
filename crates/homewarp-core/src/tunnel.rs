//! Core's end of the tunnel (PLAN.md §5.3 and §5.4): the interface and the way
//! back at home, and telling the Gate what to forward.
//!
//! One task does all of it, every few seconds: sets the kernel up if the Gate
//! in the database is not the one it was set up for, tells the Gate which
//! ports to forward if that has changed, and otherwise asks how it is. The
//! asking is not only for show. It is traffic that goes unanswered when the
//! Gate has lost its end of the tunnel, and that is what makes WireGuard at
//! home shake hands again within seconds and not minutes.

use std::{
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::{Context, bail};
use axum::{Json, extract::State, http::StatusCode};
use homewarp_net::{Link, apply, bring_up, home_ruleset, route_replies, take_down};
use homewarp_proto::{Desired, Forward, Mode, Protocol, Status};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    api::{AppState, Problem, ProblemBody, SignedIn},
    auth,
};

/// The two ends' addresses inside the tunnel (PLAN.md §5.3, Addressing).
const GATE: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 1);
const HOME: Ipv4Addr = Ipv4Addr::new(10, 213, 77, 2);
/// The bridge servers sit on, which `runtime` makes.
const BRIDGE: &str = "homewarp-br";
const EVERY: Duration = Duration::from_secs(10);

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(get_gate, connect_gate, disconnect_gate))
}

/// The Gate as the database has it.
#[derive(Clone)]
struct Gate {
    address: String,
    wg_port: u16,
    private_key: String,
    gate_public_key: String,
    preshared_key: String,
    token: String,
    api_port: u16,
    mode: Mode,
}

/// Core's end of the tunnel, and what it last heard from the other.
pub(crate) struct Tunnel {
    db: SqlitePool,
    /// What the Gate last said, or why it could not be asked.
    heard: Mutex<Option<Result<Status, String>>>,
    /// Which Gate the kernel was last set up for: it is set up once for each,
    /// and not every time round.
    up_for: Mutex<Option<String>>,
    /// What the Gate was last told.
    told: Mutex<Option<Desired>>,
}

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Tunnel {
    pub(crate) fn new(db: SqlitePool) -> Arc<Self> {
        Arc::new(Self {
            db,
            heard: Mutex::default(),
            up_for: Mutex::default(),
            told: Mutex::default(),
        })
    }

    /// Keeps the tunnel as the database says it should be, for as long as Core runs.
    pub(crate) fn keep(self: &Arc<Self>) {
        let tunnel = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                let heard = match tunnel.gate().await {
                    Ok(Some(gate)) => Some(
                        tunnel
                            .tend(&gate)
                            .await
                            .map_err(|error| format!("{error:#}")),
                    ),
                    Ok(None) => None,
                    Err(error) => Some(Err(format!("{error:#}"))),
                };
                *lock(&tunnel.heard) = heard;
                tokio::time::sleep(EVERY).await;
            }
        });
    }

    async fn gate(&self) -> anyhow::Result<Option<Gate>> {
        type Row = (String, i64, String, String, String, String, i64, String);
        let row: Option<Row> = sqlx::query_as(
            "SELECT address, wg_port, private_key, gate_public_key, preshared_key, token, api_port, mode
             FROM gate WHERE id = 1",
        )
        .fetch_optional(&self.db)
        .await?;
        let Some((
            address,
            wg_port,
            private_key,
            gate_public_key,
            preshared_key,
            token,
            api_port,
            mode,
        )) = row
        else {
            return Ok(None);
        };
        Ok(Some(Gate {
            address,
            wg_port: wg_port.try_into()?,
            private_key,
            gate_public_key,
            preshared_key,
            token,
            api_port: api_port.try_into()?,
            mode: if mode == "nat" {
                Mode::Nat
            } else {
                Mode::Transparent
            },
        }))
    }

    /// One round: the kernel, then the Gate.
    async fn tend(&self, gate: &Gate) -> anyhow::Result<Status> {
        let which = format!("{}:{} {}", gate.address, gate.wg_port, gate.gate_public_key);
        if lock(&self.up_for).as_deref() != Some(which.as_str()) {
            let link = gate.clone();
            tokio::task::spawn_blocking(move || set_up(&link)).await??;
            *lock(&self.up_for) = Some(which);
            *lock(&self.told) = None;
        }

        // An egg does not say which protocol its port speaks, so both are forwarded.
        let ports: Vec<i64> = sqlx::query_scalar("SELECT port FROM servers ORDER BY port")
            .fetch_all(&self.db)
            .await?;
        let mut forwards = Vec::with_capacity(ports.len() * 2);
        for port in ports {
            for protocol in [Protocol::Tcp, Protocol::Udp] {
                forwards.push(Forward {
                    port: port.try_into()?,
                    protocol,
                });
            }
        }
        let told = lock(&self.told).clone();
        if let Some(told) = told.filter(|told| told.forwards == forwards && told.mode == gate.mode)
        {
            let status = ask(gate, "GET", "/v1/status", &[]).await?;
            // A Gate that has forgotten what it was told, a new VPS say, is told again.
            if status.generation == told.generation {
                return Ok(status);
            }
        }
        let desired = Desired {
            generation: auth::now().unsigned_abs(),
            mode: gate.mode,
            forwards,
        };
        let status = ask(gate, "PUT", "/v1/state", &serde_json::to_vec(&desired)?).await?;
        *lock(&self.told) = Some(desired);
        Ok(status)
    }
}

/// Home's end in the kernel: the interface, dialling the Gate; the way back
/// for replies; and the rules. Done again it changes nothing.
fn set_up(gate: &Gate) -> anyhow::Result<()> {
    let endpoint = (gate.address.as_str(), gate.wg_port)
        .to_socket_addrs()
        .with_context(|| format!("looking up {}", gate.address))?
        .find(SocketAddr::is_ipv4)
        .with_context(|| format!("{} has no IPv4 address", gate.address))?;
    bring_up(&Link {
        private_key: gate.private_key.clone(),
        listen_port: 0,
        address: HOME,
        peer_public_key: gate.gate_public_key.clone(),
        preshared_key: gate.preshared_key.clone(),
        // Players come from anywhere, and their packets come in by this link.
        peer_allowed: (Ipv4Addr::UNSPECIFIED, 0),
        peer_endpoint: Some(endpoint),
    })?;
    route_replies()?;
    apply(&home_ruleset(BRIDGE)?)?;
    Ok(())
}

/// One request to the Gate, through the tunnel. Its API is a handful of small
/// JSON answers from a program of ours, so this speaks just enough HTTP for
/// that and brings no client library with it.
async fn ask(gate: &Gate, method: &str, path: &str, body: &[u8]) -> anyhow::Result<Status> {
    let talk = async {
        let mut stream = TcpStream::connect(SocketAddr::from((GATE, gate.api_port))).await?;
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: gate\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            gate.token,
            body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body).await?;
        let mut answer = Vec::new();
        stream.take(1 << 20).read_to_end(&mut answer).await?;
        anyhow::Ok(answer)
    };
    let answer = timeout(Duration::from_secs(5), talk)
        .await
        .context("the Gate did not answer through the tunnel")?
        .context("the Gate could not be reached through the tunnel")?;
    let answer = String::from_utf8_lossy(&answer);
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .context("what the Gate answered was not HTTP")?;
    if !head.starts_with("HTTP/1.1 200") {
        bail!("the Gate refused: {}", body.trim());
    }
    serde_json::from_str(body).context("reading the Gate's answer")
}

/// The Gate as the Network page shows it.
#[derive(Serialize, ToSchema)]
struct GateView {
    /// Whether a Gate has been connected at all.
    connected: bool,
    /// The VPS's public address: what players type.
    address: Option<String>,
    /// Whether servers see their players' own addresses, and not the Gate's.
    transparent: bool,
    /// How long ago the Gate was heard from over the tunnel. None if it never was.
    handshake_age_seconds: Option<u64>,
    /// How many ports the Gate forwards.
    forwards: usize,
    /// Why the Gate could not be reached, when it could not.
    problem: Option<String>,
}

/// What it takes to reach a Gate that is already running.
#[derive(Deserialize, ToSchema)]
struct GateSettings {
    /// The VPS's public address or name.
    address: String,
    wg_port: u16,
    /// Home's own key for the tunnel, the public half of which the Gate has.
    private_key: String,
    gate_public_key: String,
    preshared_key: String,
    token: String,
    api_port: u16,
    /// Have servers see the Gate's address and not their players' own: for a
    /// home where the way back through the tunnel cannot be made to work.
    #[serde(default)]
    nat: bool,
}

/// The Gate, and how the tunnel to it is doing.
#[utoipa::path(
    get,
    path = "/api/v1/gate",
    responses(
        (status = OK, body = GateView),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
    )
)]
async fn get_gate(State(state): State<AppState>, _: SignedIn) -> Result<Json<GateView>, Problem> {
    let gate = state.tunnel.gate().await?;
    let heard = lock(&state.tunnel.heard).clone();
    let (status, problem) = match heard {
        Some(Ok(status)) => (Some(status), None),
        Some(Err(problem)) => (None, Some(problem)),
        None => (None, None),
    };
    Ok(Json(GateView {
        connected: gate.is_some(),
        transparent: gate
            .as_ref()
            .is_none_or(|gate| gate.mode == Mode::Transparent),
        address: gate.map(|gate| gate.address),
        handshake_age_seconds: status
            .as_ref()
            .and_then(|status| status.handshake_age_seconds),
        forwards: status.map_or(0, |status| status.forwards),
        problem,
    }))
}

/// Connects this home to a Gate that is already running. The tunnel comes up
/// within a few seconds; asking again shows how it went.
#[utoipa::path(
    put,
    path = "/api/v1/gate",
    request_body = GateSettings,
    responses(
        (status = NO_CONTENT, description = "The Gate is kept, and the tunnel is on its way up."),
        (status = UNAUTHORIZED, body = ProblemBody, description = "Nobody is signed in."),
        (status = UNPROCESSABLE_ENTITY, body = ProblemBody, description = "Something given will not do."),
    )
)]
async fn connect_gate(
    State(state): State<AppState>,
    _: SignedIn,
    Json(gate): Json<GateSettings>,
) -> Result<StatusCode, Problem> {
    let invalid = |sentence: &'static str| Err(Problem::Invalid(sentence.into()));
    let address = gate.address.trim();
    let named = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    if address.is_empty() || address.len() > 253 || !address.chars().all(named) {
        return invalid("A Gate's address is an IPv4 address or a name.");
    }
    // A WireGuard key is 32 bytes, which base64 writes as 44 characters.
    let key =
        |text: &str| text.len() == 44 && text.ends_with('=') && !text.contains(char::is_whitespace);
    if !key(&gate.private_key) || !key(&gate.gate_public_key) || !key(&gate.preshared_key) {
        return invalid("That is not a WireGuard key.");
    }
    // It is sent in a header, so it is held to what cannot end one.
    if gate.token.is_empty() || !gate.token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return invalid("A Gate's token is letters and digits.");
    }
    if gate.wg_port == 0 || gate.api_port == 0 {
        return invalid("A port is a number from 1 to 65535.");
    }
    sqlx::query(
        "INSERT OR REPLACE INTO gate
             (id, address, wg_port, private_key, gate_public_key, preshared_key, token, api_port, mode, created_at)
         VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(address)
    .bind(gate.wg_port)
    .bind(&gate.private_key)
    .bind(&gate.gate_public_key)
    .bind(&gate.preshared_key)
    .bind(&gate.token)
    .bind(gate.api_port)
    .bind(if gate.nat { "nat" } else { "transparent" })
    .bind(auth::now())
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Forgets the Gate and takes home's end of the tunnel down. Servers go back
/// to being reached on the home network only.
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
    _: SignedIn,
) -> Result<StatusCode, Problem> {
    sqlx::query("DELETE FROM gate").execute(&state.db).await?;
    let was_up = lock(&state.tunnel.up_for).take().is_some();
    *lock(&state.tunnel.heard) = None;
    if was_up {
        tokio::task::spawn_blocking(take_down)
            .await
            .map_err(|error| Problem::Internal(error.into()))?;
    }
    Ok(StatusCode::NO_CONTENT)
}
