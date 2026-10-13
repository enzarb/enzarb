//! `receiver [serve]` runs the webhook server; `receiver migrate` applies the
//! `connectors` schema migrations and exits (Helm pre-install/upgrade Job).

use std::process::ExitCode;

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
    let mut state = enzarb_receiver::AppState::new(pool, cfg.webhook_secret, cfg.bot);
    // Optional: with App credentials, `status` webhooks are mapped to PRs.
    if std::env::var_os("GITHUB_APP_ID").is_some() {
        state = state.with_connector(enzarb_connector_github::GitHubConnector::from_env()?);
    } else {
        tracing::info!("GITHUB_APP_ID not set: `status` webhooks will be ignored");
    }
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
