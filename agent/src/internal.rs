use axum::{Router, extract::State, routing::get};
use serde_json::json;

use crate::AppState;
use crate::process::ProcessStatus;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/processes", get(processes))
        .with_state(state)
}

async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(json!({ "status": "ok" }))
}

async fn metrics() -> String {
    // Prometheus text format — extend with actual metrics as needed
    "# HELP project_agent_up Agent is running\n\
     # TYPE project_agent_up gauge\n\
     project_agent_up 1\n"
        .to_string()
}

// Unauthenticated — internal port is cluster-only. Used by the operator to
// decide whether it's safe to restart the workspace pod for a version update.
async fn processes(State(state): State<AppState>) -> axum::Json<serde_json::Value> {
    let all = state.process_store.list().await;
    let running: Vec<_> = all
        .iter()
        .filter(|p| p.status == ProcessStatus::Running)
        .map(|p| json!({ "id": p.id, "name": p.name }))
        .collect();

    let all_sessions = state.acp_store.list_sessions().await;
    let active_sessions: Vec<_> = all_sessions
        .iter()
        .map(|s| json!({ "id": s.id, "label": s.label, "status": s.status }))
        .collect();

    axum::Json(json!({
        "running": running.len(),
        "processes": running,
        "sessions": active_sessions.len(),
        "active_sessions": active_sessions,
    }))
}
