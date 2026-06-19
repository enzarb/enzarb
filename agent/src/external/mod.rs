mod files;
mod processes;
mod tools;
mod watch;

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
};
use tower_http::cors::CorsLayer;

use crate::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        // Process management
        .route("/processes", post(processes::create).get(processes::list))
        .route(
            "/processes/{id}",
            get(processes::get_one).delete(processes::kill),
        )
        .route("/processes/{id}/output", get(processes::output_ws))
        .route("/processes/{id}/history", get(processes::history))
        // File operations
        .route("/files", get(files::list).delete(files::delete))
        .route("/files/download", get(files::download))
        .route("/files/upload", post(files::upload))
        // Tool management (mise) — mise.toml on the PVC is the source of truth
        .route("/tools", get(tools::list).post(tools::add))
        .route("/tools/registry", get(tools::registry))
        .route("/tools/{name}", delete(tools::remove))
        .route("/tools/{name}/versions", get(tools::versions))
        // Watch (inotify SSE)
        .route("/watch", get(watch::watch))
        // Auth middleware on all routes
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = extract_bearer(&headers).ok_or(StatusCode::UNAUTHORIZED)?;

    state
        .jwks
        .validate(token, &state.project_id)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    Ok(next.run(request).await)
}

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    auth.strip_prefix("Bearer ")
}
