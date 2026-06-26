mod files;
mod processes;
mod status;
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
        .route("/files/git-status", get(files::git_status))
        .route("/files/git-diff", get(files::git_diff))
        .route("/files/git-commit", post(files::git_commit))
        // Tool management (mise) — mise.toml on the PVC is the source of truth
        .route("/tools", get(tools::list).post(tools::add))
        .route("/tools/registry", get(tools::registry))
        .route("/tools/{name}", delete(tools::remove))
        .route("/tools/{name}/versions", get(tools::versions))
        // Workspace status (disk usage etc.)
        .route("/status", get(status::status))
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
    // Prefer the Authorization header; fall back to a `token` query param.
    // Browsers cannot set headers on WebSocket connections, so WS clients
    // authenticate via `?token=<jwt>`.
    let query_token = extract_query_token(request.uri());
    let token = extract_bearer(&headers)
        .or(query_token.as_deref())
        .ok_or(StatusCode::UNAUTHORIZED)?;

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

fn extract_query_token(uri: &axum::http::Uri) -> Option<String> {
    let query = uri.query()?;
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(k, _)| *k == "token")
        .map(|(_, v)| percent_decode(v))
}

/// Minimal percent-decoding for query values (handles %XX and `+`).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                if let Ok(b) = u8::from_str_radix(&input[i + 1..i + 3], 16) {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
