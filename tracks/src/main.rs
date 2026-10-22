use anyhow::Result;
use enzarb_tracks::{api, config::Config};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cfg = Config::from_env()?;
    let app = api::router(api::AppState::new(cfg.token.clone()), cfg.max_body_bytes);
    let listener = tokio::net::TcpListener::bind(cfg.listen).await?;
    tracing::info!(listen = %cfg.listen, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            use tokio::signal::unix::{SignalKind, signal};
            let mut term = signal(SignalKind::terminate()).expect("sigterm");
            tokio::select! { _ = term.recv() => {}, _ = tokio::signal::ctrl_c() => {} }
            tracing::info!("shutting down");
        })
        .await?;
    Ok(())
}
