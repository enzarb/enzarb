use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::{
    Json,
    extract::{Extension, Path, State, WebSocketUpgrade},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;

use crate::AppState;
use crate::acp::AcpWsClientMsg;
use crate::acp::events::AcpWsEvent;
use crate::acp::store::SessionMeta;
use crate::auth::ProjectPermissions;
use crate::init::home_dir;

/// Wire envelope adding a receive-time timestamp to every event. ACP itself
/// carries no per-notification timestamp, so this is stamped as each event
/// crosses into the WS — an approximation (broadcast/serialize time, not
/// generation time) but close enough for "when did this happen" in the UI.
#[derive(serde::Serialize)]
struct TimestampedEvent<'a> {
    #[serde(flatten)]
    event: &'a AcpWsEvent,
    ts_ms: i64,
}

fn encode(event: &AcpWsEvent) -> Option<String> {
    serde_json::to_string(&TimestampedEvent {
        event,
        ts_ms: chrono::Utc::now().timestamp_millis(),
    })
    .ok()
}

const PERM: &str = "agent:manage";

fn expand_tilde(path: String) -> std::path::PathBuf {
    let home = home_dir();
    if path == "~" {
        home
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        std::path::PathBuf::from(path)
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct CreateSessionRequest {
    pub label: Option<String>,
    pub cwd: Option<String>,
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Extension(perms): Extension<ProjectPermissions>,
) -> Result<Json<Vec<SessionMeta>>, StatusCode> {
    perms.require(PERM)?;
    Ok(Json(state.acp_store.list_sessions().await))
}

pub async fn create_session(
    State(state): State<AppState>,
    Extension(perms): Extension<ProjectPermissions>,
    Json(req): Json<CreateSessionRequest>,
) -> Result<Json<SessionMeta>, StatusCode> {
    perms.require(PERM)?;
    state
        .acp_store
        .create_session(req.label, req.cwd.map(expand_tilde))
        .await
        .map(Json)
        .map_err(|e| {
            tracing::error!(error = %e, "failed to create ACP session");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

pub async fn get_session(
    State(state): State<AppState>,
    Extension(perms): Extension<ProjectPermissions>,
    Path(id): Path<String>,
) -> Result<Json<SessionMeta>, StatusCode> {
    perms.require(PERM)?;
    state
        .acp_store
        .list_sessions()
        .await
        .into_iter()
        .find(|s| s.id == id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

pub async fn archive_session(
    State(state): State<AppState>,
    Extension(perms): Extension<ProjectPermissions>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    perms.require(PERM)?;
    state
        .acp_store
        .archive_session(&id)
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn session_ws(
    State(state): State<AppState>,
    Extension(perms): Extension<ProjectPermissions>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(e) = perms.require(PERM) {
        return e.into_response();
    }
    // Echo back the `bearer` subprotocol (never the token), matching the
    // terminal WS auth pattern — the browser requires the server to confirm
    // one of the offered subprotocols or it aborts the handshake.
    ws.protocols(["bearer"])
        .on_upgrade(move |socket| attach_ws(socket, id, state))
}

async fn attach_ws(mut socket: WebSocket, session_id: String, state: AppState) {
    let (history, mut rx) = match state.acp_store.attach(&session_id).await {
        Ok(result) => result,
        Err(e) => {
            tracing::warn!(error = %e, session_id, "failed to attach ACP session");
            let code = if e.to_string().contains("unknown session") {
                4404u16
            } else {
                1011
            };
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code,
                    reason: e.to_string().into(),
                })))
                .await;
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Replay in-memory history directly (reconnect after the session was
    // already loaded in this process lifetime).
    for event in &history {
        let Some(text) = encode(event) else {
            continue;
        };
        if ws_tx.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }

    let output = async move {
        // Idle ACP sessions can otherwise sit silent long enough for an
        // intermediate proxy/ingress to consider the connection dead and
        // reset it, which the client sees as an abnormal drop and reconnects.
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(25));
        ticker.tick().await;
        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Ok(event) => {
                            let Some(text) = encode(&event) else {
                                continue;
                            };
                            if ws_tx.send(Message::Text(text.into())).await.is_err() {
                                return;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    }
                }
                _ = ticker.tick() => {
                    if ws_tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                        return;
                    }
                }
            }
        }
    };

    let input = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(t) => {
                    if let Ok(client_msg) = serde_json::from_str::<AcpWsClientMsg>(&t)
                        && let Err(e) = state
                            .acp_store
                            .handle_client_msg(&session_id, client_msg)
                            .await
                    {
                        tracing::warn!(error = %e, session_id, "failed to handle ACP client message");
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };

    tokio::select! {
        _ = output => {},
        _ = input => {},
    }
}
