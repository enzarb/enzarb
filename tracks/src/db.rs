//! Pool and migrations for schema `tracks`.

use std::str::FromStr;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

pub const SCHEMA: &str = "tracks";

/// Embedded migrations; history is kept in `tracks._sqlx_migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Pool whose connections default to `search_path = tracks`.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let opts = PgConnectOptions::from_str(database_url)?.options([("search_path", SCHEMA)]);
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await
}

/// Applies pending migrations. Run as the schema owner role.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    let mut conn = pool.acquire().await?;
    // Only create the schema when missing: `CREATE SCHEMA IF NOT EXISTS` needs database-level
    // CREATE even when the schema exists (same as the connectors migrator).
    sqlx::query(&format!(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = '{SCHEMA}') THEN \
             CREATE SCHEMA {SCHEMA}; \
           END IF; \
         END $$"
    ))
    .execute(&mut *conn)
    .await?;
    sqlx::query(&format!("SET search_path TO {SCHEMA}"))
        .execute(&mut *conn)
        .await?;
    MIGRATOR.run(&mut *conn).await
}
