use std::sync::Arc;

use anyhow::Context;
use sandbox_init::{AppState, Config, reaper::Reaper, serve};
use tokio::signal::unix::{SignalKind, signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let grace = config.grace;
    let reaper = Reaper::for_current_process();
    tokio::spawn(reaper.clone().run());

    // Install handlers before serving so an early SIGTERM is not lost.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind {}", config.listen))?;
    tracing::info!(addr = %config.listen, reap_all = reaper.reaps_all(), "sandbox-init listening");
    let state = Arc::new(AppState::new(config, reaper.clone()));

    tokio::select! {
        r = serve(listener, state) => r.context("server failed")?,
        _ = sigterm.recv() => tracing::info!("SIGTERM received"),
        _ = sigint.recv() => tracing::info!("SIGINT received"),
    }

    // Forward the signal to the children and give them the grace period.
    if !reaper.terminate_all(grace).await {
        tracing::warn!("children did not exit within the grace period; killed");
    }
    tracing::info!("bye");
    Ok(())
}
