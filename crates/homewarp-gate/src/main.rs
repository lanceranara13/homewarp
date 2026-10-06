//! Homewarp Gate: the VPS end of the tunnel (PLAN.md §5.2 to §5.5).
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
//!
//! `homewarp-gate join <token>` enrols a VPS, `homewarp-gate leave` undoes
//! that, and `homewarp-gate run` is what the service it installs runs.

mod install;

use std::{
    env, fs,
    io::Read,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use anyhow::{Context, bail, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    routing::{get, post, put},
};
use homewarp_net::{
    Link, ProbeForward, apply, bring_up, gate_ruleset, has_table, heard, new_keypair,
};
use homewarp_proto::{Desired, Probe, ProbeRequest, Protocol, Rotate, Rotated, Status};
use serde::{Deserialize, Serialize};
use tokio::signal::unix::{SignalKind, signal};

/// Where a Gate keeps its two files unless it is told otherwise.
const DIR: &str = "/var/lib/homewarp-gate";
/// More than any home will ask for, and few enough to refuse a mistake.
const MOST_FORWARDS: usize = 1024;
/// How often the kernel is looked at, in case something else has undone the setup.
const LOOK: Duration = Duration::from_secs(30);
/// How long after agreeing to change keys the change is made: long enough for
/// the answer to get home under the old ones.
const SWITCH_AFTER: Duration = Duration::from_millis(500);

#[derive(Clone, Serialize, Deserialize)]
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

impl Config {
    /// This end of the tunnel, as the kernel is to have it.
    fn link(&self) -> Link {
        Link {
            private_key: self.private_key.clone(),
            listen_port: self.listen_port,
            address: self.address,
            peer_public_key: self.home_public_key.clone(),
            preshared_key: self.preshared_key.clone(),
            // Home may speak from its tunnel address and from no other.
            peer_allowed: (self.home_address, 32),
            peer_endpoint: None,
        }
    }

    /// Written beside the file and moved over it, so that it is never half
    /// there, and readable by nobody else: it holds this Gate's key.
    fn keep(&self, dir: &Path) -> anyhow::Result<()> {
        let beside = dir.join("config.json.new");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&beside)
            .with_context(|| format!("writing {}", beside.display()))?;
        serde_json::to_writer(&mut file, self)?;
        file.sync_all()?;
        fs::rename(&beside, dir.join("config.json"))?;
        Ok(())
    }
}

/// A port held open on the VPS's public address while home looks at how a
/// connection through it arrives.
struct Probing {
    forward: ProbeForward,
    /// Nothing is ever accepted on it. It keeps the port from being given to
    /// anything else on this machine for as long as it is sent home.
    _held: std::net::TcpListener,
}

struct Gate {
    dir: PathBuf,
    config: Mutex<Config>,
    desired: Mutex<Desired>,
    /// Keys made for home to see, and not in use until home says it has them.
    next: Mutex<Option<Config>>,
    probe: Mutex<Option<Probing>>,
}

fn lock<T>(shared: &Mutex<T>) -> MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Gate {
    /// The table for `desired`, with the port of a probe in it if there is one.
    fn rules(&self, desired: &Desired) -> anyhow::Result<String> {
        let config = lock(&self.config);
        let probe = lock(&self.probe).as_ref().map(|probe| probe.forward);
        Ok(gate_ruleset(
            &config.wan,
            config.home_address,
            desired,
            probe,
        )?)
    }

    /// Puts the table in the kernel again, as it should be now.
    fn apply_again(&self) -> anyhow::Result<()> {
        let desired = lock(&self.desired).clone();
        Ok(apply(&self.rules(&desired)?)?)
    }
}

/// What follows the command on the command line.
struct Options {
    dir: PathBuf,
    /// Whether `join` also installs and starts the service.
    service: bool,
    /// The interface players arrive on, if it is not the default route's.
    wan: Option<String>,
    /// What is neither a flag nor a flag's value: the join token.
    rest: Vec<String>,
}

fn options(arguments: Vec<String>) -> anyhow::Result<Options> {
    let mut options = Options {
        dir: PathBuf::from(DIR),
        service: true,
        wan: None,
        rest: Vec::new(),
    };
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--dir" => options.dir = arguments.next().context("--dir takes a directory")?.into(),
            "--wan" => options.wan = Some(arguments.next().context("--wan takes an interface")?),
            "--no-service" => options.service = false,
            flag if flag.starts_with("--") => bail!("there is no option {flag}"),
            _ => options.rest.push(argument),
        }
    }
    Ok(options)
}

fn main() -> anyhow::Result<()> {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "run".to_owned());
    let options = options(arguments.collect())?;
    match command.as_str() {
        "run" => {
            tracing_subscriber::fmt().with_ansi(false).init();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run(options.dir))
        }
        "join" => {
            let [token] = options.rest.as_slice() else {
                bail!("usage: homewarp-gate join <token>");
            };
            install::join(token, &options)
        }
        "leave" => install::leave(&options),
        "version" | "--version" => {
            println!("homewarp-gate {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => bail!("usage: homewarp-gate run | join <token> | leave | version"),
    }
}

async fn run(dir: PathBuf) -> anyhow::Result<()> {
    let config: Config = read(&dir.join("config.json"))?.with_context(|| {
        format!(
            "there is no config.json in {}: enrol this VPS with `homewarp-gate join`",
            dir.display()
        )
    })?;
    let desired: Desired = read(&dir.join("desired.json"))?.unwrap_or_default();

    bring_up(&config.link())?;
    forward_packets()?;
    tracing::info!(
        "The tunnel is up on UDP {} with {} forwards, generation {}.",
        config.listen_port,
        desired.forwards.len(),
        desired.generation
    );

    let at = SocketAddr::from((config.address, config.api_port));
    let gate = Arc::new(Gate {
        dir,
        config: Mutex::new(config),
        desired: Mutex::new(desired),
        next: Mutex::default(),
        probe: Mutex::default(),
    });
    gate.apply_again()?;
    let listener = listen(at).await?;
    tokio::spawn(keep_set_up(Arc::clone(&gate)));
    let app = Router::new()
        .route("/v1/state", put(set_state))
        .route("/v1/status", get(status))
        .route("/v1/rotate", post(rotate))
        .route("/v1/rotate/commit", post(commit))
        .route("/v1/probe", post(probe))
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

/// Puts back what something else on the VPS took away: a firewall that
/// restarts empties the whole ruleset, this Gate's table with it.
async fn keep_set_up(gate: Arc<Gate>) {
    loop {
        tokio::time::sleep(LOOK).await;
        let link = lock(&gate.config).link();
        // An interface that is as it should be is left alone.
        let kernel = bring_up(&link)
            .map_err(anyhow::Error::new)
            .and_then(|()| forward_packets());
        if let Err(error) = kernel {
            tracing::error!("{error:#}");
        }
        if !has_table() {
            tracing::warn!("The Gate's table was gone from the kernel. Putting it back.");
            if let Err(error) = gate.apply_again() {
                tracing::error!("{error:#}");
            }
        }
    }
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

/// Whether text is a WireGuard key: 32 bytes, which base64 writes as 44 characters.
fn is_key(text: &str) -> bool {
    text.len() == 44
        && text.ends_with('=')
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='))
}

/// A new token for Core to show: 24 bytes from the kernel, in hexadecimal.
fn new_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 24];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

type Refusal = (StatusCode, String);

fn failed(error: impl std::fmt::Display) -> Refusal {
    tracing::error!("{error:#}");
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
}

/// Whether the request carries this Gate's token. Compared without stopping at
/// the first difference, so that how long it takes says nothing of the token.
fn allowed(gate: &Gate, headers: &HeaderMap) -> Result<(), Refusal> {
    let shown = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let config = lock(&gate.config);
    let token = config.token.as_bytes();
    let same = shown.len() == token.len()
        && !token.is_empty()
        && shown
            .bytes()
            .zip(token)
            .fold(0, |differences, (a, b)| differences | (a ^ b))
            == 0;
    match same {
        true => Ok(()),
        false => Err((
            StatusCode::UNAUTHORIZED,
            "That is not this Gate's token.".to_owned(),
        )),
    }
}

/// Takes all that Core wants done and does it: the rules first, which nft
/// applies whole or not at all, and then the file, so that what is kept is
/// what took effect.
async fn set_state(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
    Json(mut wanted): Json<Desired>,
) -> Result<Json<Status>, Refusal> {
    allowed(&gate, &headers)?;
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
    *lock(&gate.desired) = wanted;
    status(State(gate), headers).await
}

fn carry_out(gate: &Gate, wanted: &Desired) -> anyhow::Result<()> {
    ensure!(
        wanted.forwards.len() <= MOST_FORWARDS,
        "{} forwards is more than a Gate takes",
        wanted.forwards.len()
    );
    let listen_port = lock(&gate.config).listen_port;
    for forward in &wanted.forwards {
        // Forwarded, the tunnel's own port would send the tunnel into itself.
        if forward.port == 0 || (forward.protocol == Protocol::Udp && forward.port == listen_port) {
            bail!("port {} cannot be forwarded", forward.port);
        }
    }
    apply(&gate.rules(wanted)?)?;
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
    allowed(&gate, &headers)?;
    let heard = heard().map_err(failed)?;
    let desired = lock(&gate.desired);
    Ok(Json(Status {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        generation: desired.generation,
        mode: desired.mode,
        forwards: desired.forwards.len(),
        handshake_age_seconds: heard.handshake_age_seconds,
        home_endpoint: heard.endpoint,
        received_bytes: heard.received_bytes,
        sent_bytes: heard.sent_bytes,
    }))
}

/// Makes a key that has been nowhere but here, and a new token, and tells
/// home the public half and the token. Nothing changes yet: this Gate goes on
/// with the keys it has until [`commit`] says that home has the new ones.
async fn rotate(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
    Json(asked): Json<Rotate>,
) -> Result<Json<Rotated>, Refusal> {
    allowed(&gate, &headers)?;
    if !is_key(&asked.preshared_key) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "That is not a WireGuard key.".to_owned(),
        ));
    }
    let (private_key, public_key) = new_keypair();
    let token = new_token().map_err(failed)?;
    let next = Config {
        private_key,
        preshared_key: asked.preshared_key,
        token: token.clone(),
        ..lock(&gate.config).clone()
    };
    *lock(&gate.next) = Some(next);
    Ok(Json(Rotated { public_key, token }))
}

/// Switches to the keys [`rotate`] made. They are written down first, so that
/// a restart at any moment after this comes back with them, and put to use
/// once the answer has had time to get home under the old ones.
async fn commit(State(gate): State<Arc<Gate>>, headers: HeaderMap) -> Result<StatusCode, Refusal> {
    allowed(&gate, &headers)?;
    let Some(next) = lock(&gate.next).take() else {
        return Err((
            StatusCode::CONFLICT,
            "There are no new keys to switch to.".to_owned(),
        ));
    };
    next.keep(&gate.dir).map_err(failed)?;
    *lock(&gate.config) = next.clone();
    tokio::spawn(async move {
        tokio::time::sleep(SWITCH_AFTER).await;
        match bring_up(&next.link()) {
            Ok(()) => tracing::info!("This Gate has keys of its own now."),
            Err(error) => tracing::error!("{error:#}"),
        }
    });
    Ok(StatusCode::NO_CONTENT)
}

/// Sends one more port home for a few seconds: a port of this machine that
/// nothing listens on, to the port home says it is listening on.
async fn probe(
    State(gate): State<Arc<Gate>>,
    headers: HeaderMap,
    Json(asked): Json<ProbeRequest>,
) -> Result<Json<Probe>, Refusal> {
    allowed(&gate, &headers)?;
    if asked.home_port == 0 {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            "A probe needs a port at home.".to_owned(),
        ));
    }
    let held = free_port(&gate).map_err(failed)?;
    let forward = ProbeForward {
        public_port: held.local_addr().map_err(failed)?.port(),
        home_port: asked.home_port,
    };
    *lock(&gate.probe) = Some(Probing {
        forward,
        _held: held,
    });
    if let Err(error) = gate.apply_again() {
        *lock(&gate.probe) = None;
        return Err(failed(error));
    }
    let seconds = u64::from(asked.seconds.clamp(1, 30));
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        {
            let mut probe = lock(&gate.probe);
            // A later probe has taken this one's place, and ends by itself.
            if probe.as_ref().map(|probe| probe.forward) != Some(forward) {
                return;
            }
            *probe = None;
        }
        if let Err(error) = gate.apply_again() {
            tracing::error!("{error:#}");
        }
    });
    Ok(Json(Probe {
        port: forward.public_port,
    }))
}

/// A TCP port that nothing on this machine listens on and that is not sent home.
fn free_port(gate: &Gate) -> anyhow::Result<std::net::TcpListener> {
    for _ in 0..16 {
        let held = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        let port = held.local_addr()?.port();
        let forwarded = lock(&gate.desired)
            .forwards
            .iter()
            .any(|forward| forward.port == port && forward.protocol == Protocol::Tcp);
        if !forwarded {
            return Ok(held);
        }
    }
    bail!("no port could be found for a probe")
}
