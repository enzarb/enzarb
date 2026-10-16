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

fn stat_i64(v: Option<u64>) -> Option<i64> {
    v.and_then(|v| i64::try_from(v).ok())
}

/// Stores one `stats` reply, attributed to the step runs running on the Project at the time.
/// `false` when `(project, sandbox, at)` was already recorded.
pub async fn insert_sandbox_stats(
    pool: &PgPool,
    project: &str,
    sandbox: &str,
    s: &enzarb_proto::SandboxStats,
) -> Result<bool, sqlx::Error> {
    let at = i64::try_from(s.ts_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .ok_or_else(|| sqlx::Error::Encode("ts_ms out of range".into()))?;
    let done = sqlx::query(
        "INSERT INTO engine.sandbox_stats
            (project, sandbox, at, run_ids, cpu_usage_usec, cpu_user_usec, cpu_system_usec,
             memory_bytes, io_read_bytes, io_write_bytes, storage_used_bytes, storage_total_bytes)
         VALUES ($1, $2, $3,
            COALESCE((SELECT array_agg(run_id) FROM engine.step_runs
                      WHERE project = $1 AND started_at IS NOT NULL AND ended_at IS NULL), '{}'),
            $4, $5, $6, $7, $8, $9, $10, $11)
         ON CONFLICT (project, sandbox, at) DO NOTHING",
    )
    .bind(project)
    .bind(sandbox)
    .bind(at)
    .bind(stat_i64(s.cpu_usage_usec))
    .bind(stat_i64(s.cpu_user_usec))
    .bind(stat_i64(s.cpu_system_usec))
    .bind(stat_i64(s.memory_bytes))
    .bind(stat_i64(s.io_read_bytes))
    .bind(stat_i64(s.io_write_bytes))
    .bind(stat_i64(s.storage_used_bytes))
    .bind(stat_i64(s.storage_total_bytes))
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Deletes at most `batch` samples older than `older_than_days`.
pub async fn prune_sandbox_stats(
    pool: &PgPool,
    older_than_days: i32,
    batch: i64,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM engine.sandbox_stats WHERE (project, sandbox, at) IN (
             SELECT project, sandbox, at FROM engine.sandbox_stats
             WHERE at < now() - make_interval(days => $1) ORDER BY at LIMIT $2)",
    )
    .bind(older_than_days)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}
