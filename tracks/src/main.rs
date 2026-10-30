use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use enzarb_tracks::exec::{ExecConfig, Executor};
use enzarb_tracks::github::{GithubApp, NoApp, RepoSource};
use enzarb_tracks::projects::KubeProjects;
use enzarb_tracks::router::Router;
use enzarb_tracks::sandbox::{KubeSandboxes, StoredTokens};
use enzarb_tracks::{api, config::Config, db};
use tracing_subscriber::EnvFilter;

fn database_url() -> Result<String> {
    std::env::var("DATABASE_URL").context("DATABASE_URL is required")
}

#[tokio::main]
async fn main() -> Result<()> {
    enzarb_tracks::install_crypto_provider();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // `enzarb-tracks migrate`: the chart's migration Job (runs as the schema owner role).
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let pool = db::connect(&database_url()?, 1).await?;
        db::migrate(&pool).await?;
        tracing::info!("migrations applied");
        return Ok(());
    }

    let cfg = Config::from_env()?;
    let mut state = api::AppState::new(cfg.token.clone());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut router_task = None;
    let mut exec_task = None;
    if cfg.routing {
        // Routing workers plus, when executing, a driver and a lease heartbeat per run.
        let runs = if cfg.executor {
            2 * cfg.run_concurrency as u32
        } else {
            0
        };
        let pool = db::connect(
            &database_url()?,
            (cfg.concurrency as u32 + 4 + runs).min(64),
        )
        .await?;
        let source: Arc<dyn RepoSource> = match (&cfg.github_app_id, &cfg.github_app_private_key) {
            (Some(id), Some(key)) => Arc::new(GithubApp::new(id, key).context("GitHub App key")?),
            _ => {
                tracing::warn!("no GitHub App configured; every event will get an error decision");
                Arc::new(NoApp)
            }
        };
        let projects = Arc::new(KubeProjects::new().await.context("Kubernetes client")?);
        if cfg.executor {
            let dir = std::env::var("ENZARB_CLAUDE_KEYRING_DIR")
                .unwrap_or_else(|_| "/etc/enzarb/claude-key".into());
            let keyring =
                enzarb_sandbox_proxy::credential::Keyring::load(std::path::Path::new(&dir))
                    .context("Claude keyring")?;
            let store = Arc::new(enzarb_sandbox_proxy::store::PgStore::from_env()?);
            let creds = enzarb_sandbox_proxy::store::Credentials::new(store, Arc::new(keyring));
            let ex = Arc::new(Executor::new(
                pool.clone(),
                source.clone(),
                projects.clone(),
                Arc::new(KubeSandboxes::new().await.context("Kubernetes client")?),
                Arc::new(StoredTokens::new(creds).await?),
                ExecConfig {
                    concurrency: cfg.run_concurrency,
                    ..ExecConfig::default()
                },
            ));
            exec_task = Some(tokio::spawn(ex.run(stop_rx.clone())));
            tracing::info!(concurrency = cfg.run_concurrency, "run executor on");
        }
        let r = Arc::new(Router::new(
            pool.clone(),
            source,
            projects,
            cfg.concurrency,
            Duration::from_secs(cfg.sweep_secs),
        ));
        router_task = Some(tokio::spawn(r.run(stop_rx)));
        state = state.with_pool(pool);
        tracing::info!(concurrency = cfg.concurrency, "event routing on");
    } else {
        tracing::info!("event routing off");
    }
    let app = api::router(state, cfg.max_body_bytes);
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
    let _ = stop_tx.send(true);
    if let Some(t) = exec_task {
        let _ = tokio::time::timeout(Duration::from_secs(25), t).await;
    }
    if let Some(t) = router_task {
        let _ = tokio::time::timeout(Duration::from_secs(25), t).await;
    }
    Ok(())
}
