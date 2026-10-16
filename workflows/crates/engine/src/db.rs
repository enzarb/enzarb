//! Pool and migrations for the `engine` schema (docs/workflows/contracts-v2.md).

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;

/// Schema that owns the engine tables and its `_sqlx_migrations` history.
pub const SCHEMA: &str = "engine";

/// Migrations embedded at build time from `migrations/`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// A pool whose connections default to `search_path = engine`.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let opts = PgConnectOptions::from_str(database_url)?.options([("search_path", SCHEMA)]);
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await
}

/// Apply pending migrations (history in `engine._sqlx_migrations`). Runs as the schema owner.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    let mut conn = pool.acquire().await?;
    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {SCHEMA}"))
        .execute(&mut *conn)
        .await?;
    sqlx::query(&format!("SET search_path TO {SCHEMA}"))
        .execute(&mut *conn)
        .await?;
    MIGRATOR.run(&mut *conn).await
}
