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
