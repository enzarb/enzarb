//! Pool and migrations for schema `connectors`.

use std::str::FromStr;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

pub const SCHEMA: &str = "connectors";

/// Embedded migrations; history is kept in `connectors._sqlx_migrations`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Pool whose connections default to `search_path = connectors`.
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
    // `CREATE SCHEMA IF NOT EXISTS` checks the database-level CREATE privilege before it looks
    // for the schema, so it fails for the schema owner role when pgop already created the schema
    // but the role does not own the database. Only create the schema when it is missing.
    sqlx::query(&format!(
        "DO $$ BEGIN \
           IF NOT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = '{SCHEMA}') THEN \
             CREATE SCHEMA {SCHEMA}; \
           END IF; \
         END $$"
    ))
    .execute(&mut *conn)
    .await?;
    // Session-local; unqualified `_sqlx_migrations` then resolves into `connectors`.
    sqlx::query(&format!("SET search_path TO {SCHEMA}"))
        .execute(&mut *conn)
        .await?;
    MIGRATOR.run(&mut *conn).await
}
