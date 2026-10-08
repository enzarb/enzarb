//! Job results back into the event stream.

use serde_json::{Value, json};
use sqlx::PgPool;

use crate::error::Error;
use crate::machine::JobKind;

/// Outcome of a job attempt, as reported by an executor.
#[derive(Debug, Clone, PartialEq)]
pub enum JobResult {
    /// `output` is stored in `jobs.output` and carried by `JobSucceeded`.
    Succeeded { output: Value },
    /// Terminal failure for this job (retries with backoff are the executor's concern).
    Failed { error: String },
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
