use std::{env, io::IsTerminal, net::SocketAddr, path::PathBuf};

use homewarp_core::{AppState, Runtime};
use tokio::signal::unix::{SignalKind, signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `homewarp openapi` prints the API description that web/openapi.json is a copy of.
    if env::args().nth(1).as_deref() == Some("openapi") {
        println!("{}", homewarp_core::openapi().to_pretty_json()?);
        return Ok(());
    }
    // Colour is for a terminal; in `docker logs` it would be escape codes around the setup code.
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    let data = PathBuf::from(env::var("HOMEWARP_DATA").unwrap_or_else(|_| "data".to_owned()));
    let listen: SocketAddr = env::var("HOMEWARP_LISTEN")
        .unwrap_or_else(|_| "0.0.0.0:3600".to_owned())
        .parse()?;
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
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(
        "Homewarp {} is listening on http://{listen}",
        env!("CARGO_PKG_VERSION")
    );
    if let Some(code) = state.setup_code() {
        tracing::info!(
            "There is no account yet. Open the panel and create one with this setup code: {code}"
        );
    }

    axum::serve(listener, homewarp_core::app(state))
        .with_graceful_shutdown(stop_requested())
        .await?;
    Ok(())
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
