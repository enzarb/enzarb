//! `run`: start the selected roles under one shutdown signal.

use std::sync::Arc;
use std::time::Duration;

use engine::findings::{DbPlatformFacts, HttpDeployState};
use engine::sandbox_poller::{MuxStatsClient, PgPollLock, PgSampleSink, SandboxPoller};
use engine::{
    CachedPermissions, ConnectorPermissions, Engine, EngineConfig, Findings, KubeProjectIndex,
    KubeSandboxResolver, Library, MuxStepRunner,
};
use enzarb_connector::Connector;
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::Error;
use crate::cli::Role;
use crate::config::Config;
use crate::health;

fn spawn_signal_listener(tx: watch::Sender<bool>) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM received; shutting down"),
            _ = int.recv() => tracing::info!("SIGINT received; shutting down"),
        }
        let _ = tx.send(true);
    });
}

pub async fn run(cfg: Config) -> Result<(), Error> {
    let (tx, rx) = watch::channel(false);
    spawn_signal_listener(tx.clone());

    let pool = engine::db::connect(&cfg.database_url, cfg.db_max_connections)
        .await
        .map_err(|e| Error::Run(format!("connecting to database: {e}")))?;
    let kube = kube::Client::try_default()
        .await
        .map_err(|e| Error::Run(format!("kubernetes client: {e}")))?;
    // GitHub App credentials come from GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY.
    let gh: Arc<dyn Connector> = Arc::new(
        enzarb_connector_github::GitHubConnector::from_env()
            .map_err(|e| Error::Run(format!("github connector: {e}")))?,
    );
    let (index, index_task) = KubeProjectIndex::start(kube.clone());
    tracing::info!("waiting for the Project index");
    index.wait_ready().await;
    let sandboxes = Arc::new(KubeSandboxResolver::new(kube.clone()));
    let mut facts =
        DbPlatformFacts::new(pool.clone(), sandboxes.clone()).with_connector(gh.clone());
    if let Some(url) = &cfg.updater_status_url {
        facts = facts.with_deploy_state(Arc::new(HttpDeployState::new(url)));
    }
    let engine = Engine {
        pool: pool.clone(),
        index: Arc::new(index),
        library: Library::new(gh.clone(), &cfg.default_library, cfg.library_repos.clone())
            .with_budget(std::time::Duration::from_secs(cfg.resolve_budget_secs)),
        permissions: Arc::new(CachedPermissions::new(
            ConnectorPermissions::new().with("github", gh.clone()),
        )),
        runner: Arc::new(MuxStepRunner::new(sandboxes.clone())),
        sandboxes: sandboxes.clone(),
        secrets_key: cfg.secrets_key.clone(),
        claude: cfg.claude.clone(),
        findings: Findings::new(Arc::new(facts))
            .with_interval(Duration::from_secs(cfg.findings_secs)),
        cfg: EngineConfig {
            bot_handle: cfg.bot_handle.clone(),
            bot_login: cfg.bot_login.clone(),
            public_url: cfg.public_url.clone(),
            stall: Duration::from_secs(cfg.stall_secs),
            max_steps_per_project: cfg.max_steps_per_project as i64,
            worker_id: std::env::var("HOSTNAME")
                .unwrap_or_else(|_| format!("workflows-{}", std::process::id())),
            ..EngineConfig::default()
        },
    };

    let mut tasks: JoinSet<(&'static str, Result<(), String>)> = JoinSet::new();
    if cfg.roles.contains(&Role::Reducer) {
        let (e, rx) = (engine.clone(), rx.clone());
        tasks.spawn(async move {
            (
                "reducer",
                e.run_reducer(rx).await.map_err(|e| e.to_string()),
            )
        });
    }
    if cfg.roles.contains(&Role::Executor) {
        let (e, rx) = (engine.clone(), rx.clone());
        tasks.spawn(async move {
            (
                "executor",
                e.run_executor(rx).await.map_err(|e| e.to_string()),
            )
        });
    }
    if cfg.roles.contains(&Role::Ticker) {
        let (e, rx2) = (engine.clone(), rx.clone());
        tasks.spawn(async move { ("ticker", e.run_ticker(rx2).await.map_err(|e| e.to_string())) });
        // Its own task: a hung sandbox must never delay the ticker.
        let poller = SandboxPoller::new(
            sandboxes.clone(),
            Arc::new(MuxStatsClient),
            Arc::new(PgSampleSink::new(pool.clone())),
            Arc::new(PgPollLock::new(pool.clone())),
            cfg.stats.clone(),
        );
        let rx = rx.clone();
        tasks.spawn(async move {
            poller.run(rx).await;
            ("sandbox-poller", Ok(()))
        });
    }
    let listener = tokio::net::TcpListener::bind(cfg.health_listen)
        .await
        .map_err(|e| Error::Run(format!("binding {}: {e}", cfg.health_listen)))?;
    tracing::info!(addr = %cfg.health_listen, "health server");
    let srx = rx.clone();
    tasks.spawn(async move {
        (
            "health",
            health::serve_on(listener, health::router(), srx)
                .await
                .map_err(|e| e.to_string()),
        )
    });
    tracing::info!(roles = ?cfg.roles.iter().map(|r| r.as_str()).collect::<Vec<_>>(), "started");

    // Any task finishing (error or not) before shutdown brings the whole process down.
    let mut failure: Option<String> = None;
    while let Some(joined) = tasks.join_next().await {
        let (name, res) = match joined {
            Ok(v) => v,
            Err(e) => ("task", Err(format!("panicked: {e}"))),
        };
        match res {
            Ok(()) => tracing::info!(task = name, "stopped"),
            Err(e) => {
                tracing::error!(task = name, error = %e, "stopped with error");
                failure.get_or_insert(format!("{name}: {e}"));
            }
        }
        let _ = tx.send(true);
    }
    index_task.abort();
    match failure {
        Some(f) => Err(Error::Run(f)),
        None => Ok(()),
    }
}
