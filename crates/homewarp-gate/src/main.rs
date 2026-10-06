//! Homewarp Gate: the VPS end of the tunnel (PLAN.md §5.2 to §5.4).
//!
//! It sets the kernel up, with a WireGuard interface and one nftables table,
//! and then answers Core. Players' packets never pass through this process:
//! the kernel forwards them, and goes on forwarding them while this is
//! restarted or replaced.
//!
//! It keeps two files in its directory. `config.json` is written when the VPS
//! is enrolled and says who this Gate is and who its home is. `desired.json`
//! is the last thing Core asked for, kept so that a reboot comes back to it
//! before home has been heard from again.

use std::{
    env, fs,
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{get, put},
};
use homewarp_net::{Link, apply, bring_up, gate_ruleset, heard};
use homewarp_proto::{Desired, Protocol, Status};
use serde::Deserialize;
use tokio::signal::unix::{SignalKind, signal};

/// More than any home will ask for, and few enough to refuse a mistake.
const MOST_FORWARDS: usize = 1024;

#[derive(Deserialize)]
struct Config {
    private_key: String,
    /// The UDP port WireGuard listens on, which home dials.
    listen_port: u16,
    /// This Gate's address inside the tunnel, and home's.
    address: Ipv4Addr,
    home_address: Ipv4Addr,
    home_public_key: String,
    preshared_key: String,
    /// What Core has to show, on top of having come through the tunnel.
    token: String,
    /// The interface players arrive on.
    wan: String,
    /// Where this answers Core, on the tunnel address only.
    api_port: u16,
}

struct Gate {
    config: Config,
    dir: PathBuf,
    desired: Mutex<Desired>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_ansi(false).init();
    let dir = PathBuf::from(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "/var/lib/homewarp-gate".to_owned()),
    );
    let config: Config = read(&dir.join("config.json"))?.context("there is no config.json")?;
    let desired: Desired = read(&dir.join("desired.json"))?.unwrap_or_default();

    bring_up(&Link {
        private_key: config.private_key.clone(),
        listen_port: config.listen_port,
        address: config.address,
        peer_public_key: config.home_public_key.clone(),
        preshared_key: config.preshared_key.clone(),
        // Home may speak from its tunnel address and from no other.
        peer_allowed: (config.home_address, 32),
        peer_endpoint: None,
    })?;
    forward_packets()?;
    apply(&gate_ruleset(&config.wan, config.home_address, &desired)?)?;
    tracing::info!(
        "The tunnel is up on UDP {} with {} forwards, generation {}.",
        config.listen_port,
        desired.forwards.len(),
        desired.generation
    );

    let at = SocketAddr::from((config.address, config.api_port));
    let listener = listen(at).await?;
    let gate = Arc::new(Gate {
        config,
        dir,
        desired: Mutex::new(desired),
    });
    let app = Router::new()
        .route("/v1/state", put(set_state))
        .route("/v1/status", get(status))
        .with_state(gate);
    // The kernel is left as it is on the way out: forwarding does not stop
    // because this process does.
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let mut terminate = signal(SignalKind::terminate()).expect("a SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        })
        .await?;
    Ok(())
}

/// A JSON file's contents, or None if there is no such file.
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("reading {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// A router forwards; a host does not until it is told to.
fn forward_packets() -> anyhow::Result<()> {
    const SWITCH: &str = "/proc/sys/net/ipv4/ip_forward";
    if fs::read_to_string(SWITCH).is_ok_and(|now| now.trim() == "1") {
        return Ok(());
    }
    fs::write(SWITCH, "1").context("turning on packet forwarding")
}

/// The tunnel address may take a moment to be usable after it is assigned.
async fn listen(at: SocketAddr) -> anyhow::Result<tokio::net::TcpListener> {
    for _ in 0..20 {
        if let Ok(listener) = tokio::net::TcpListener::bind(at).await {
            return Ok(listener);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(tokio::net::TcpListener::bind(at).await?)
}

/// Whether the request carries this Gate's token. Compared without stopping at
/// the first difference, so that how long it takes says nothing of the token.
fn allowed(gate: &Gate, headers: &HeaderMap) -> bool {
    let shown = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let token = gate.config.token.as_bytes();
    shown.len() == token.len()
        && !token.is_empty()
        && shown
            .bytes()
            .zip(token)
            .fold(0, |differences, (a, b)| differences | (a ^ b))
            == 0
}

type Refusal = (StatusCode, String);

/// Takes all that Core wants done and does it: the rules first, which nft
/// applies whole or not at all, and then the file, so that what is kept is
/// what took effect.
async fn set_state(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
    Json(mut wanted): Json<Desired>,
) -> Result<Json<Status>, Refusal> {
    if !allowed(&gate, &headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "That is not this Gate's token.".to_owned(),
        ));
    }
    wanted.forwards.sort();
    wanted.forwards.dedup();
    carry_out(&gate, &wanted).map_err(|error| {
        tracing::error!("{error:#}");
        (StatusCode::UNPROCESSABLE_ENTITY, format!("{error:#}"))
    })?;
    tracing::info!(
        "Generation {}: {} forwards.",
        wanted.generation,
        wanted.forwards.len()
    );
    *gate.desired.lock().unwrap_or_else(PoisonError::into_inner) = wanted;
    status(State(gate), headers).await
}

fn carry_out(gate: &Gate, wanted: &Desired) -> anyhow::Result<()> {
    ensure!(
        wanted.forwards.len() <= MOST_FORWARDS,
        "{} forwards is more than a Gate takes",
        wanted.forwards.len()
    );
    for forward in &wanted.forwards {
        // Forwarded, the tunnel's own port would send the tunnel into itself.
        if forward.port == 0
            || (forward.protocol == Protocol::Udp && forward.port == gate.config.listen_port)
        {
            bail!("port {} cannot be forwarded", forward.port);
        }
    }
    apply(&gate_ruleset(
        &gate.config.wan,
        gate.config.home_address,
        wanted,
    )?)?;
    // Written beside the file and moved over it, so that it is never half there.
    let kept = gate.dir.join("desired.json");
    let beside = gate.dir.join("desired.json.new");
    fs::write(&beside, serde_json::to_vec(wanted)?)?;
    fs::rename(&beside, &kept)?;
    Ok(())
}

async fn status(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
) -> Result<Json<Status>, Refusal> {
    if !allowed(&gate, &headers) {
        return Err((
            StatusCode::UNAUTHORIZED,
            "That is not this Gate's token.".to_owned(),
        ));
    }
    let heard = heard().map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let desired = gate.desired.lock().unwrap_or_else(PoisonError::into_inner);
    Ok(Json(Status {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        generation: desired.generation,
        mode: desired.mode,
        forwards: desired.forwards.len(),
        handshake_age_seconds: heard.handshake_age_seconds,
        received_bytes: heard.received_bytes,
        sent_bytes: heard.sent_bytes,
    }))
}
