//! The `enzarb-workflows` binary: CLI, configuration and role wiring around the `engine` crate.

pub mod cli;
pub mod config;
pub mod health;
pub mod migrate;
pub mod run;

use cli::{Cli, Command, role_set};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Migrate(#[from] migrate::MigrateError),
    #[error("{0}")]
    Run(String),
}

/// JSON logs on stdout, filtered by `RUST_LOG` (default `info`).
pub fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .init();
}

pub async fn run_cli(cli: Cli) -> Result<(), Error> {
    match cli.command {
        Command::Migrate { require_roles } => {
            let url = std::env::var("DATABASE_URL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .ok_or(config::ConfigError::Missing("DATABASE_URL"))?;
            let pool = engine::db::connect(&url, 2)
                .await
                .map_err(|e| Error::Run(format!("connecting to database: {e}")))?;
            migrate::migrate(&pool, require_roles).await?;
            tracing::info!("migrations applied");
            Ok(())
        }
        Command::Run { roles } => {
            let cfg = config::Config::from_env(role_set(&roles))?;
            run::run(cfg).await
        }
    }
}
