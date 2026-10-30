//! `POST /v1/runs`: start a run of a named workflow on an issue or pull request by hand.
//!
//! Generic, with no knowledge of any workflow: the request becomes a synthetic `manual` event
//! (like GitHub's `workflow_dispatch`) whose payload carries the issue as `issue` and the person
//! as `sender.login`. The workflow must list `manual` in its `on`; its `if` is evaluated against
//! the event as for any other. The same one-live-run-per-subject-and-workflow rule as routed
//! runs applies. The caller (the UI) has already checked that the person may write to the repo.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use ez_workflow::{Event, RunInfo, RunState};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{ApiError, ApiResult, AppState, db_error, no_database};
use crate::github::SourceError;
use crate::router::{EventRow, evaluate};

#[derive(Deserialize)]
pub struct StartRun {
    /// `owner/repo`.
    pub repo: String,
    /// Issue or pull request number.
    pub number: u64,
    pub workflow: String,
    /// GitHub login of the person who asked (`sender.login`). Trusted as given: tracks is
    /// cluster-internal and bearer-authenticated, and the UI checks the person's write access
    /// live before calling. Tracks does not re-verify it.
    pub actor: String,
}

fn bad(code: &'static str, msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, code, msg)
}

fn source_error(e: SourceError) -> ApiError {
    match e {
        SourceError::Transient(m) => {
            ApiError::new(StatusCode::BAD_GATEWAY, "github_unavailable", m)
        }
        SourceError::Permanent(m) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "github", m),
    }
}

fn valid_repo(r: &str) -> bool {
    let ok = |s: &str| {
        !s.starts_with('.')
            && !s.is_empty()
            && s.len() <= 100
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    matches!(r.split_once('/'), Some((o, n)) if ok(o) && ok(n))
}

/// The `manual` event for `issue` (GitHub's issue object) requested by `actor`.
pub fn manual_event(repo: &str, issue: &Value, actor: &str) -> Event {
    let pick = |k: &str| issue.get(k).cloned().unwrap_or(Value::Null);
    let text = |k: &str| json!(issue.get(k).and_then(Value::as_str).unwrap_or_default());
    let mut i = json!({
        "number": pick("number"),
        "title": text("title"),
        "body": text("body"),
        "html_url": text("html_url"),
        "state": text("state"),
        "user": { "login": issue.pointer("/user/login").and_then(Value::as_str).unwrap_or_default() },
        "labels": issue.get("labels").cloned().unwrap_or(json!([])),
    });
    if issue.get("pull_request").is_some_and(|v| !v.is_null()) {
        i["pull_request"] = issue["pull_request"].clone();
    }
    Event::new(
        "manual",
        None,
        json!({
            "issue": i,
            "repository": { "full_name": repo },
            "sender": { "login": actor, "type": "User" },
        }),
    )
}

pub async fn start(State(s): State<AppState>, Json(req): Json<StartRun>) -> ApiResult<Value> {
    let pool = s.pool.as_ref().ok_or_else(no_database)?;
    let (Some(source), Some(projects)) = (&s.source, &s.projects) else {
        return Err(no_database());
    };
    if !valid_repo(&req.repo) || req.number == 0 || req.workflow.is_empty() {
        return Err(bad(
            "invalid_request",
            "give repo (owner/repo), number and workflow",
        ));
    }
    if req.actor.is_empty() || req.actor.len() > 100 {
        return Err(bad("invalid_request", "give the actor's login"));
    }
    let repo = req.repo.to_ascii_lowercase();
    let project = projects
        .owner(&repo)
        .await
        .map_err(|m| ApiError::new(StatusCode::BAD_GATEWAY, "unavailable", m))?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "no_project",
                "no Project owns this repository",
            )
        })?;
    let issue = source
        .issue(&repo, req.number)
        .await
        .map_err(source_error)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "no_such_issue",
                "there is no such issue or pull request",
            )
        })?;
    if issue.get("state").and_then(Value::as_str) == Some("closed") {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "issue_closed",
            "the issue or pull request is closed, so a workflow cannot be started on it",
        ));
    }
    let snap = source
        .snapshot(&repo, &project.default_branch)
        .await
        .map_err(source_error)?;
    let event = manual_event(&repo, &issue, &req.actor);
    let row = EventRow {
        id: 0,
        event: event.name.clone(),
        action: None,
        repo: Some(repo.clone()),
        payload: event.payload.clone(),
        received_at: None,
    };
    let (decision, file) = evaluate(&s.engine, &row, &project, &snap);
    if let Some(e) = decision.errors.first() {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_workflows",
            e.clone(),
        ));
    }
    let Some(file) = file else {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_workflows",
            "the repository defines no workflows",
        ));
    };
    let Some(reason) = decision.reasons.iter().find(|r| r.workflow == req.workflow) else {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "no_such_workflow",
            format!("the repository has no workflow `{}`", req.workflow),
        ));
    };
    if !reason.matched {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "not_startable",
            format!(
                "`{}` cannot be started by hand: {} (add \"manual\" to its on)",
                req.workflow, reason.reason
            ),
        ));
    }
    let subject = event.subject();
    let key = subject.as_ref().map(|x| x.key(&repo));

    let mut tx = pool.begin().await.map_err(db_error)?;
    if let Some(k) = &key {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(k)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        let live: Option<String> = sqlx::query_scalar(
            "SELECT id::text FROM tracks.runs
              WHERE subject_key = $1 AND workflow = $2 AND finished_at IS NULL AND NOT test LIMIT 1",
        )
        .bind(k)
        .bind(&req.workflow)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if let Some(id) = live {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "already_running",
                format!("{k} already has a live run of {}", req.workflow),
            )
            .with("run", json!(id)));
        }
    }
    let id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    let state = RunState::start(
        &file,
        &req.workflow,
        &event,
        RunInfo {
            id: id.clone(),
            attempt: 1,
            test: false,
        },
    )
    .map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "could_not_start",
            e.to_string(),
        )
    })?;
    let event_id: i64 = sqlx::query_scalar("SELECT -nextval('tracks.manual_event_seq')")
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    sqlx::query(
        "INSERT INTO tracks.runs (id, workflow, project, repo, subject_key, subject, event_id,
                                  sha, status, test, current_node, visits, state)
         VALUES ($1::uuid, $2, $3, $4, $5, $6, $7, $8, 'pending', false, $9, $10, $11)",
    )
    .bind(&id)
    .bind(&req.workflow)
    .bind(&project.name)
    .bind(&repo)
    .bind(&key)
    .bind(subject.as_ref().and_then(|x| serde_json::to_value(x).ok()))
    .bind(event_id)
    .bind(&snap.sha)
    .bind(state.current().map(str::to_string))
    .bind(serde_json::to_value(&state.visits).unwrap_or(json!({})))
    .bind(serde_json::to_value(&state).unwrap_or(Value::Null))
    .execute(&mut *tx)
    .await
    .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    Ok(Json(
        json!({ "run": { "id": id, "workflow": req.workflow, "project": project.name,
        "repo": repo, "event_id": event_id, "status": "pending" } }),
    ))
}
