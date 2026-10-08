//! sandbox-init's run-finished callback (`contracts.md` section 4).
//!
//! `POST /v1/runs/{run_id}/finished` with `Authorization: Bearer <callback_token>` and a
//! `RunFinished` body. `run_id` is `job-<job id>`. The token is
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
use crate::results::{JobResult, ingest_job_result};

type HmacSha256 = Hmac<Sha256>;

/// `run_id` for a job.
pub fn run_id(job_id: i64) -> String {
    format!("job-{job_id}")
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
        .and_then(|n| n.parse::<i64>().ok())
    else {
        return reply(StatusCode::BAD_REQUEST, "unrecognized run_id");
    };

    let row: Result<Option<(String, i32)>, _> =
        sqlx::query_as("SELECT status, attempt FROM jobs WHERE id = $1 AND kind LIKE 'agent\\_%'")
            .bind(job_id)
            .fetch_optional(&exec.pool)
            .await;
    let (status, attempt) = match row {
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

    let result = match body.outcome {
        RunOutcome::Succeeded => JobResult::Succeeded {
            output: serde_json::to_value(&body).unwrap_or(Value::Null),
        },
        RunOutcome::Failed => JobResult::Failed {
            error: format!(
                "the agent run failed (exit code {}): {}",
                body.exit_code.map_or("none".into(), |c| c.to_string()),
                truncate(&body.summary, 2000)
            ),
        },
        RunOutcome::Cancelled => JobResult::Failed {
            error: "the agent run was cancelled".into(),
        },
    };
    match ingest_job_result(&exec.pool, job_id, attempt, result).await {
        Ok(id) => (StatusCode::OK, Json(json!({ "accepted": id.is_some() }))),
        Err(e) => {
            tracing::warn!(job_id, error = %e, "callback: recording the result failed");
            reply(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
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
