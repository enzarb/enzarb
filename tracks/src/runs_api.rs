//! `GET /v1/runs`, `GET /v1/runs/{id}` and `POST /v1/runs/{id}/stop` (ADR-0029 section 7).

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;

use crate::api::{ApiError, ApiResult, AppState, db_error, no_database};

const TS: &str = "'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"'";

#[derive(Deserialize)]
pub struct RunsQuery {
    project: Option<String>,
    repo: Option<String>,
    /// Comma-separated connector event ids: the runs those events started.
    event_ids: Option<String>,
    limit: Option<i64>,
}

fn run_select() -> String {
    format!(
        "SELECT id::text AS id, workflow, project, repo, subject, event_id, sha, status, test,
                current_node, reason, stop_requested, visits,
                to_char(created_at AT TIME ZONE 'UTC', {TS}) AS created_at,
                to_char(updated_at AT TIME ZONE 'UTC', {TS}) AS updated_at,
                to_char(finished_at AT TIME ZONE 'UTC', {TS}) AS finished_at
           FROM tracks.runs"
    )
}

fn run_json(r: &sqlx::postgres::PgRow) -> Value {
    json!({
        "id": r.get::<String, _>("id"),
        "workflow": r.get::<String, _>("workflow"),
        "project": r.get::<String, _>("project"),
        "repo": r.get::<String, _>("repo"),
        "subject": r.get::<Option<Value>, _>("subject"),
        "event_id": r.get::<i64, _>("event_id"),
        "sha": r.get::<String, _>("sha"),
        "status": r.get::<String, _>("status"),
        "test": r.get::<bool, _>("test"),
        "current_node": r.get::<Option<String>, _>("current_node"),
        "reason": r.get::<Option<String>, _>("reason"),
        "stop_requested": r.get::<bool, _>("stop_requested"),
        "visits": r.get::<Value, _>("visits"),
        "created_at": r.get::<String, _>("created_at"),
        "updated_at": r.get::<String, _>("updated_at"),
        "finished_at": r.get::<Option<String>, _>("finished_at"),
    })
}

fn bad(msg: &'static str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", msg)
}

fn not_found() -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", "there is no such run")
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Newest first, at most 200.
pub async fn list(State(s): State<AppState>, Query(q): Query<RunsQuery>) -> ApiResult<Value> {
    let pool = s.pool.as_ref().ok_or_else(no_database)?;
    let ids: Option<Vec<i64>> = match &q.event_ids {
        None => None,
        Some(v) => Some(
            v.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::parse::<i64>)
                .collect::<Result<_, _>>()
                .map_err(|_| bad("event_ids must be integers"))?,
        ),
    };
    if q.project.is_none() && q.repo.is_none() && ids.is_none() {
        return Err(bad("give project, repo or event_ids"));
    }
    let limit = q.limit.unwrap_or(100).clamp(1, 200);
    let rows = sqlx::query(&format!(
        "{} WHERE ($1::text IS NULL OR project = $1) AND ($2::text IS NULL OR repo = lower($2))
            AND ($3::bigint[] IS NULL OR event_id = ANY($3))
          ORDER BY created_at DESC LIMIT $4",
        run_select()
    ))
    .bind(&q.project)
    .bind(&q.repo)
    .bind(&ids)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    Ok(Json(
        json!({ "runs": rows.iter().map(run_json).collect::<Vec<_>>() }),
    ))
}

/// The run and its node runs.
pub async fn get(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    let pool = s.pool.as_ref().ok_or_else(no_database)?;
    if !is_uuid(&id) {
        return Err(not_found());
    }
    let row = sqlx::query(&format!("{} WHERE id = $1::uuid", run_select()))
        .bind(&id)
        .fetch_optional(pool)
        .await
        .map_err(db_error)?
        .ok_or_else(not_found)?;
    let nodes = sqlx::query(&format!(
        "SELECT node, visit, status, rendered, mode, outputs, usage, git, error,
                to_char(started_at AT TIME ZONE 'UTC', {TS}) AS started_at,
                to_char(finished_at AT TIME ZONE 'UTC', {TS}) AS finished_at
           FROM tracks.node_runs WHERE run_id = $1::uuid ORDER BY id"
    ))
    .bind(&id)
    .fetch_all(pool)
    .await
    .map_err(db_error)?;
    let node_runs: Vec<Value> = nodes
        .iter()
        .map(|n| {
            json!({
                "node": n.get::<String, _>("node"),
                "visit": n.get::<i32, _>("visit"),
                "status": n.get::<String, _>("status"),
                "rendered": n.get::<Option<Value>, _>("rendered"),
                "mode": n.get::<Option<String>, _>("mode"),
                "outputs": n.get::<Option<Value>, _>("outputs"),
                "usage": n.get::<Option<Value>, _>("usage"),
                "git": n.get::<Option<Value>, _>("git"),
                "error": n.get::<Option<String>, _>("error"),
                "started_at": n.get::<Option<String>, _>("started_at"),
                "finished_at": n.get::<Option<String>, _>("finished_at"),
            })
        })
        .collect();
    let mut run = run_json(&row);
    run["node_runs"] = Value::Array(node_runs);
    Ok(Json(json!({ "run": run })))
}

/// The executor cancels the run at its next check and stops the running step. Idempotent; a
/// finished run answers 409.
pub async fn stop(State(s): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    let pool = s.pool.as_ref().ok_or_else(no_database)?;
    if !is_uuid(&id) {
        return Err(not_found());
    }
    let r = sqlx::query(
        "UPDATE tracks.runs SET stop_requested = true, updated_at = now()
          WHERE id = $1::uuid AND finished_at IS NULL",
    )
    .bind(&id)
    .execute(pool)
    .await
    .map_err(db_error)?;
    if r.rows_affected() == 0 {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tracks.runs WHERE id = $1::uuid)")
                .bind(&id)
                .fetch_one(pool)
                .await
                .map_err(db_error)?;
        return Err(if exists {
            ApiError::new(
                StatusCode::CONFLICT,
                "run_finished",
                "the run has already ended",
            )
        } else {
            not_found()
        });
    }
    Ok(Json(json!({ "stopping": true })))
}
