use axum::{
    Json,
    extract::{Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{AppState, process::ProcessKind};

#[derive(Debug, Deserialize)]
pub struct CreateProcessRequest {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub kind: ProcessKindReq,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProcessKindReq {
    #[default]
    OneShot,
    Persistent,
}

#[derive(Debug, Serialize)]
pub struct ProcessResponse {
    pub id: String,
    pub name: String,
    pub command: String,
    pub kind: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl From<crate::process::Process> for ProcessResponse {
    fn from(p: crate::process::Process) -> Self {
        ProcessResponse {
            id: p.id,
            name: p.name,
            command: p.command,
            kind: format!("{:?}", p.kind).to_lowercase(),
            status: format!("{:?}", p.status).to_lowercase(),
            exit_code: p.exit_code,
            started_at: p.started_at.to_rfc3339(),
            finished_at: p.finished_at.map(|t| t.to_rfc3339()),
        }
    }
}

pub async fn create(
    State(state): State<AppState>,
    Json(req): Json<CreateProcessRequest>,
) -> Result<Json<ProcessResponse>, StatusCode> {
    let kind = match req.kind {
        ProcessKindReq::Persistent => ProcessKind::Persistent,
        ProcessKindReq::OneShot => ProcessKind::OneShot,
    };

    let process = state
        .process_store
        .create(req.name, req.command, req.args, req.cwd, req.env, kind)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to create process");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(process.into()))
}

pub async fn list(State(state): State<AppState>) -> Json<Vec<ProcessResponse>> {
    let processes = state.process_store.list().await;
    Json(processes.into_iter().map(Into::into).collect())
}

pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ProcessResponse>, StatusCode> {
    state
        .process_store
        .get(&id)
        .await
        .map(|p| Json(p.into()))
        .ok_or(StatusCode::NOT_FOUND)
}

pub async fn kill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    state
        .process_store
        .kill(&id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|e| {
            if e.to_string().contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        })
}

pub async fn output_ws(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    if state.process_store.get(&id).await.is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    ws.on_upgrade(move |socket| crate::terminal::attach_ws(socket, id, state))
}

pub async fn history(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let log_path = state.process_store.log_path(&id);
    if !log_path.exists() {
        return StatusCode::NOT_FOUND.into_response();
    }

    match tokio::fs::read(&log_path).await {
        Ok(data) => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            data,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
