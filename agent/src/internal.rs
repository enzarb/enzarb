use axum::{Router, routing::get};
use serde_json::json;

pub fn router() -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
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
