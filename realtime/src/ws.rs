//! HTTP surface: `GET /live/v1/stream?token=<jwt>` (websocket), `/healthz`, `/readyz`.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::{StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, broadcast::error::RecvError, watch},
    time::Instant,
};

use crate::{
    auth::{self, Claims},
    hub::{Frame, Hub},
};

/// Application close code: the token expired, reconnect with a fresh one.
pub const CLOSE_TOKEN_EXPIRED: u16 = 4001;
/// Close code 1001: the server is shutting down, or the client stopped answering pings.
pub const CLOSE_GOING_AWAY: u16 = 1001;
/// Close code 1011: unexpected server-side condition.
pub const CLOSE_INTERNAL: u16 = 1011;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub secret: Arc<Vec<u8>>,
    pub ping_interval: Duration,
    pub max_token_ttl: Duration,
    /// One permit per allowed concurrent websocket.
    pub limit: Arc<Semaphore>,
    /// Connections refused because the cap was reached.
    pub rejected: Arc<AtomicU64>,
    /// Flips to `true` when the server begins shutting down.
    pub shutdown: watch::Receiver<bool>,
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
    let Ok(permit) = s.limit.clone().try_acquire_owned() else {
        let total = s.rejected.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(sub = %claims.sub, rejected_total = total, "connection cap reached; rejecting");
        return (StatusCode::SERVICE_UNAVAILABLE, [(RETRY_AFTER, "1")]).into_response();
    };
    // Subscribe before the upgrade completes so nothing published in between is lost.
    let rx = s.hub.subscribe();
    let deadline = Instant::now() + auth::lifetime(&claims, s.max_token_ttl);
    let shutdown = s.shutdown.clone();
    ws.max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |sock| {
            client(
                sock,
                rx,
                claims,
                deadline,
                s.ping_interval,
                shutdown,
                permit,
            )
        })
}

async fn close(sock: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = send(
        sock,
        Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })),
    )
    .await;
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
    mut shutdown: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit, // released when the task ends
) {
    tracing::info!(sub = %claims.sub, projects = claims.projects.len(), "stream open");
    let mut ping = tokio::time::interval_at(Instant::now() + ping_every, ping_every);
    let pong_window = ping_every * 3 / 2;
    let pong_deadline = tokio::time::sleep_until(Instant::now() + pong_window);
    tokio::pin!(pong_deadline);
    let expiry = tokio::time::sleep_until(deadline);
    tokio::pin!(expiry);
    // If the sender is dropped without signalling, stop watching rather than spin.
    let mut watching = true;
    loop {
        tokio::select! {
            _ = &mut expiry => {
                close(&mut sock, CLOSE_TOKEN_EXPIRED, "token expired; reconnect with a fresh token").await;
                break;
            }
            r = async { shutdown.wait_for(|v| *v).await.is_ok() }, if watching => {
                if r {
                    close(&mut sock, CLOSE_GOING_AWAY, "server shutting down").await;
                    break;
                }
                watching = false;
            }
            _ = &mut pong_deadline => {
                tracing::info!(sub = %claims.sub, "no pong; dropping client");
                close(&mut sock, CLOSE_GOING_AWAY, "ping timeout").await;
                break;
            }
            _ = ping.tick() => {
                if !send(&mut sock, Message::Ping(Vec::new().into())).await { break; }
            }
            incoming = sock.recv() => match incoming {
                Some(Ok(Message::Pong(_))) => pong_deadline.as_mut().reset(Instant::now() + pong_window),
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
                    Err(RecvError::Closed) => {
                        close(&mut sock, CLOSE_INTERNAL, "internal error").await;
                        break;
                    }
                };
                if !ok { break; }
            }
        }
    }
    tracing::info!(sub = %claims.sub, "stream closed");
}
