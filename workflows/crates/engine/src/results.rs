//! Job results back into the event stream.

use enzarb_proto::Usage;
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::error::Error;
use crate::machine::JobKind;
use crate::usage;

/// Outcome of a job attempt, as reported by an executor.
#[derive(Debug, Clone, PartialEq)]
pub enum JobResult {
    /// `output` is stored in `jobs.output` and carried by `JobSucceeded`.
    Succeeded { output: Value },
    /// Terminal failure for this job (retries with backoff are the executor's concern).
    Failed { error: String },
}

/// Add the usage of run `run_id` to `jobs.usage` (see [`crate::usage::merge`]).
///
/// Counted once per `run_id`: a repeated callback (sandbox-init retries, duplicate delivery) is
/// ignored, as is an empty `usage`. Applies whatever the job's status, because the spend happened
/// even if the job was cancelled meanwhile. Returns whether the usage was added.
pub async fn record_job_usage(
    pool: &PgPool,
    job_id: i64,
    run_id: &str,
    usage: &Usage,
) -> Result<bool, Error> {
    if usage.is_empty() {
        return Ok(false);
    }
    let mut tx = pool.begin().await?;
    let row: Option<(Option<Value>, Vec<String>)> =
        sqlx::query_as("SELECT usage, usage_runs FROM jobs WHERE id = $1 FOR UPDATE")
            .bind(job_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((current, runs)) = row else {
        return Err(Error::JobNotFound(job_id));
    };
    if runs.iter().any(|r| r == run_id) {
        return Ok(false);
    }
    // An unreadable stored value is treated as absent rather than blocking the callback.
    let current = current.and_then(|v| serde_json::from_value::<Usage>(v).ok());
    let merged = serde_json::to_value(usage::merge(current, usage))
        .map_err(|e| Error::State(format!("job {job_id}: cannot encode usage: {e}")))?;
    sqlx::query(
        "UPDATE jobs SET usage = $2, usage_runs = array_append(usage_runs, $3) WHERE id = $1",
    )
    .bind(job_id)
    .bind(merged)
    .bind(run_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Record the result of `attempt` of job `job_id` and queue the matching engine event.
///
/// In one transaction: lock the job, finish it (`succeeded` / `failed`, `output`, lease
/// cleared) and insert `JobSucceeded {job_id, kind, output}` or `JobFailed {job_id, kind,
/// error}` with dedupe key `job:<job_id>:<attempt>` and the work item's key
/// (`workflows_api.ingest_event`, which also sends `NOTIFY workflow_events`).
///
/// Returns the new event id, or `None` when the result is stale and was dropped: the job is
/// no longer `running` (cancelled by a transition, or already reported) or `attempt` is not
/// its current attempt. That makes the call idempotent and safe against late executors.
pub async fn ingest_job_result(
    pool: &PgPool,
    job_id: i64,
    attempt: i32,
    result: JobResult,
) -> Result<Option<i64>, Error> {
    let mut tx = pool.begin().await?;
    let row: Option<(String, String, i32, String)> = sqlx::query_as(
        "SELECT j.status, j.kind, j.attempt, w.key
         FROM jobs j JOIN work_items w ON w.id = j.work_item_id
         WHERE j.id = $1 FOR UPDATE OF j",
    )
    .bind(job_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((status, kind, current, key)) = row else {
        return Err(Error::JobNotFound(job_id));
    };
    if status != "running" || current != attempt {
        return Ok(None);
    }
    let kind: JobKind = serde_json::from_value(Value::String(kind.clone()))
        .map_err(|_| Error::State(format!("job {job_id} has unknown kind {kind:?}")))?;

    let (new_status, ty, payload, output) = match result {
        JobResult::Succeeded { output } => (
            "succeeded",
            "JobSucceeded",
            json!({ "job_id": job_id, "kind": kind, "output": output }),
            Some(output),
        ),
        JobResult::Failed { error } => (
            "failed",
            "JobFailed",
            json!({ "job_id": job_id, "kind": kind, "error": error }),
            None,
        ),
    };
    sqlx::query(
        "UPDATE jobs SET status = $2, output = COALESCE($3, output), lease_owner = NULL,
                lease_until = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(job_id)
    .bind(new_status)
    .bind(output)
    .execute(&mut *tx)
    .await?;
    let id: Option<i64> =
        sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
            .bind("engine")
            .bind(format!("job:{job_id}:{attempt}"))
            .bind(key)
            .bind(ty)
            .bind(payload)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(id)
}
