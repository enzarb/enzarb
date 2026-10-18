//! `receiver [serve]` runs the webhook server; `receiver migrate` applies the
//! `connectors` schema migrations and exits (Helm pre-install/upgrade Job).

use std::process::ExitCode;

use enzarb_connector_github::GitHubConnector;
use enzarb_receiver::{Config, db};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    // reqwest has no default rustls provider (`rustls-no-provider`); install ring before any client.
    let _ = rustls::crypto::ring::default_provider().install_default();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let cmd = std::env::args().nth(1);
    let result = match cmd.as_deref() {
        None | Some("serve") => serve().await,
        Some("migrate") => migrate().await,
        Some("-h" | "--help" | "help") => {
            println!("usage: receiver [serve|migrate]");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("unknown command {other:?}; usage: receiver [serve|migrate]");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

type BoxErr = Box<dyn std::error::Error + Send + Sync>;

async fn migrate() -> Result<(), BoxErr> {
    let url = Config::database_url()?;
    let pool = db::connect(&url, 1).await?;
    db::migrate(&pool).await?;
    tracing::info!("connectors migrations applied");
    Ok(())
}

async fn serve() -> Result<(), BoxErr> {
    let cfg = Config::from_env()?;
    let pool = db::connect(&cfg.database_url, 10).await?;
    if cfg.secrets_key.is_none() {
        tracing::info!("ENZARB_SECRETS_KEY not set: only pending (unencrypted) event secrets work");
    }
    let state = enzarb_receiver::AppState::new(pool, cfg.webhook_secret, cfg.bot)
        .with_custom_events(cfg.events_secret_name, cfg.secrets_key);
    let state = match GitHubConnector::from_env() {
        Ok(c) => state.with_permissions(c),
        Err(e) => {
            tracing::info!("no GitHub App credentials ({e}): comment events carry can_write=false");
            state
        }
    };
    let app = enzarb_receiver::router(state);
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    tracing::info!(addr = %cfg.listen, "receiver listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
