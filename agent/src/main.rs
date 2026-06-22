mod auth;
mod external;
mod init;
mod internal;
mod process;
mod terminal;

use anyhow::Result;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "project_agent=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let project_id = std::env::var("ENZARB_PROJECT_ID").expect("ENZARB_PROJECT_ID must be set");
    let org_id = std::env::var("ENZARB_ORG_ID").expect("ENZARB_ORG_ID must be set");
    let project_slug =
        std::env::var("ENZARB_PROJECT_SLUG").expect("ENZARB_PROJECT_SLUG must be set");

    tracing::info!(project_id, org_id, project_slug, "starting project-agent");

    // Fetch and cache JWKS for JWT validation
    let jwks_url = "https://enzarb.dev/.well-known/jwks.json".to_string();
    let jwks = auth::JwksCache::new(jwks_url).await?;

    // First-boot initialization: write mise.toml if absent, run mise install
    init::bootstrap().await?;

    // Rehydrate persistent processes from state file
    let process_store = process::ProcessStore::load_or_create().await?;

    let state = AppState {
        project_id: project_id.clone(),
        org_id,
        project_slug,
        jwks,
        process_store,
    };

    let internal_addr: SocketAddr = "0.0.0.0:9090".parse()?;
    let external_addr: SocketAddr = "0.0.0.0:8080".parse()?;

    let internal_app = internal::router(state.clone());
    let external_app = external::router(state.clone());

    tracing::info!("internal server listening on {}", internal_addr);
    tracing::info!("external server listening on {}", external_addr);

    let (r1, r2) = tokio::join!(
        serve(internal_app, internal_addr),
        serve(external_app, external_addr),
    );
    r1?;
    r2?;

    Ok(())
}

async fn serve(app: axum::Router, addr: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[derive(Clone)]
pub struct AppState {
    pub project_id: String,
    pub org_id: String,
    pub project_slug: String,
    pub jwks: auth::JwksCache,
    pub process_store: process::ProcessStore,
}
