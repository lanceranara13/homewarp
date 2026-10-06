use std::{
    env,
    io::IsTerminal,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use anyhow::Context;
use homewarp_core::{AppState, Runtime};
use tokio::{
    net::{TcpListener, UnixListener, UnixStream},
    signal::unix::{SignalKind, signal},
};

/// Where the panel is to be reached unless `HOMEWARP_LISTEN` says otherwise.
const LISTEN: &str = "0.0.0.0:3600";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut arguments = env::args().skip(1);
    match arguments.next().as_deref() {
        // `homewarp openapi` prints the API description that web/openapi.json is a copy of.
        Some("openapi") => {
            println!("{}", homewarp_core::openapi().to_pretty_json()?);
            return Ok(());
        }
        // What Core runs itself as, in a container, to probe the tunnel.
        Some("probe-listen") => {
            let port = arguments.next().unwrap_or_default().parse()?;
            return homewarp_core::probe_listen(port).await;
        }
        // `homewarp door <address> <socket>`: the panel's door (deploy/compose.yml).
        Some("door") => {
            let usage = "usage: homewarp door <address> <socket>";
            let listen: SocketAddr = arguments.next().context(usage)?.parse()?;
            let socket = PathBuf::from(arguments.next().context(usage)?);
            return door(listen, &socket).await;
        }
        _ => {}
    }
    // Colour is for a terminal; in `docker logs` it would be escape codes around the setup code.
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    let data = PathBuf::from(env::var("HOMEWARP_DATA").unwrap_or_else(|_| "data".to_owned()));
    let listen = env::var("HOMEWARP_LISTEN").unwrap_or_else(|_| LISTEN.to_owned());
    std::fs::create_dir_all(&data)?;

    let db = homewarp_core::open(&data.join("homewarp.db")).await?;
    // Without Docker the panel still opens. It says so where a server would be made.
    let runtime = match Runtime::start(&data, db.clone()).await {
        Ok(runtime) => Some(runtime),
        Err(error) => {
            tracing::warn!("Homewarp cannot run servers: {error:#}");
            None
        }
    };
    let state = AppState::start(db, runtime).await?;
    state.keep_tunnel();
    let said = |at: &str| {
        tracing::info!(
            "Homewarp {} is listening on {at}",
            env!("CARGO_PKG_VERSION")
        );
        if let Some(code) = state.setup_code() {
            tracing::info!(
                "There is no account yet. Open the panel and create one with this setup code: {code}"
            );
        }
    };

    // `unix:<path>` is a socket in the file system, for a door to pass
    // connections to. Anything else is an address to listen on.
    match listen.strip_prefix("unix:") {
        Some(socket) => {
            let listener = socket_at(Path::new(socket))?;
            said(socket);
            axum::serve(listener, homewarp_core::app(state))
                .with_graceful_shutdown(stop_requested())
                .await?;
        }
        None => {
            let address: SocketAddr = listen.parse()?;
            let listener = TcpListener::bind(address).await?;
            said(&format!("http://{address}"));
            axum::serve(listener, homewarp_core::app(state))
                .with_graceful_shutdown(stop_requested())
                .await?;
        }
    }
    Ok(())
}

/// Listens on a socket in the file system, in place of one a Core that
/// stopped left behind.
fn socket_at(socket: &Path) -> anyhow::Result<UnixListener> {
    if let Some(directory) = socket.parent() {
        std::fs::create_dir_all(directory)?;
    }
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("replacing {}", socket.display()));
        }
        _ => {}
    }
    UnixListener::bind(socket).with_context(|| format!("listening on {}", socket.display()))
}

/// The panel's door. Core sets up its end of the tunnel in the machine's own
/// network, and a program there is behind whatever firewall the machine has:
/// a port it listened on itself would be shut on a machine that shuts what it
/// was not told about. A port that Docker publishes is not, and that is how
/// everything else on such a machine is reached. So the door is this program
/// once more, in a container of the ordinary kind with the panel's port
/// published, and all it does is pass each connection to Core's socket, which
/// the two share as a file.
async fn door(listen: SocketAddr, socket: &Path) -> anyhow::Result<()> {
    let listener = TcpListener::bind(listen).await?;
    let stop = stop_requested();
    tokio::pin!(stop);
    loop {
        let (mut from, _) = tokio::select! {
            accepted = listener.accept() => accepted?,
            () = &mut stop => return Ok(()),
        };
        let socket = socket.to_owned();
        tokio::spawn(async move {
            // Core is being restarted, most likely. The browser asks again.
            if let Ok(mut to) = UnixStream::connect(&socket).await {
                let _ = tokio::io::copy_bidirectional(&mut from, &mut to).await;
            }
        });
    }
}

/// Resolves on Ctrl-C or on the SIGTERM that `docker stop` sends.
async fn stop_requested() {
    let mut terminate =
        signal(SignalKind::terminate()).expect("a SIGTERM handler can be installed");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
