//! HTTP surface: `/healthz` and the websocket mux route.

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{Path, Query, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::{
    auth::{self, AuthError, JwtValidator},
    config::Config,
    relay::{self, DialError, ProjectWatch, RelayOpts},
    resolver::{ResolveError, Resolver},
    store::Credentials,
};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub validator: JwtValidator,
    pub resolver: Arc<dyn Resolver>,
    pub creds: Arc<Credentials>,
    pub shutdown: CancellationToken,
    /// Chat capture into the engine (`engine_api.chat_*`); none in tests.
    pub chats: Option<Arc<dyn crate::chat::ChatSink>>,
}

impl AppState {
    pub fn new(
        cfg: Config,
        resolver: Arc<dyn Resolver>,
        creds: Arc<Credentials>,
        shutdown: CancellationToken,
    ) -> Self {
        Self {
            creds,
            validator: JwtValidator::new(&cfg.jwt_secret),
            cfg: Arc::new(cfg),
            resolver,
            shutdown,
            chats: None,
        }
    }

    pub fn with_chats(mut self, chats: Arc<dyn crate::chat::ChatSink>) -> Self {
        self.chats = Some(chats);
        self
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/sandbox/v1/projects/{project}/mux", get(mux))
        .with_state(state)
}

#[derive(Deserialize)]
struct MuxQuery {
    token: Option<String>,
}

fn fail(status: StatusCode, msg: &'static str) -> Response {
    (status, msg).into_response()
}

async fn mux(
    State(st): State<AppState>,
    Path(project): Path<String>,
    Query(q): Query<MuxQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    // Never log the URI: the JWT travels in the query string.
    let Some(token) = q.token.filter(|t| !t.is_empty()) else {
        return fail(StatusCode::UNAUTHORIZED, "missing token");
    };
    if !auth::valid_project_name(&project) {
        return fail(StatusCode::BAD_REQUEST, "invalid project name");
    }
    let claims = match st.validator.validate(&token, &project) {
        Ok(c) => c,
        Err(AuthError::InvalidToken(why)) => {
            tracing::info!(project, why, "rejected token");
            return fail(StatusCode::UNAUTHORIZED, "invalid token");
        }
        Err(AuthError::ProjectMismatch) => {
            tracing::info!(project, "token project mismatch");
            return fail(StatusCode::FORBIDDEN, "forbidden");
        }
    };
    // Subscribe before the lookup so a change right after it cannot be missed.
    let rx = st.resolver.subscribe();
    let info = match st.resolver.project(&project).await {
        Ok(i) => i,
        Err(ResolveError::NotFound) => return fail(StatusCode::NOT_FOUND, "no such project"),
        Err(ResolveError::Unavailable(_)) => {
            return fail(StatusCode::SERVICE_UNAVAILABLE, "lookup unavailable");
        }
    };
    if !auth::authorize(&claims.sub, &info.owner) {
        tracing::info!(project, sub = %claims.sub, "not the project owner");
        return fail(StatusCode::FORBIDDEN, "forbidden");
    }
    let owner = info.owner;
    let Some(sandbox) = info.sandbox else {
        return fail(StatusCode::SERVICE_UNAVAILABLE, "sandbox not ready");
    };
    let bearer = match st.resolver.token(&sandbox).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(project, error = %e, "sandbox token unavailable");
            return fail(StatusCode::SERVICE_UNAVAILABLE, "sandbox not ready");
        }
    };
    let dialed = match relay::dial(
        &sandbox.endpoint,
        &bearer,
        st.cfg.max_message_bytes,
        st.cfg.dial_timeout,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(project, error = %e, "dial failed");
            if matches!(e, DialError::Unauthorized) {
                // The token may have been rotated; re-read it next time.
                st.resolver.invalidate_token(&sandbox);
            }
            return fail(StatusCode::BAD_GATEWAY, "sandbox unreachable");
        }
    };

    let opts = RelayOpts {
        idle_timeout: st.cfg.idle_timeout,
        handshake_timeout: Duration::from_secs(10),
        ping_interval: st.cfg.ping_interval,
        creds: st.creds.clone(),
        chats: st.chats.clone(),
    };
    let shutdown = st.shutdown.clone();
    let watch = ProjectWatch {
        rx,
        resolver: st.resolver.clone(),
        project: project.clone(),
        owner,
    };
    let sub = claims.sub;
    ws.max_message_size(st.cfg.max_message_bytes)
        .max_frame_size(st.cfg.max_message_bytes)
        .on_upgrade(move |sock| async move {
            relay::run(sock, dialed, opts, shutdown, watch, &project, &sub).await;
        })
}
