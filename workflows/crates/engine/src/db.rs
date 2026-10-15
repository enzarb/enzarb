//! Database access: pool, migrations and typed rows for the `workflows` schema.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{FromRow, PgPool};
use std::str::FromStr;

/// Schema that owns the engine tables and its `_sqlx_migrations` history.
pub const SCHEMA: &str = "workflows";

/// Migrations embedded at build time from `migrations/`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// Build a pool whose connections default to `search_path = workflows`.
pub async fn connect(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let opts = PgConnectOptions::from_str(database_url)?.options([("search_path", SCHEMA)]);
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(opts)
        .await
}

/// Apply pending migrations. History is kept in `workflows._sqlx_migrations`.
///
/// Must run as the schema owner role. Creates the schema if pgop has not.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    let mut conn = pool.acquire().await?;
    sqlx::query(&format!("CREATE SCHEMA IF NOT EXISTS {SCHEMA}"))
        .execute(&mut *conn)
        .await?;
    // Session-local; unqualified `_sqlx_migrations` then resolves into `workflows`.
    sqlx::query(&format!("SET search_path TO {SCHEMA}"))
        .execute(&mut *conn)
        .await?;
    MIGRATOR.run(&mut *conn).await
}

#[derive(Debug, Clone, FromRow)]
pub struct Event {
    pub id: i64,
    pub source: String,
    pub dedupe_key: String,
    pub work_item_key: String,
    pub r#type: String,
    pub payload: Value,
    pub received_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct WorkItem {
    pub id: i64,
    pub key: String,
    pub project_id: Option<String>,
    pub workflow: String,
    pub workflow_version: i32,
    pub state: String,
    pub state_data: Value,
    pub wake_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Transition {
    pub id: i64,
    pub work_item_id: Option<i64>,
    pub from_state: Option<String>,
    pub to_state: String,
    pub event_id: Option<i64>,
    pub event_type: String,
    pub effects: Value,
    pub at: DateTime<Utc>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct Job {
    pub id: i64,
    pub work_item_id: i64,
    pub project_id: Option<String>,
    pub kind: String,
    pub priority: i32,
    pub idempotency_key: String,
    pub status: String,
    pub attempt: i32,
    pub max_attempts: i32,
    pub run_after: DateTime<Utc>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,
    pub input: Value,
    pub output: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub limit_wait_started_at: Option<DateTime<Utc>>,
    pub limit_wait_count: i32,
    pub restart_retries: i32,
    pub waiting_for: Option<String>,
    pub resumes_at: Option<DateTime<Utc>>,
    pub sandbox_wait_started_at: Option<DateTime<Utc>>,
}

/// Row of `workflows_api.work_item_v`.
#[derive(Debug, Clone, FromRow)]
pub struct WorkItemView {
    pub id: i64,
    pub key: String,
    pub project: Option<String>,
    pub workflow: String,
    pub state: String,
    pub state_data: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `session_limit` while a queued agent job waits for Claude's session limit to reset.
    pub waiting_for: Option<String>,
    pub resumes_at: Option<DateTime<Utc>>,
    /// Sum of the item's job usage (`enzarb_proto::Usage` shape); a field no job reported is
    /// absent. `None` when no job reported anything.
    pub usage_totals: Option<Value>,
    /// `human`, `enzarb`, `ci` or `github`; NULL in final states and before the first transition.
    pub waiting_on: Option<String>,
    pub wait_reason: Option<String>,
    pub wait_text: Option<String>,
}

/// Row of `workflows_api.transition_v`.
#[derive(Debug, Clone, FromRow)]
pub struct TransitionView {
    pub id: i64,
    pub work_item_id: Option<i64>,
    pub from_state: Option<String>,
    pub to_state: String,
    pub event_type: String,
    pub at: DateTime<Utc>,
}

/// Row of `workflows_api.job_v`.
#[derive(Debug, Clone, FromRow)]
pub struct JobView {
    pub id: i64,
    pub work_item_id: i64,
    pub kind: String,
    pub status: String,
    pub attempt: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub output: Option<Value>,
    pub waiting_for: Option<String>,
    pub resumes_at: Option<DateTime<Utc>>,
    /// Usage summed over the job's runs (`enzarb_proto::Usage` shape); `None` when not reported.
    pub usage: Option<Value>,
}

/// Per-`(project, sandbox)` row cap of `workflows.sandbox_stats` (ADR-0019 section 4).
pub const SANDBOX_STATS_ROW_CAP: i64 = 150_000;

/// Row of `workflows_api.sandbox_stats_window`. Rates are `None` when the bucket has no usable
/// interval, gauges when not reported.
#[derive(Debug, Clone, FromRow)]
pub struct SandboxStatsBucket {
    pub bucket_start: DateTime<Utc>,
    pub samples: i32,
    pub cpu_cores_avg: Option<f64>,
    pub cpu_cores_max: Option<f64>,
    pub mem_bytes_avg: Option<f64>,
    pub mem_bytes_max: Option<i64>,
    pub io_read_bps_avg: Option<f64>,
    pub io_read_bps_max: Option<f64>,
    pub io_write_bps_avg: Option<f64>,
    pub io_write_bps_max: Option<f64>,
    pub storage_used_bytes_avg: Option<f64>,
    pub storage_used_bytes_max: Option<i64>,
    pub storage_total_bytes: Option<i64>,
}

fn stat_i64(v: Option<u64>, name: &str) -> Result<Option<i64>, sqlx::Error> {
    v.map(|v| {
        i64::try_from(v).map_err(|_| sqlx::Error::Encode(format!("{name} out of range").into()))
    })
    .transpose()
}

/// Store one `stats` reply; `false` when `(project, sandbox, at)` was already recorded. A field
/// the sandbox did not report is stored as NULL.
pub async fn insert_sandbox_stats(
    pool: &PgPool,
    project: &str,
    sandbox: &str,
    work_item_id: Option<i64>,
    s: &enzarb_proto::SandboxStats,
) -> Result<bool, sqlx::Error> {
    let at = i64::try_from(s.ts_ms)
        .ok()
        .and_then(DateTime::from_timestamp_millis)
        .ok_or_else(|| sqlx::Error::Encode("ts_ms out of range".into()))?;
    let done = sqlx::query(
        "INSERT INTO workflows.sandbox_stats
            (project, sandbox, work_item_id, at, cpu_usage_usec, cpu_user_usec, cpu_system_usec,
             memory_bytes, io_read_bytes, io_write_bytes, storage_used_bytes, storage_total_bytes)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         ON CONFLICT (project, sandbox, at) DO NOTHING",
    )
    .bind(project)
    .bind(sandbox)
    .bind(work_item_id)
    .bind(at)
    .bind(stat_i64(s.cpu_usage_usec, "cpu_usage_usec")?)
    .bind(stat_i64(s.cpu_user_usec, "cpu_user_usec")?)
    .bind(stat_i64(s.cpu_system_usec, "cpu_system_usec")?)
    .bind(stat_i64(s.memory_bytes, "memory_bytes")?)
    .bind(stat_i64(s.io_read_bytes, "io_read_bytes")?)
    .bind(stat_i64(s.io_write_bytes, "io_write_bytes")?)
    .bind(stat_i64(s.storage_used_bytes, "storage_used_bytes")?)
    .bind(stat_i64(s.storage_total_bytes, "storage_total_bytes")?)
    .execute(pool)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Bucketed stats of a project over `[from, to)`. `interval_secs` and `retention_days` are the
/// poller's `ENZARB_STATS_INTERVAL` and `ENZARB_STATS_RETENTION` (they set the gap cutoff and the
/// window clamp).
pub async fn sandbox_stats_window(
    pool: &PgPool,
    project: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    buckets: i32,
    interval_secs: i32,
    retention_days: i32,
) -> Result<Vec<SandboxStatsBucket>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM workflows_api.sandbox_stats_window($1, $2, $3, $4, $5, $6)")
        .bind(project)
        .bind(from)
        .bind(to)
        .bind(buckets)
        .bind(interval_secs)
        .bind(retention_days)
        .fetch_all(pool)
        .await
}

/// Delete at most `batch` rows older than `older_than_days`; returns how many. Callers repeat
/// while a full batch comes back.
pub async fn prune_sandbox_stats(
    pool: &PgPool,
    older_than_days: i32,
    batch: i64,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM workflows.sandbox_stats WHERE (project, sandbox, at) IN (
             SELECT project, sandbox, at FROM workflows.sandbox_stats
             WHERE at < now() - make_interval(days => $1)
             ORDER BY at LIMIT $2)",
    )
    .bind(older_than_days)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}

/// Delete at most `batch` of the oldest rows beyond `cap` per `(project, sandbox)` (use
/// [`SANDBOX_STATS_ROW_CAP`]); returns how many. Callers repeat while a full batch comes back.
pub async fn prune_sandbox_stats_over_cap(
    pool: &PgPool,
    cap: i64,
    batch: i64,
) -> Result<u64, sqlx::Error> {
    let done = sqlx::query(
        "DELETE FROM workflows.sandbox_stats WHERE (project, sandbox, at) IN (
             SELECT project, sandbox, at FROM (
                 SELECT project, sandbox, at,
                        row_number() OVER (PARTITION BY project, sandbox ORDER BY at DESC) AS rn
                 FROM workflows.sandbox_stats) t
             WHERE rn > $1 LIMIT $2)",
    )
    .bind(cap)
    .bind(batch)
    .execute(pool)
    .await?;
    Ok(done.rows_affected())
}
