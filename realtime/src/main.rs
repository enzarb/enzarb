use std::sync::Arc;

use anyhow::Result;
use enzarb_realtime::{
    config::Config,
    hub::Hub,
    listener::{self, Backoff},
    pg::{PgConnector, PgLookup},
    ws,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::from_env()?;
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
    });
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!(listen = %cfg.listen, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).expect("sigterm");
            tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {}, _ = listener_task => { tracing::error!("listener task exited"); } }
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
