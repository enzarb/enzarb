//! sandbox-init's run-finished callback (`contracts.md` section 4).
//!
//! `POST /v1/runs/{run_id}/finished` with `Authorization: Bearer <callback_token>` and a
//! `RunFinished` body. `run_id` is `job-<job id>` (`job-<job id>-<n>` after `n` session-limit
//! waits and sandbox-restart retries; a `-r1` suffix marks the one recovery run started when a
//! finished run's output had no usable `json` block). The token is
//! `hex(HMAC-SHA256(callback_secret, run_id))`, so any replica can verify it and nothing
//! per-job is stored.
//!
//! Responses: `200` once the result is recorded **or** is stale (job not running any more),
//! which stops sandbox-init's retries; `401` bad token; `400` malformed; `404` unknown job;
//! `500` database trouble (sandbox-init retries).

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use enzarb_proto::{RunFinished, RunOutcome};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use super::Executor;
use super::restart::RestartOutcome;
use super::session_limit::LimitOutcome;
use crate::machine::{AgentJobInput, AgentOutput, JobKind, OutputError};
use crate::results::{JobResult, ingest_job_result};

type HmacSha256 = Hmac<Sha256>;

/// `run_id` for a job. After `waits` session-limit waits and sandbox-restart retries (the run
/// generation) the run is a new one (`job-<id>-<waits>`), because the sandbox dedupes
/// `run_start` by `run_id` and would hand back the earlier run.
pub fn run_id(job_id: i64, waits: i32) -> String {
    if waits <= 0 {
        format!("job-{job_id}")
    } else {
        format!("job-{job_id}-{waits}")
    }
}

/// `run_id` of the job's one recovery run (see [`run_id`] for the first part).
pub fn recovery_run_id(job_id: i64, waits: i32) -> String {
    format!("{}-r1", run_id(job_id, waits))
}

/// Has the job's recovery run been started (`input.recovery_used`)?
pub(super) fn recovery_used(input: &Value) -> bool {
    input
        .get("recovery_used")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The columns the finished callback reads (`generation` = session-limit waits + restarts).
#[derive(sqlx::FromRow)]
struct CallbackJob {
    status: String,
    attempt: i32,
    generation: i32,
    project_id: Option<String>,
    input: Value,
}

/// The run whose callback finishes the job: the recovery run once it was started.
pub(super) fn current_run_id(job_id: i64, waits: i32, recovery_used: bool) -> String {
    if recovery_used {
        recovery_run_id(job_id, waits)
    } else {
        run_id(job_id, waits)
    }
}

fn mac(secret: &[u8], run_id: &str) -> HmacSha256 {
    let mut m = HmacSha256::new_from_slice(secret).expect("hmac accepts any key length");
    m.update(b"enzarb-run-callback:");
    m.update(run_id.as_bytes());
    m
}

/// The bearer token sandbox-init must present for `run_id`.
pub fn callback_token(secret: &[u8], run_id: &str) -> String {
    mac(secret, run_id)
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn verify(secret: &[u8], run_id: &str, token: &str) -> bool {
    let Some(bytes) = unhex(token) else {
        return false;
    };
    mac(secret, run_id).verify_slice(&bytes).is_ok()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// The callback and health routes.
pub fn router(exec: Arc<Executor>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/runs/{run_id}/finished", post(finished))
        .with_state(exec)
}

fn reply(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg })))
}

async fn finished(
    State(exec): State<Arc<Executor>>,
    Path(path_run_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<RunFinished>,
) -> (StatusCode, Json<Value>) {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !verify(&exec.cfg.callback_secret, &path_run_id, token) {
        return reply(StatusCode::UNAUTHORIZED, "bad callback token");
    }
    if body.run_id != path_run_id {
        return reply(StatusCode::BAD_REQUEST, "run_id does not match the path");
    }
    let Some(job_id) = path_run_id
        .strip_prefix("job-")
        .map(|n| n.split_once('-').map_or(n, |(id, _)| id))
        .and_then(|n| n.parse::<i64>().ok())
    else {
        return reply(StatusCode::BAD_REQUEST, "unrecognized run_id");
    };

    let row: Result<Option<CallbackJob>, _> = sqlx::query_as(
        "SELECT status, attempt, limit_wait_count + restart_retries AS generation, project_id, input
         FROM jobs WHERE id = $1 AND kind LIKE 'agent\\_%'",
    )
    .bind(job_id)
    .fetch_optional(&exec.pool)
    .await;
    let CallbackJob {
        status,
        attempt,
        generation,
        project_id: project,
        input,
    } = match row {
        Ok(Some(r)) => r,
        Ok(None) => return reply(StatusCode::NOT_FOUND, "unknown job"),
        Err(e) => {
            tracing::warn!(job_id, error = %e, "callback: job lookup failed");
            return reply(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    };
    if status != "running" {
        tracing::info!(job_id, %status, "callback for a job that is not running; ignored");
        return (StatusCode::OK, Json(json!({ "accepted": false })));
    }
    let recovery_used = recovery_used(&input);
    if path_run_id != current_run_id(job_id, generation, recovery_used) {
        tracing::info!(job_id, %path_run_id, "callback for an earlier run of this job; ignored");
        return (StatusCode::OK, Json(json!({ "accepted": false })));
    }

    let result = match body.outcome {
        RunOutcome::Succeeded => {
            let output = serde_json::to_value(&body).unwrap_or(Value::Null);
            match unparseable_output(&input, &output) {
                None => JobResult::Succeeded { output },
                Some(e) => {
                    let error = format!(
                        "unusable agent output: {e}; tail of output: {}",
                        tail(&body.summary, TAIL_BYTES)
                    );
                    if recovery_used {
                        JobResult::Failed { error }
                    } else {
                        match recover(
                            &exec,
                            job_id,
                            attempt,
                            generation,
                            project.as_deref(),
                            &input,
                        )
                        .await
                        {
                            Ok(Recovery::Started) => {
                                return (StatusCode::OK, Json(json!({ "accepted": true })));
                            }
                            Ok(Recovery::Stale) => {
                                return (StatusCode::OK, Json(json!({ "accepted": false })));
                            }
                            Ok(Recovery::CouldNotStart) => JobResult::Failed { error },
                            Err(e) => {
                                tracing::warn!(job_id, error = %e, "callback: claiming the recovery failed");
                                return reply(StatusCode::INTERNAL_SERVER_ERROR, "database error");
                            }
                        }
                    }
                }
            }
        }
        RunOutcome::Failed => {
            let error = format!(
                "the agent run failed (exit code {}): {}",
                body.exit_code.map_or("none".into(), |c| c.to_string()),
                truncate(&body.summary, 2000)
            );
            match exec
                .session_limit_outcome(job_id, attempt, &body.summary, &error)
                .await
            {
                Ok(LimitOutcome::Waiting) => {
                    return (StatusCode::OK, Json(json!({ "accepted": true })));
                }
                Ok(LimitOutcome::Stale) => {
                    return (StatusCode::OK, Json(json!({ "accepted": false })));
                }
                Ok(LimitOutcome::NotALimit) => JobResult::Failed { error },
                Ok(LimitOutcome::GaveUp(error)) => JobResult::Failed { error },
                Err(e) => {
                    tracing::warn!(job_id, error = %e, "callback: session limit handling failed");
                    return reply(StatusCode::INTERNAL_SERVER_ERROR, "database error");
                }
            }
        }
        RunOutcome::Cancelled => {
            // The job is still `running`, so no transition cancelled it: the sandbox restarted.
            let reason = "the agent stopped with cancelled";
            match exec.restart_outcome(job_id, attempt, reason).await {
                Ok(RestartOutcome::Requeued) => {
                    return (StatusCode::OK, Json(json!({ "accepted": true })));
                }
                Ok(RestartOutcome::Stale) => {
                    return (StatusCode::OK, Json(json!({ "accepted": false })));
                }
                Ok(RestartOutcome::GaveUp(error)) => JobResult::Failed { error },
                Err(e) => {
                    tracing::warn!(job_id, error = %e, "callback: restart handling failed");
                    return reply(StatusCode::INTERNAL_SERVER_ERROR, "database error");
                }
            }
        }
    };
    match ingest_job_result(&exec.pool, job_id, attempt, result).await {
        Ok(id) => (StatusCode::OK, Json(json!({ "accepted": id.is_some() }))),
        Err(e) => {
            tracing::warn!(job_id, error = %e, "callback: recording the result failed");
            reply(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
}

/// How much of the end of an unusable output goes into the error.
const TAIL_BYTES: usize = 2048;

enum Recovery {
    /// The recovery run was started; the job stays `running`.
    Started,
    /// The attempt is no longer running, or another callback already claimed the recovery.
    Stale,
    /// The run could not be started; fail the job.
    CouldNotStart,
}

/// The parse error if `output` of the agent job `input` has no usable `json` block (missing, or
/// not valid JSON). Other problems (unknown decision, empty body) are not recoverable by
/// asking again for the block.
fn unparseable_output(input: &Value, output: &Value) -> Option<OutputError> {
    let kind = serde_json::from_value::<JobKind>(input.get("kind")?.clone()).ok()?;
    match AgentOutput::from_job_output(kind, output) {
        Err(e @ (OutputError::NoJsonBlock | OutputError::BadJson(_))) => Some(e),
        _ => None,
    }
}

/// Claim the job's single recovery and start the recovery run.
async fn recover(
    exec: &Executor,
    job_id: i64,
    attempt: i32,
    generation: i32,
    project: Option<&str>,
    input: &Value,
) -> Result<Recovery, sqlx::Error> {
    let claimed = sqlx::query(
        "UPDATE jobs SET input = jsonb_set(input, '{recovery_used}', 'true'), updated_at = now()
         WHERE id = $1 AND status = 'running' AND attempt = $2
           AND NOT COALESCE((input->>'recovery_used')::bool, false)",
    )
    .bind(job_id)
    .bind(attempt)
    .execute(&exec.pool)
    .await?;
    if claimed.rows_affected() == 0 {
        return Ok(Recovery::Stale);
    }
    let (Some(project), Ok(input)) = (
        project,
        serde_json::from_value::<AgentJobInput>(input.clone()),
    ) else {
        return Ok(Recovery::CouldNotStart);
    };
    match exec
        .start_recovery(job_id, attempt, generation, project, &input)
        .await
    {
        Ok(()) => Ok(Recovery::Started),
        Err(e) => {
            tracing::warn!(job_id, error = %e.message, "could not start the recovery run");
            Ok(Recovery::CouldNotStart)
        }
    }
}

/// The last `max` bytes of `s`, cut on a char boundary and prefixed with `…` when cut.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_run_ids_are_distinct_and_bound_by_the_token() {
        assert_eq!(current_run_id(7, 0, false), "job-7");
        assert_eq!(current_run_id(7, 0, true), "job-7-r1");
        assert_eq!(current_run_id(7, 2, true), "job-7-2-r1");
        let t = callback_token(b"s3", "job-7");
        assert!(!verify(b"s3", "job-7-r1", &t));
        assert!(verify(
            b"s3",
            "job-7-r1",
            &callback_token(b"s3", "job-7-r1")
        ));
    }

    #[test]
    fn tail_keeps_the_end_on_char_boundaries() {
        assert_eq!(tail("short", 10), "short");
        assert_eq!(tail("abcdef", 3), "…def");
        // "é" is two bytes: cutting inside it moves forward to the next boundary.
        assert_eq!(tail("aéb", 2), "…b");
        assert_eq!(tail("aéb", 3), "…éb");
        assert_eq!(tail("", 3), "");
    }

    #[test]
    fn only_missing_or_bad_blocks_are_unparseable() {
        let input = json!({ "kind": "agent_triage" });
        let out = |s: &str| json!({ "summary": s });
        assert!(unparseable_output(&input, &out("no block")).is_some());
        assert!(unparseable_output(&input, &out("```json\n{\"decision\": 1}\n```")).is_some());
        let ok = "```json\n{\"decision\":\"ready\",\"body\":\"b\"}\n```";
        assert!(unparseable_output(&input, &out(ok)).is_none());
        let unknown = "```json\n{\"decision\":\"nope\",\"body\":\"b\"}\n```";
        assert!(unparseable_output(&input, &out(unknown)).is_none());
        assert!(unparseable_output(&input, &json!({})).is_none());
        assert!(unparseable_output(&json!({}), &out("no block")).is_none());
    }

    #[test]
    fn run_id_generations() {
        assert_eq!(run_id(7, 0), "job-7");
        assert_eq!(run_id(7, 1), "job-7-1");
        assert_eq!(run_id(7, 4), "job-7-4");
    }

    #[test]
    fn tokens_bind_secret_and_run() {
        let t = callback_token(b"s3", "job-7");
        assert!(verify(b"s3", "job-7", &t));
        assert!(!verify(b"s3", "job-8", &t));
        assert!(!verify(b"other", "job-7", &t));
        assert!(!verify(b"s3", "job-7", ""));
        assert!(!verify(b"s3", "job-7", "zz"));
        assert!(!verify(b"s3", "job-7", "é"));
    }
}
