//! HTTP routes.
//!
//! * `POST /hooks/github`: 401 bad/missing signature, 400 missing headers or
//!   malformed payload, 204 nothing to ingest (ping, unmapped action, bot
//!   events), 202 accepted (events ingested, duplicates are no-ops, or an
//!   installation change applied), 500 database failure (GitHub may redeliver).
//! * `GET /healthz`: process is up. `GET /readyz`: database reachable.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use enzarb_connector::Event;
use enzarb_connector_github::{BotIdentity, Normalized, verify_signature};
use sqlx::PgPool;

/// GitHub caps webhook payloads at 25 MiB.
const MAX_BODY: usize = 26 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pool: PgPool,
    secret: Vec<u8>,
    bot: BotIdentity,
}

impl AppState {
    pub fn new(pool: PgPool, secret: Vec<u8>, bot: BotIdentity) -> Self {
        Self(Arc::new(Inner { pool, secret, bot }))
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
    let (Some(event_name), Some(delivery)) = (
        header(&headers, "x-github-event"),
        header(&headers, "x-github-delivery"),
    ) else {
        return StatusCode::BAD_REQUEST;
    };

    let normalized = match st.0.bot.normalize(event_name, delivery, &body) {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(delivery, event_name, "normalize: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    match normalized {
        Normalized::Events { events } if events.is_empty() => StatusCode::NO_CONTENT,
        Normalized::Events { events } => match ingest(&st.0.pool, events).await {
            Ok((new, dup)) => {
                tracing::info!(delivery, event_name, new, dup, "ingested");
                StatusCode::ACCEPTED
            }
            Err(e) => {
                tracing::error!(delivery, event_name, "ingest: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        },
        Normalized::Installation { change } => {
            let res = async {
                let mut tx = st.0.pool.begin().await?;
                crate::installations::apply(&mut tx, &change).await?;
                tx.commit().await
            }
            .await;
            match res {
                Ok(()) => StatusCode::ACCEPTED,
                Err(e) => {
                    tracing::error!(delivery, event_name, "installation: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            }
        }
    }
}

/// Repo keys and subjects are lowercase in `workflows` and the Project CRDs.
fn lowercase_keys(e: &mut Event) {
    e.data.repo.owner = e.data.repo.owner.to_lowercase();
    e.data.repo.repo = e.data.repo.repo.to_lowercase();
    e.subject.repo = e.data.repo.clone();
}

/// Ingests all events of one delivery in one transaction. Returns (new, duplicate).
async fn ingest(pool: &PgPool, events: Vec<Event>) -> Result<(usize, usize), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let (mut new, mut dup) = (0, 0);
    for mut ev in events {
        lowercase_keys(&mut ev);
        let payload = serde_json::to_value(&ev).expect("event serializes");
        let id: Option<i64> =
            sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
                .bind(&ev.source)
                .bind(ev.dedupe_key())
                .bind(ev.subject.to_string())
                .bind(ev.event_type.as_str())
                .bind(payload)
                .fetch_one(&mut *tx)
                .await?;
        if id.is_some() {
            new += 1;
        } else {
            dup += 1;
        }
    }
    tx.commit().await?;
    Ok((new, dup))
}
