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

/// One poll of one sandbox, as stored by `workflows_api.record_sandbox_sample`. CPU time and
/// disk bytes are monotonic counters; memory and storage are gauges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSample {
    pub project: String,
    /// The work item the sandbox serves; `None` while it is idle.
    pub work_item_id: Option<i64>,
    pub sandbox: String,
    pub sampled_at: DateTime<Utc>,
    pub cpu_usage_ns: i64,
    pub memory_bytes: i64,
    pub storage_bytes: i64,
    pub disk_read_bytes: i64,
    pub disk_write_bytes: i64,
}

/// Row of `workflows_api.sandbox_series`. Values are `None` in a bucket without samples (rates:
/// without a usable interval).
#[derive(Debug, Clone, FromRow)]
pub struct SandboxSeriesPoint {
    pub bucket: DateTime<Utc>,
    pub cpu_cores: Option<f64>,
    pub memory_bytes_avg: Option<f64>,
    pub memory_bytes_max: Option<i64>,
    pub storage_bytes_max: Option<i64>,
    pub disk_read_bps: Option<f64>,
    pub disk_write_bps: Option<f64>,
    pub samples: i32,
}

/// Store a sample; `false` when that (project, sandbox, sampled_at) was already recorded.
pub async fn record_sandbox_sample(pool: &PgPool, s: &SandboxSample) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT workflows_api.record_sandbox_sample($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(&s.project)
    .bind(s.work_item_id)
    .bind(&s.sandbox)
    .bind(s.sampled_at)
    .bind(s.cpu_usage_ns)
    .bind(s.memory_bytes)
    .bind(s.storage_bytes)
    .bind(s.disk_read_bytes)
    .bind(s.disk_write_bytes)
    .fetch_one(pool)
    .await
}

/// Bucketed series of a project over `[from, to)`.
pub async fn sandbox_series(
    pool: &PgPool,
    project: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    bucket: std::time::Duration,
) -> Result<Vec<SandboxSeriesPoint>, sqlx::Error> {
    sqlx::query_as(
        "SELECT * FROM workflows_api.sandbox_series($1, $2, $3, $4::bigint * interval '1 microsecond')",
    )
    .bind(project)
    .bind(from)
    .bind(to)
    .bind(i64::try_from(bucket.as_micros()).unwrap_or(i64::MAX))
    .fetch_all(pool)
    .await
}
