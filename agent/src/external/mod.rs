mod agent;
mod files;
mod processes;
mod status;
mod tools;
mod watch;

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
};
use tower_http::cors::CorsLayer;

use crate::AppState;
use crate::auth::ProjectPermissions;

pub fn router(state: AppState) -> Router {
    let origin = std::env::var("APP_ORIGIN").unwrap_or_else(|_| "https://enzarb.dev".to_string());
    let cors = CorsLayer::new()
        .allow_origin(
            origin
                .parse::<HeaderValue>()
                .expect("APP_ORIGIN is not a valid header value"),
        )
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        // Process management
        .route("/processes", post(processes::create).get(processes::list))
        .route(
            "/processes/{id}",
            get(processes::get_one).delete(processes::kill),
        )
        .route("/processes/{id}/output", get(processes::output_ws))
        .route("/processes/{id}/history", get(processes::history))
        // Agent tab (ACP sessions)
        .route(
            "/agent/sessions",
            get(agent::list_sessions).post(agent::create_session),
        )
        .route(
            "/agent/sessions/{id}",
            get(agent::get_session).delete(agent::archive_session),
        )
        .route("/agent/sessions/{id}/ws", get(agent::session_ws))
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
        // Auth middleware on all routes above
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // Unauthenticated liveness probe (registered after the auth layer, so
        // it is NOT wrapped by it). The frontend polls this to detect a
        // restarting workspace pod: while the pod is down the gateway answers
        // with a 5xx instead. Returns no body and reveals nothing.
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .layer(cors)
        .with_state(state)
}

async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Token resolution order:
    //  1. Authorization: Bearer <jwt> — normal HTTP requests.
    //  2. Sec-WebSocket-Protocol: bearer, <jwt> — browsers can't set arbitrary
    //     headers on a WS handshake but can set subprotocols, so WS clients send
    //     the token there (kept out of the URL, and therefore out of logs).
    //
    // A token is never accepted from the query string: that would leak the JWT
    // into access/proxy logs.
    let ws_token = extract_ws_protocol_token(&headers);
    let token = extract_bearer(&headers)
        .or(ws_token.as_deref())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = state
        .jwks
        .validate(token, &state.project_id)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Extract permissions for this project and store in extensions so handlers can check them.
    let perms = claims
        .projects
        .get(&state.project_id)
        .cloned()
        .unwrap_or_default();
    request.extensions_mut().insert(ProjectPermissions(perms));

    Ok(next.run(request).await)
}

fn extract_bearer(headers: &HeaderMap) -> Option<&str> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    auth.strip_prefix("Bearer ")
}

/// Extract a JWT carried in the `Sec-WebSocket-Protocol` request header. The
/// browser offers two subprotocols — a `bearer` marker and the token itself —
/// as `Sec-WebSocket-Protocol: bearer, <jwt>`. The server echoes back only the
/// `bearer` marker (see `output_ws`), never the token.
fn extract_ws_protocol_token(headers: &HeaderMap) -> Option<String> {
    let proto = headers.get("sec-websocket-protocol")?.to_str().ok()?;
    let mut parts = proto.split(',').map(str::trim);
    if parts.next()? != "bearer" {
        return None;
    }
    parts.next().filter(|t| !t.is_empty()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_header(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("sec-websocket-protocol", v.parse().unwrap());
        h
    }

    #[test]
    fn ws_protocol_token_extracted() {
        let h = proto_header("bearer, eyJabc.def.ghi");
        assert_eq!(
            extract_ws_protocol_token(&h).as_deref(),
            Some("eyJabc.def.ghi")
        );
    }

    #[test]
    fn ws_protocol_token_requires_bearer_marker() {
        assert!(extract_ws_protocol_token(&proto_header("eyJabc.def.ghi")).is_none());
        assert!(extract_ws_protocol_token(&proto_header("bearer")).is_none());
        assert!(extract_ws_protocol_token(&proto_header("bearer, ")).is_none());
    }

    #[test]
    fn ws_protocol_token_absent() {
        assert!(extract_ws_protocol_token(&HeaderMap::new()).is_none());
    }
}
