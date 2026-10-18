use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use enzarb_updater::{
    app,
    charts::KubeCharts,
    history::KubeHistory,
    poll::{self, PollHealth},
    registry::OciRegistry,
    rollout,
};
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
        .filter(|s| !s.is_empty());
    let poll_secs: u64 = env_or("ENZARB_UPDATER_POLL_SECS", "300")
        .parse()
        .context("ENZARB_UPDATER_POLL_SECS must be a number of seconds (0 disables polling)")?;
    anyhow::ensure!(
        poll_secs == 0 || poll_secs >= 30,
        "ENZARB_UPDATER_POLL_SECS must be 0 (off) or at least 30"
    );
    anyhow::ensure!(
        secret.is_some() || poll_secs > 0,
        "nothing to do: set ENZARB_UPDATER_WEBHOOK_SECRET and/or ENZARB_UPDATER_POLL_SECS > 0"
    );
    let chart = env_or("ENZARB_UPDATER_CHART", "oci://ghcr.io/enzarb/charts/enzarb");
    let token = std::env::var("ENZARB_UPDATER_REGISTRY_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    let listen: SocketAddr = env_or("ENZARB_UPDATER_LISTEN", "0.0.0.0:8080").parse()?;
    let name = env_or("ENZARB_UPDATER_HELMCHART", "enzarb");
    let ns = env_or("ENZARB_UPDATER_NAMESPACE", "kube-system");
    // Where the chart is installed (the operator Deployment lives there).
    let release_ns = env_or("ENZARB_UPDATER_RELEASE_NAMESPACE", "default");

    let client = kube::Client::try_default().await?;
    let history = Arc::new(KubeHistory::new(client.clone(), &ns));
    let health = (poll_secs > 0).then(|| Arc::new(PollHealth::default()));
    let state = app::AppState {
        secret: secret.map(|s| Arc::new(s.into_bytes())),
        poll: health.clone(),
        registry: Arc::new(OciRegistry::new(&chart, token)?),
        charts: Arc::new(KubeCharts::new(client, &ns, &release_ns, &name)),
        lock: Default::default(),
        status: Default::default(),
    };
    // Visibility only (cached for /status, logs a failing upgrade); its death must not take the
    // webhook or poller down.
    tokio::spawn(rollout::run(
        state.registry.clone(),
        state.charts.clone(),
        history,
        state.status.clone(),
        Duration::from_secs(60),
    ));
    if let Some(health) = health {
        // If the poller ever dies, exit so the pod restarts instead of silently never updating.
        let task = tokio::spawn(poll::run(
            state.registry.clone(),
            state.charts.clone(),
            state.lock.clone(),
            health,
            Duration::from_secs(poll_secs),
        ));
        tokio::spawn(async move {
            let _ = task.await;
            tracing::error!("poller stopped; exiting");
            std::process::exit(1);
        });
    }
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, %chart, webhook = state.secret.is_some(), poll_secs, helmchart = %name, namespace = %ns, "listening");
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
