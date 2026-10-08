//! `run`: start the selected roles under one shutdown signal.

use std::sync::Arc;

use engine::executor::KubeSandboxResolver;
use engine::{
    CachedPermissions, ConnectorPermissions, ConnectorRegistry, Executor, ExecutorConfig,
    KubeProjectIndex, Reducer, ReducerConfig, Ticker,
};
use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::Error;
use crate::cli::Role;
use crate::config::Config;
use crate::health;

/// Flip the returned sender's watch on SIGTERM or SIGINT.
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

    let needs_kube = cfg.roles.contains(&Role::Reducer) || cfg.roles.contains(&Role::Executor);
    let needs_connector = needs_kube;
    let kube_client = if needs_kube {
        Some(
            kube::Client::try_default()
                .await
                .map_err(|e| Error::Run(format!("kubernetes client: {e}")))?,
        )
    } else {
        None
    };
    let connectors = if needs_connector {
        // GitHub App credentials come from GITHUB_APP_ID / GITHUB_APP_PRIVATE_KEY.
        Some(
            ConnectorRegistry::github_from_env()
                .map_err(|e| Error::Run(format!("github connector: {e}")))?,
        )
    } else {
        None
    };

    let mut tasks: JoinSet<(&'static str, Result<(), String>)> = JoinSet::new();
    let mut index_task = None;

    // Project index is shared by reducer and executor; start it once.
    let index = match &kube_client {
        Some(c) => {
            let (index, task) = KubeProjectIndex::start(c.clone());
            index_task = Some(task);
            tracing::info!("waiting for the Project index");
            index.wait_ready().await;
            Some(Arc::new(index))
        }
        None => None,
    };

    if cfg.roles.contains(&Role::Reducer) {
        let connectors = connectors.as_ref().expect("connectors");
        let gh = connectors
            .get("github")
            .ok_or_else(|| Error::Run("github connector not registered".into()))?
            .clone();
        let permissions = CachedPermissions::new(ConnectorPermissions::new().with("github", gh));
        let rcfg = ReducerConfig {
            bot_login: cfg.bot_login.clone(),
            bot_handle: cfg.bot_handle.clone(),
            ..ReducerConfig::default()
        };
        let reducer = Arc::new(Reducer::new(
            pool.clone(),
            index.clone().expect("index"),
            Arc::new(permissions),
            rcfg,
        ));
        let rx = rx.clone();
        tasks.spawn(async move { ("reducer", reducer.run(rx).await.map_err(|e| e.to_string())) });
    }

    if cfg.roles.contains(&Role::Ticker) {
        let ticker = Ticker::new(pool.clone());
        let rx = rx.clone();
        tasks.spawn(async move {
            ticker.run(rx).await;
            ("ticker", Ok(()))
        });
    }

    if cfg.roles.contains(&Role::Executor) {
        let ecfg = ExecutorConfig::new(
            cfg.executor_url.clone(),
            cfg.executor_callback_secret.clone(),
        );
        let executor = Arc::new(Executor::new(
            pool.clone(),
            connectors.clone().expect("connectors"),
            Arc::new(KubeSandboxResolver::new(kube_client.clone().expect("kube"))),
            ecfg,
        ));
        let listener = tokio::net::TcpListener::bind(cfg.executor_listen)
            .await
            .map_err(|e| Error::Run(format!("binding {}: {e}", cfg.executor_listen)))?;
        tracing::info!(addr = %cfg.executor_listen, "executor callback + health server");
        let app = executor.callback_router();
        let (srx, ex_rx) = (rx.clone(), rx.clone());
        tasks.spawn(async move {
            (
                "executor-http",
                health::serve_on(listener, app, srx)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
        tasks.spawn(async move {
            (
                "executor",
                executor.run(ex_rx).await.map_err(|e| e.to_string()),
            )
        });
    } else {
        let addr = cfg.health_listen;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| Error::Run(format!("binding {addr}: {e}")))?;
        tracing::info!(%addr, "health server");
        let srx = rx.clone();
        tasks.spawn(async move {
            (
                "health",
                health::serve_on(listener, health::router(), srx)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

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
    if let Some(t) = index_task {
        t.abort();
    }
    match failure {
        Some(f) => Err(Error::Run(f)),
        None => Ok(()),
    }
}
