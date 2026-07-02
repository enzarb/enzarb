mod acp;
mod auth;
mod external;
mod init;
mod internal;
mod path_utils;
mod process;
mod terminal;

use anyhow::Result;
use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "project_agent=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Exclusive non-blocking flock — released automatically on process exit.
    // A second instance finds the lock held and exits immediately.
    let lock_path = init::home_dir().join(".enzarb/agent.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)?;
    let _lock =
        match nix::fcntl::Flock::lock(lock_file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(guard) => guard,
            Err(_) => {
                println!("project-agent v{VERSION} is already running");
                std::process::exit(1);
            }
        };

    let project_id = std::env::var("ENZARB_PROJECT_ID").expect("ENZARB_PROJECT_ID must be set");
    let org_id = std::env::var("ENZARB_ORG_ID").expect("ENZARB_ORG_ID must be set");
    let project_slug =
        std::env::var("ENZARB_PROJECT_SLUG").expect("ENZARB_PROJECT_SLUG must be set");

    tracing::info!(project_id, org_id, project_slug, "starting project-agent");

    // Fetch and cache JWKS + revoked JTI list for JWT validation. The base
    // origin is configurable (Helm-driven) via APP_ORIGIN so the agent's JWKS,
    // revocation, and expected issuer all track the deployment domain rather
    // than a hardcoded host. It must equal the issuer (`iss`) the app signs with.
    let base = std::env::var("APP_ORIGIN").unwrap_or_else(|_| "https://enzarb.dev".to_string());
    let base = base.trim_end_matches('/').to_string();
    let jwks_url = format!("{base}/.well-known/jwks.json");
    let revoked_url = format!("{base}/.well-known/revoked-jtis");
    let jwks = auth::JwksCache::new(jwks_url, revoked_url, base).await?;

    // First-boot initialization: write mise.toml if absent, run mise install
    init::bootstrap().await?;

    // Rehydrate persistent processes from state file
    let process_store = process::ProcessStore::load_or_create().await?;

    // ACP (Agent tab) session index — the store itself lazily spawns
    // claude-agent-acp on first use, not here.
    let acp_session_index = acp::SessionIndex::load_or_create().await?;
    let acp_store = acp::AcpStore::new(init::home_dir(), acp_session_index);

    let state = AppState {
        project_id: project_id.clone(),
        org_id,
        project_slug,
        jwks,
        process_store,
        acp_store,
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
    pub acp_store: acp::AcpStore,
}
