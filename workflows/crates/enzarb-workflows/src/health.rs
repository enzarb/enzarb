//! Health server for processes that do not run the executor (whose router has `/healthz`).

use std::net::SocketAddr;

use axum::{Router, routing::get};
use tokio::sync::watch;

pub fn router() -> Router {
    Router::new().route("/healthz", get(|| async { "ok" }))
}

/// Serve `GET /healthz` until `shutdown` flips to true.
pub async fn serve(addr: SocketAddr, shutdown: watch::Receiver<bool>) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    serve_on(listener, router(), shutdown).await
}

pub async fn serve_on(
    listener: tokio::net::TcpListener,
    app: Router,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.wait_for(|s| *s).await;
        })
        .await
}
