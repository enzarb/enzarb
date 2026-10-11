//! HTTP surface: `GET /live/v1/stream?token=<jwt>` (websocket), `/healthz`, `/readyz`.

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use tokio::{sync::broadcast::error::RecvError, time::Instant};

use crate::{
    auth::{self, Claims},
    hub::{Frame, Hub},
};

/// Application close code: the token expired, reconnect with a fresh one.
pub const CLOSE_TOKEN_EXPIRED: u16 = 4001;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub secret: Arc<Vec<u8>>,
    pub ping_interval: Duration,
    pub max_token_ttl: Duration,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/live/v1/stream", get(stream))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .with_state(state)
}

async fn readyz(State(s): State<AppState>) -> Response {
    if s.hub.is_ready() {
        "ok".into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "listener not connected").into_response()
    }
}

#[derive(Deserialize)]
struct StreamQuery {
    token: Option<String>,
}

async fn stream(
    State(s): State<AppState>,
    Query(q): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = q.token else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let claims = match auth::validate(&token, &s.secret, s.max_token_ttl) {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(error = ?e, "stream auth rejected");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };
    // Subscribe before the upgrade completes so nothing published in between is lost.
    let rx = s.hub.subscribe();
    let deadline = Instant::now() + auth::lifetime(&claims, s.max_token_ttl);
    ws.max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |sock| client(sock, rx, claims, deadline, s.ping_interval))
}

async fn send(sock: &mut WebSocket, m: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, sock.send(m)).await,
        Ok(Ok(()))
    )
}

async fn send_frame(sock: &mut WebSocket, f: &Frame) -> bool {
    let json = serde_json::to_string(f).expect("frame serializes");
    send(sock, Message::Text(json.into())).await
}

async fn client(
    mut sock: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<Frame>,
    claims: Claims,
    deadline: Instant,
    ping_every: Duration,
) {
    tracing::info!(sub = %claims.sub, projects = claims.projects.len(), "stream open");
    let mut ping = tokio::time::interval_at(Instant::now() + ping_every, ping_every);
    let mut last_pong = Instant::now();
    let expiry = tokio::time::sleep_until(deadline);
    tokio::pin!(expiry);
    loop {
        tokio::select! {
            _ = &mut expiry => {
                let _ = send(&mut sock, Message::Close(Some(CloseFrame {
                    code: CLOSE_TOKEN_EXPIRED,
                    reason: "token expired; reconnect with a fresh token".into(),
                }))).await;
                break;
            }
            _ = ping.tick() => {
                if last_pong.elapsed() >= ping_every * 3 / 2 {
                    tracing::info!(sub = %claims.sub, "no pong; dropping client");
                    break;
                }
                if !send(&mut sock, Message::Ping(Vec::new().into())).await { break; }
            }
            incoming = sock.recv() => match incoming {
                Some(Ok(Message::Pong(_))) => last_pong = Instant::now(),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {} // client messages are ignored
            },
            f = rx.recv() => {
                let ok = match f {
                    Ok(f) if f.visible_to(&claims.projects) => send_frame(&mut sock, &f).await,
                    Ok(_) => true,
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(sub = %claims.sub, skipped = n, "client lagged; resync");
                        send_frame(&mut sock, &Frame::Resync).await
                    }
                    Err(RecvError::Closed) => false,
                };
                if !ok { break; }
            }
        }
    }
    tracing::info!(sub = %claims.sub, "stream closed");
}
