use std::{
    sync::{Arc, atomic::AtomicU64},
    time::Duration,
};

use anyhow::Result;
use enzarb_realtime::{
    config::Config,
    hub::Hub,
    listener::{self, Backoff},
    pg::{PgConnector, PgLookup},
    ws,
};
use tokio::sync::{Semaphore, watch};
use tracing_subscriber::EnvFilter;

/// Longest we wait for connected clients to receive their Close frame after the signal.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::from_env()?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let hub = Arc::new(Hub::new(cfg.buffer));
    let listener_task = tokio::spawn(listener::run(
        Arc::new(PgConnector {
            url: cfg.database_url.clone(),
        }),
        Arc::new(PgLookup::new(&cfg.database_url)?),
        hub.clone(),
        Backoff::default(),
    ));
    let app = ws::router(ws::AppState {
        hub,
        secret: Arc::new(cfg.jwt_secret.clone()),
        ping_interval: cfg.ping_interval,
        max_token_ttl: cfg.max_token_ttl,
        limit: Arc::new(Semaphore::new(cfg.max_connections)),
        rejected: Arc::new(AtomicU64::new(0)),
        allowed_origins: Arc::new(cfg.allowed_origins.clone()),
        deprecated_query: Arc::new(AtomicU64::new(0)),
        shutdown: shutdown_rx.clone(),
    });
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!(listen = %cfg.listen, max_connections = cfg.max_connections, "listening");
    let serve = axum::serve(listener, app).with_graceful_shutdown(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("sigterm");
        tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {}, _ = listener_task => { tracing::error!("listener task exited"); } }
        tracing::info!("shutting down");
        // Tell every client task to send Close 1001 and finish.
        let _ = shutdown_tx.send(true);
    });
    // Bound the wait for clients to flush their Close frames, counted from the shutdown signal.
    let mut signalled = shutdown_rx;
    let grace = async {
        let _ = signalled.wait_for(|v| *v).await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };
    tokio::select! {
        r = serve => r?,
        _ = grace => tracing::warn!("shutdown grace elapsed; exiting with clients still connected"),
    }
    Ok(())
}
