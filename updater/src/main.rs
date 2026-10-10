use std::{net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use enzarb_updater::{app, charts::KubeCharts, registry::OciRegistry};
use tracing_subscriber::EnvFilter;

fn env_or(k: &str, d: &str) -> String {
    std::env::var(k)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| d.into())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let secret = std::env::var("ENZARB_UPDATER_WEBHOOK_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
        .context("ENZARB_UPDATER_WEBHOOK_SECRET is required")?;
    let chart = env_or("ENZARB_UPDATER_CHART", "oci://ghcr.io/enzarb/charts/enzarb");
    let token = std::env::var("ENZARB_UPDATER_REGISTRY_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    let listen: SocketAddr = env_or("ENZARB_UPDATER_LISTEN", "0.0.0.0:8080").parse()?;
    let name = env_or("ENZARB_UPDATER_HELMCHART", "enzarb");
    let ns = env_or("ENZARB_UPDATER_NAMESPACE", "kube-system");

    let client = kube::Client::try_default().await?;
    let state = app::AppState {
        secret: Arc::new(secret.into_bytes()),
        registry: Arc::new(OciRegistry::new(&chart, token)?),
        charts: Arc::new(KubeCharts::new(client, &ns, &name)),
        lock: Default::default(),
    };
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, %chart, helmchart = %name, namespace = %ns, "listening");
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(async {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).expect("sigterm");
            tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
