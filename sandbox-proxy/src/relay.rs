//! Dialing the sandbox and relaying frames.
//!
//! # Handshake design: terminate, don't relay
//!
//! The proxy dials sandbox-init first and completes `hello`/`welcome` as
//! `client: "proxy"` (contracts §3, §7) *before* upgrading the browser socket.
//! That way a dead or refusing sandbox surfaces as an HTTP 502/503 rather than an
//! opened-then-closed websocket, and the sandbox always sees the proxy identity.
//! The browser's own `hello` (which says `client: "ui"`) is then answered locally:
//! it must be `ui` and offer the version the sandbox already agreed to; the proxy
//! replies `welcome` itself. After that both sockets carry the same session and
//! frames are relayed byte for byte, so end-to-end flow control stays intact.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use axum::extract::ws::{CloseFrame as AxClose, Message as AxMsg, WebSocket};
use enzarb_proto::{ClientKind, Ctrl, Frame, FrameHeader, error_code, session::handshake};
use futures_util::{SinkExt, StreamExt};
use tokio::{net::TcpStream, sync::broadcast};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message as TgMsg,
        client::IntoClientRequest,
        http::{HeaderValue, header::AUTHORIZATION},
        protocol::WebSocketConfig,
    },
};
use tokio_util::sync::CancellationToken;

use crate::{
    guard::{self, Reject},
    resolver::{ResolveError, Resolver, revoked},
};

pub type Upstream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, thiserror::Error)]
pub enum DialError {
    #[error("bad sandbox endpoint")]
    BadEndpoint,
    #[error("sandbox rejected the credentials")]
    Unauthorized,
    #[error("sandbox unreachable: {0}")]
    Unreachable(String),
}

/// A sandbox connection that has completed the `proxy` handshake.
pub struct Dialed {
    pub ws: Upstream,
    pub version: u16,
}

pub fn ws_config(max_message_bytes: usize) -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(max_message_bytes))
        .max_frame_size(Some(max_message_bytes))
}

/// Connect to `endpoint` with `Authorization: Bearer <token>` and handshake as
/// `client: "proxy"`.
pub async fn dial(
    endpoint: &str,
    token: &str,
    max_message_bytes: usize,
    timeout: Duration,
) -> Result<Dialed, DialError> {
    if !endpoint.starts_with("ws://") {
        // TLS is not compiled in; the in-cluster endpoint is plain ws:// (contracts §2).
        return Err(DialError::BadEndpoint);
    }
    let mut req = endpoint
        .into_client_request()
        .map_err(|_| DialError::BadEndpoint)?;
    let mut value =
        HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| DialError::BadEndpoint)?;
    value.set_sensitive(true);
    req.headers_mut().insert(AUTHORIZATION, value);

    let connect = async {
        let (mut ws, _) = connect_async_with_config(req, Some(ws_config(max_message_bytes)), false)
            .await
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Http(r)
                    if matches!(r.status().as_u16(), 401 | 403) =>
                {
                    DialError::Unauthorized
                }
                e => DialError::Unreachable(e.to_string()),
            })?;
        let version = handshake::client_handshake(&mut ws, ClientKind::Proxy)
            .await
            .map_err(|e| DialError::Unreachable(e.to_string()))?;
        Ok(Dialed { ws, version })
    };
    tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| DialError::Unreachable("timeout".into()))?
}

/// Why a relay ended. Fields are only read through `Debug` in the close log.
#[allow(dead_code)]
#[derive(Debug)]
enum End {
    BrowserClosed,
    UpstreamClosed,
    Rejected(Reject),
    BrowserProtocol(&'static str),
    Transport(String),
    Idle,
    Shutdown,
    Revoked,
}

/// Browser-side handshake: answer `hello` locally (see module docs).
async fn browser_handshake(
    ws: &mut WebSocket,
    version: u16,
    timeout: Duration,
) -> Result<(), &'static str> {
    let first = tokio::time::timeout(timeout, async {
        loop {
            match ws.recv().await {
                Some(Ok(AxMsg::Binary(b))) => return Some(b),
                Some(Ok(AxMsg::Ping(_) | AxMsg::Pong(_))) => {}
                _ => return None,
            }
        }
    })
    .await
    .map_err(|_| "handshake timeout")?
    .ok_or("expected binary hello")?;
    let reply = match Frame::from_bytes(first)
        .ok()
        .and_then(|f| f.ctrl_message().ok())
    {
        Some(Ctrl::Hello { versions, client })
            if client == ClientKind::Ui && versions.contains(&version) =>
        {
            Ctrl::Welcome { version }
        }
        Some(Ctrl::Hello { .. }) => Ctrl::Error {
            code: error_code::UNSUPPORTED_VERSION.into(),
            message: format!("expected client \"ui\" offering version {version}"),
            run_id: None,
            channel: None,
        },
        _ => Ctrl::Error {
            code: error_code::PROTOCOL_VIOLATION.into(),
            message: "first frame must be CTRL hello".into(),
            run_id: None,
            channel: None,
        },
    };
    let ok = matches!(reply, Ctrl::Welcome { .. });
    let bytes = Frame::ctrl(&reply)
        .and_then(|f| f.to_bytes())
        .map_err(|_| "encode")?;
    ws.send(AxMsg::Binary(bytes)).await.map_err(|_| "send")?;
    if ok { Ok(()) } else { Err("bad hello") }
}

#[derive(Default)]
struct Counters {
    up_msgs: AtomicU64,
    up_bytes: AtomicU64,
    down_msgs: AtomicU64,
    down_bytes: AtomicU64,
}

pub struct RelayOpts {
    pub idle_timeout: Duration,
    pub handshake_timeout: Duration,
}

/// Watches a Project for changes that revoke an open relay's authorisation.
pub struct ProjectWatch {
    pub rx: broadcast::Receiver<String>,
    pub resolver: Arc<dyn Resolver>,
    pub project: String,
    /// The `spec.owner` that was authorised at connect time.
    pub owner: String,
}

impl ProjectWatch {
    async fn is_revoked(&self) -> bool {
        match self.resolver.project(&self.project).await {
            Ok(info) => revoked(&info, &self.owner),
            Err(ResolveError::NotFound) => true,
            Err(ResolveError::Unavailable(why)) => {
                tracing::warn!(project = %self.project, why, "project re-check failed");
                false
            }
        }
    }

    /// Returns once access has been revoked. Checks once immediately (covering
    /// changes since the subscription was taken), then on each relevant event.
    pub async fn changed(&mut self) {
        if self.is_revoked().await {
            return;
        }
        loop {
            match self.rx.recv().await {
                Ok(name) if name != self.project => continue,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    if self.is_revoked().await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            }
        }
    }
}

/// Run the browser<->sandbox relay until either side closes, a browser frame
/// is rejected, the connection idles out, access to the Project is revoked or
/// the process shuts down.
pub async fn run(
    mut browser: WebSocket,
    upstream: Dialed,
    opts: RelayOpts,
    shutdown: CancellationToken,
    mut watch: ProjectWatch,
    project: &str,
    sub: &str,
) {
    if let Err(why) =
        browser_handshake(&mut browser, upstream.version, opts.handshake_timeout).await
    {
        tracing::info!(project, sub, why, "browser handshake failed");
        let _ = browser.close().await;
        let mut up = upstream.ws;
        let _ = up.close(None).await;
        return;
    }

    let (mut b_tx, mut b_rx) = browser.split();
    let (mut u_tx, mut u_rx) = upstream.ws.split();
    let counters = Counters::default();
    let start = Instant::now();
    let last = AtomicU64::new(0);
    let touch = || last.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);

    let up = async {
        while let Some(m) = b_rx.next().await {
            touch();
            let m = match m {
                Ok(m) => m,
                Err(e) => return End::Transport(e.to_string()),
            };
            match m {
                AxMsg::Binary(b) => {
                    if let Err(r) = guard::check_browser_message(&b) {
                        return End::Rejected(r);
                    }
                    counters.up_msgs.fetch_add(1, Ordering::Relaxed);
                    counters
                        .up_bytes
                        .fetch_add(b.len() as u64, Ordering::Relaxed);
                    if let Err(e) = u_tx.send(TgMsg::Binary(b)).await {
                        return End::Transport(e.to_string());
                    }
                }
                AxMsg::Text(_) => return End::BrowserProtocol("text message"),
                AxMsg::Ping(_) | AxMsg::Pong(_) => {}
                AxMsg::Close(_) => return End::BrowserClosed,
            }
        }
        End::BrowserClosed
    };
    let down = async {
        while let Some(m) = u_rx.next().await {
            touch();
            let m = match m {
                Ok(m) => m,
                Err(e) => return End::Transport(e.to_string()),
            };
            match m {
                TgMsg::Binary(b) => {
                    if let Err(e) = FrameHeader::parse(&b) {
                        tracing::warn!(error = %e, "malformed frame from sandbox");
                    }
                    counters.down_msgs.fetch_add(1, Ordering::Relaxed);
                    counters
                        .down_bytes
                        .fetch_add(b.len() as u64, Ordering::Relaxed);
                    if let Err(e) = b_tx.send(AxMsg::Binary(b)).await {
                        return End::Transport(e.to_string());
                    }
                }
                TgMsg::Close(_) => return End::UpstreamClosed,
                TgMsg::Text(_) => return End::Transport("text message from sandbox".into()),
                _ => {}
            }
        }
        End::UpstreamClosed
    };
    let idle = async {
        let tick = (opts.idle_timeout / 4).max(Duration::from_millis(50));
        loop {
            tokio::time::sleep(tick).await;
            let quiet = start.elapsed().as_millis() as u64 - last.load(Ordering::Relaxed);
            if quiet >= opts.idle_timeout.as_millis() as u64 {
                return End::Idle;
            }
        }
    };

    let revoked = async {
        watch.changed().await;
        End::Revoked
    };

    let end = tokio::select! {
        e = revoked => e,
        e = up => e,
        e = down => e,
        e = idle => e,
        _ = shutdown.cancelled() => End::Shutdown,
    };

    tracing::info!(
        project,
        sub,
        reason = ?end,
        up_msgs = counters.up_msgs.load(Ordering::Relaxed),
        up_bytes = counters.up_bytes.load(Ordering::Relaxed),
        down_msgs = counters.down_msgs.load(Ordering::Relaxed),
        down_bytes = counters.down_bytes.load(Ordering::Relaxed),
        "relay closed"
    );

    // Propagate the close. Everything here is best effort and bounded.
    let (code, reason): (u16, &str) = match &end {
        End::BrowserClosed | End::UpstreamClosed => (1000, ""),
        End::Rejected(_) => (1008, "message not permitted"),
        End::BrowserProtocol(_) => (1002, "protocol error"),
        End::Transport(_) => (1011, "relay error"),
        End::Idle => (1001, "idle timeout"),
        End::Shutdown => (1001, "going away"),
        End::Revoked => (1008, "project access changed"),
    };
    let finish = async {
        if let End::Rejected(_) | End::BrowserProtocol(_) = &end {
            let err = Ctrl::Error {
                code: error_code::PROTOCOL_VIOLATION.into(),
                message: "message not permitted from this client".into(),
                run_id: None,
                channel: None,
            };
            if let Ok(b) = Frame::ctrl(&err).and_then(|f| f.to_bytes()) {
                let _ = b_tx.send(AxMsg::Binary(b)).await;
            }
        }
        let _ = b_tx
            .send(AxMsg::Close(Some(AxClose {
                code,
                reason: reason.into(),
            })))
            .await;
        let _ = b_tx.close().await;
        let _ = u_tx.send(TgMsg::Close(None)).await;
        let _ = u_tx.close().await;
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), finish).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{FakeResolver, ProjectInfo, SandboxRef};

    fn info(owner: &str) -> ProjectInfo {
        ProjectInfo {
            owner: owner.into(),
            sandbox: Some(SandboxRef {
                endpoint: "ws://x/v1/mux".into(),
                secret_namespace: "ns".into(),
                secret_name: "n".into(),
                secret_key: "k".into(),
            }),
        }
    }

    #[tokio::test]
    async fn change_before_watch_start_is_caught_by_initial_check() {
        let r = Arc::new(FakeResolver::default());
        r.insert("p", info("alice"), "t");
        // Subscribed after the update, so no event is ever delivered.
        r.update("p", info("bob"), "t");
        let mut w = ProjectWatch {
            rx: r.subscribe(),
            resolver: r.clone(),
            project: "p".into(),
            owner: "alice".into(),
        };
        tokio::time::timeout(Duration::from_secs(1), w.changed())
            .await
            .expect("should report revoked");
    }
}
