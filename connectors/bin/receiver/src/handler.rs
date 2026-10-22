//! HTTP routes.
//!
//! * `POST /hooks/github`: 401 bad/missing signature, 400 missing headers or a body that is not
//!   JSON, 202 logged (a redelivery of a known delivery id is a no-op), 500 database failure
//!   (GitHub may redeliver). Nothing else happens to the event.
//! * `GET /healthz`: process is up. `GET /readyz`: database reachable.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use serde_json::Value;
use sqlx::PgPool;

use crate::signature::verify_signature;

/// GitHub caps webhook payloads at 25 MiB.
const MAX_BODY: usize = 26 * 1024 * 1024;

/// Postgres channel notified with the new row id of `connectors.events`.
pub const NOTIFY_CHANNEL: &str = "connector_event";

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pool: PgPool,
    secret: Vec<u8>,
}

impl AppState {
    pub fn new(pool: PgPool, secret: Vec<u8>) -> Self {
        Self(Arc::new(Inner { pool, secret }))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/hooks/github", post(github))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

async fn readyz(State(st): State<AppState>) -> StatusCode {
    match sqlx::query("SELECT 1").execute(&st.0.pool).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::warn!("readyz: {e}");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
}

async fn github(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let Some(sig) = header(&headers, "x-hub-signature-256") else {
        return StatusCode::UNAUTHORIZED;
    };
    if !verify_signature(&st.0.secret, &body, sig) {
        return StatusCode::UNAUTHORIZED;
    }
    let (Some(event), Some(delivery)) = (
        header(&headers, "x-github-event"),
        header(&headers, "x-github-delivery"),
    ) else {
        return StatusCode::BAD_REQUEST;
    };
    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };

    let str_at = |path: &[&str]| {
        path.iter()
            .try_fold(&payload, |v, k| v.get(k))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    // Repo keys are lowercase, matching the Project CRs.
    let repo = str_at(&["repository", "full_name"]).map(|r| r.to_lowercase());
    let action = str_at(&["action"]);
    let sender = str_at(&["sender", "login"]);

    match log_event(&st.0.pool, delivery, event, action, repo, sender, payload).await {
        Ok(new) => {
            tracing::info!(delivery, event, new, "logged");
            StatusCode::ACCEPTED
        }
        Err(e) => {
            tracing::error!(delivery, event, "log: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Inserts the delivery and notifies listeners, in one transaction. Returns false for a duplicate.
async fn log_event(
    pool: &PgPool,
    delivery: &str,
    event: &str,
    action: Option<String>,
    repo: Option<String>,
    sender: Option<String>,
    payload: Value,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO events (delivery_id, event, action, repo, sender, payload) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (source, delivery_id) DO NOTHING RETURNING id",
    )
    .bind(delivery)
    .bind(event)
    .bind(action)
    .bind(repo)
    .bind(sender)
    .bind(payload)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = id {
        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(NOTIFY_CHANNEL)
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(id.is_some())
}
