//! HTTP routes.
//!
//! * `POST /hooks/github`: 401 bad/missing signature, 400 missing headers or
//!   malformed payload, 204 nothing to ingest (ping, unmapped action, bot
//!   events), 202 accepted (events ingested, duplicates are no-ops, or an
//!   installation change applied), 500 database failure (GitHub may redeliver).
//! * `POST /v1/events/<project>`: a signed project-declared event (see [`custom`]).
//! * `GET /healthz`: process is up. `GET /readyz`: database reachable.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use enzarb_connector_github::{BotIdentity, installation_change, verify_signature};
use enzarb_flow::events::{Custom, Event, Payload, SubjectRef};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

use crate::secrets::{self, SecretsKey};

/// GitHub caps webhook payloads at 25 MiB.
const MAX_BODY: usize = 26 * 1024 * 1024;

/// One catalog event ready for `engine_api.ingest`.
pub struct Ingest {
    pub dedupe_key: String,
    pub project: Option<String>,
    pub subject_key: Option<String>,
    pub event: Event,
}

#[derive(Clone)]
pub struct AppState(Arc<Inner>);

struct Inner {
    pool: PgPool,
    secret: Vec<u8>,
    bot: BotIdentity,
    events_secret_name: String,
    secrets_key: Option<SecretsKey>,
}

impl AppState {
    pub fn new(pool: PgPool, secret: Vec<u8>, bot: BotIdentity) -> Self {
        Self(Arc::new(Inner {
            pool,
            secret,
            bot,
            events_secret_name: crate::config::DEFAULT_EVENTS_SECRET_NAME.into(),
            secrets_key: None,
        }))
    }

    /// Custom-event settings: the project secret holding the HMAC key, and the
    /// key that decrypts stored secrets. Call before the state is cloned.
    pub fn with_custom_events(self, secret_name: String, key: Option<SecretsKey>) -> Self {
        let inner = Arc::into_inner(self.0).expect("with_custom_events before sharing the state");
        Self(Arc::new(Inner {
            events_secret_name: secret_name,
            secrets_key: key,
            ..inner
        }))
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/hooks/github", post(github))
        .route("/v1/events/{project}", post(custom))
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

/// `github:<owner>/<repo>#<n>`, lowercase.
fn subject_key(repo: Option<&str>, subject: Option<&SubjectRef>) -> Option<String> {
    Some(format!(
        "github:{}#{}",
        repo?.to_lowercase(),
        subject?.number
    ))
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

    match installation_change(event_name, delivery, &body) {
        Ok(None) => {}
        Ok(Some(change)) => {
            let res = async {
                let mut tx = st.0.pool.begin().await?;
                crate::installations::apply(&mut tx, &change).await?;
                tx.commit().await
            }
            .await;
            return match res {
                Ok(()) => StatusCode::ACCEPTED,
                Err(e) => {
                    tracing::error!(delivery, event_name, "installation: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            };
        }
        Err(e) => {
            tracing::warn!(delivery, event_name, "installation: {e}");
            return StatusCode::BAD_REQUEST;
        }
    }

    let mapped = match st.0.bot.map(event_name, delivery, &body) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(delivery, event_name, "map: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    if mapped.is_empty() {
        return StatusCode::NO_CONTENT;
    }
    let events = mapped
        .into_iter()
        .map(|(suffix, event)| Ingest {
            dedupe_key: format!("github:{delivery}:{suffix}"),
            project: None,
            subject_key: subject_key(event.repo.as_deref(), event.subject.as_ref()),
            event,
        })
        .collect();
    ingest_response(&st, delivery, event_name, events).await
}

/// Body of `POST /v1/events/<project>`.
#[derive(Deserialize)]
struct CustomBody {
    name: String,
    #[serde(default)]
    payload: serde_json::Value,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    subject: Option<SubjectRef>,
    #[serde(default)]
    repo: Option<String>,
}

/// A project-declared event: `X-Enzarb-Signature: sha256=<hex HMAC-SHA256 of the body>`
/// keyed by the project secret `ENZARB_EVENTS_SECRET_NAME`. 401 missing/bad signature,
/// 404 no such secret, 400 malformed body, 202 accepted (duplicates included), 500
/// database or decryption failure. The engine validates the payload against the
/// project's declared schema.
async fn custom(
    State(st): State<AppState>,
    Path(project): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(sig) = header(&headers, "x-enzarb-signature") else {
        return StatusCode::UNAUTHORIZED;
    };
    let row =
        match sqlx::query("SELECT ciphertext, pending FROM engine_api.secret_for_receiver($1, $2)")
            .bind(&project)
            .bind(&st.0.events_secret_name)
            .fetch_optional(&st.0.pool)
            .await
        {
            Ok(Some(r)) => r,
            Ok(None) => return StatusCode::NOT_FOUND,
            Err(e) => {
                tracing::error!(project, "secret lookup: {e}");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
        };
    let key = match secrets::plaintext(
        row.try_get("pending").ok().flatten(),
        row.try_get("ciphertext").ok().flatten(),
        st.0.secrets_key.as_ref(),
    ) {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(project, "events secret: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    if !verify_signature(&key, &body, sig) {
        return StatusCode::UNAUTHORIZED;
    }
    let req: CustomBody = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(project, "custom event body: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    if req.name.trim().is_empty() {
        return StatusCode::BAD_REQUEST;
    }
    let id = req
        .id
        .filter(|i| !i.is_empty())
        .unwrap_or_else(|| hex::encode(Sha256::digest(&body)));
    let repo = req.repo.map(|r| r.to_lowercase());
    let mut event = Event::new(Payload::Custom(Custom {
        name: req.name,
        payload: req.payload,
    }));
    event.id = Some(id.clone());
    event.repo = repo;
    event.subject = req.subject;
    event.at = Some(chrono_now());
    let ingest = Ingest {
        dedupe_key: format!("custom:{project}:{id}"),
        subject_key: subject_key(event.repo.as_deref(), event.subject.as_ref()),
        project: Some(project),
        event,
    };
    ingest_response(&st, &id, "custom", vec![ingest]).await
}

fn chrono_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn ingest_response(
    st: &AppState,
    delivery: &str,
    event_name: &str,
    events: Vec<Ingest>,
) -> StatusCode {
    match ingest(&st.0.pool, events).await {
        Ok((new, dup)) => {
            tracing::info!(delivery, event_name, new, dup, "ingested");
            StatusCode::ACCEPTED
        }
        Err(e) => {
            tracing::error!(delivery, event_name, "ingest: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// Ingests all events of one delivery in one transaction. Returns (new, duplicate).
async fn ingest(pool: &PgPool, events: Vec<Ingest>) -> Result<(usize, usize), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let (mut new, mut dup) = (0, 0);
    for ev in events {
        let payload = serde_json::to_value(&ev.event).expect("event serializes");
        let id: Option<i64> =
            sqlx::query_scalar("SELECT engine_api.ingest($1, $2, $3, $4, $5::jsonb)")
                .bind(&ev.dedupe_key)
                .bind(&ev.project)
                .bind(&ev.subject_key)
                .bind(ev.event.type_name())
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
