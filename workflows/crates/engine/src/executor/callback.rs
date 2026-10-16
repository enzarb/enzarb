//! sandbox-init's run-finished callback (`contracts.md` section 4). **Deprecated**: only
//! sandboxes without `feature::RUN_RESULTS` get a `callback_url`; newer ones deliver results
//! over the mux ([`results`](super::results)). Removed once every sandbox has rolled
//! (`docs/gaps.md`). Ingestion is shared: [`Executor::ingest_run_finished`].
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
use enzarb_proto::RunFinished;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;

use super::Executor;
use super::finish::IngestError;

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
    match exec.ingest_run_finished(None, body).await {
        Ok(accepted) => (StatusCode::OK, Json(json!({ "accepted": accepted }))),
        Err(IngestError::BadRunId) => reply(StatusCode::BAD_REQUEST, "unrecognized run_id"),
        Err(IngestError::UnknownJob) => reply(StatusCode::NOT_FOUND, "unknown job"),
        Err(e) => {
            tracing::warn!(run_id = %path_run_id, error = %e, "callback: recording the result failed");
            reply(StatusCode::INTERNAL_SERVER_ERROR, "database error")
        }
    }
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
