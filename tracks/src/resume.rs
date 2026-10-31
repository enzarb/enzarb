//! `POST /v1/runs/{id}/resume`: deliver a synthetic `manual` event to a run parked in a `wait`.
//!
//! Generic, like [`crate::manual`]: tracks knows nothing about plans or approvals. The request
//! `{actor, action, text?}` becomes an event named `manual` with `event.action = action`,
//! `event.text`, `event.sender.login = actor` and `event.issue` taken from the run. The wait's
//! own `on` and `if` decide whether it is accepted, exactly as for a routed event, and the run
//! is flipped back to `running` with the same `wait_result` the router writes, so the executor
//! finishes the wait through its usual path. The caller (the UI) has already checked that the
//! person may write to the repository.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ez_workflow::{Event, RunState, Subject};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::{ApiError, ApiResult, AppState, db_error, no_database};
use crate::github::{SourceError, WORKFLOWS_PATH};

#[derive(Deserialize)]
pub struct Resume {
    /// GitHub login of the person (`event.sender.login`); trusted as given, like `POST /v1/runs`.
    pub actor: String,
    /// `event.action`, e.g. `approve` or `revise`.
    pub action: String,
    /// Free text (`event.text`), e.g. review feedback.
    #[serde(default)]
    pub text: Option<String>,
}

fn err(status: StatusCode, code: &'static str, msg: impl Into<String>) -> ApiError {
    ApiError::new(status, code, msg)
}

fn not_waiting() -> ApiError {
    err(
        StatusCode::CONFLICT,
        "not_waiting",
        "the run is not waiting in a wait node",
    )
}

/// The `manual` event that resumes a run: the run's last issue when it has one, completed from
/// its subject.
pub fn resume_event(state: &RunState, repo: &str, req: &Resume) -> Event {
    let mut issue = state
        .event
        .get("issue")
        .filter(|i| i.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(s) = &state.subject {
        issue = subject_issue(issue, s);
    }
    Event::new(
        "manual",
        Some(req.action.clone()),
        json!({
            "action": req.action,
            "text": req.text.clone().unwrap_or_default(),
            "issue": issue,
            "repository": { "full_name": repo },
            "sender": { "login": req.actor, "type": "User" },
        }),
    )
}

fn subject_issue(mut issue: Value, s: &Subject) -> Value {
    issue["number"] = json!(s.number);
    if issue.pointer("/user/login").is_none() {
        issue["user"] = json!({ "login": s.author });
    }
    if issue.get("html_url").is_none() {
        issue["html_url"] = json!(s.url);
    }
    issue
}

pub async fn resume(
    State(s): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<Resume>,
) -> ApiResult<Value> {
    let pool = s.pool.as_ref().ok_or_else(no_database)?;
    let Some(source) = &s.source else {
        return Err(no_database());
    };
    if req.actor.is_empty()
        || req.actor.len() > 100
        || req.action.is_empty()
        || req.action.len() > 64
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "give the actor's login and an action",
        ));
    }
    if req.text.as_ref().is_some_and(|t| t.len() > 64 * 1024) {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "text is too long",
        ));
    }
    let not_found = || err(StatusCode::NOT_FOUND, "not_found", "there is no such run");
    if id.len() != 36 || !id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        return Err(not_found());
    }
    let row = sqlx::query(
        "SELECT repo, sha, state, status, wait_node_run_id FROM tracks.runs WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_optional(pool)
    .await
    .map_err(db_error)?
    .ok_or_else(not_found)?;
    let node_run: Option<i64> = row.get("wait_node_run_id");
    let (repo, sha): (String, String) = (row.get("repo"), row.get("sha"));
    let Some(node_run) = node_run.filter(|_| row.get::<String, _>("status") == "waiting") else {
        return Err(not_waiting());
    };
    let state: RunState = serde_json::from_value(row.get("state")).map_err(|e| {
        err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "corrupt_state",
            e.to_string(),
        )
    })?;
    let text = match source.file_at(&repo, &sha, WORKFLOWS_PATH).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return Err(err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "no_workflows",
                "the workflow file is gone at the run's pinned commit",
            ));
        }
        Err(SourceError::Transient(m)) => {
            return Err(err(StatusCode::BAD_GATEWAY, "github_unavailable", m));
        }
        Err(SourceError::Permanent(m)) => {
            return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "github", m));
        }
    };
    let file = ez_workflow::parse(&text).map_err(|e| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_workflows",
            e.to_string(),
        )
    })?;
    let event = resume_event(&state, &repo, &req);
    let not_accepted = |why: String| {
        err(StatusCode::UNPROCESSABLE_ENTITY, "not_accepted", why)
            .with("node", json!(state.current()))
    };
    match state.wait_accepts(&s.engine, &file, &event) {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => {
            return Err(not_accepted(format!(
                "the wait at `{}` does not accept `manual.{}` from {}",
                state.current().unwrap_or_default(),
                req.action,
                req.actor
            )));
        }
        Ok(Err(m)) => return Err(not_accepted(m)),
        Err(_) => return Err(not_waiting()),
    }

    let mut tx = pool.begin().await.map_err(db_error)?;
    let event_id: i64 = sqlx::query_scalar("SELECT -nextval('tracks.manual_event_seq')")
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    let res = sqlx::query(
        "UPDATE tracks.runs SET status = 'running', wait_until = NULL, updated_at = now(),
                wait_result = $3
          WHERE id = $1::uuid AND status = 'waiting' AND wait_node_run_id = $2",
    )
    .bind(&id)
    .bind(node_run)
    .bind(json!({
        "outputs": ez_workflow::wait_outputs(Some(&event)),
        "event_id": event_id,
        "resumed_by": req.actor,
        "action": req.action,
    }))
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    if res.rows_affected() != 1 {
        tx.rollback().await.map_err(db_error)?;
        return Err(not_waiting());
    }
    tx.commit().await.map_err(db_error)?;
    Ok(Json(json!({
        "resumed": { "run": id, "node": state.current(), "action": req.action, "event_id": event_id }
    })))
}
