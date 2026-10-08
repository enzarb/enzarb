use std::{sync::Arc, time::Duration};

use anyhow::Result;
use enzarb_sandbox_proxy::{app, config::Config, kube_resolver::KubeResolver};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::from_env()?;
    let client = kube::Client::try_default().await?;
    let resolver = Arc::new(KubeResolver::start(client, cfg.secret_ttl).await?);
    let shutdown = CancellationToken::new();
    let listen = cfg.listen;
    let state = app::AppState::new(cfg, resolver, shutdown.clone());
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "listening");

    let sd = shutdown.clone();
    tokio::spawn(async move {
        wait_signal().await;
        tracing::info!("shutting down");
        sd.cancel();
    });
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;
    // Upgraded websocket tasks observe the cancellation and send close frames.
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

async fn wait_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("sigterm");
    tokio::select! {
        _ = term.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
    }
}
