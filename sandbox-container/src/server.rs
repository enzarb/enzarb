//! axum app (`/healthz`, `/v1/mux`), the websocket adapter and the
//! per-session event loop.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::extract::ws::{Message as AxumMsg, WebSocket, WebSocketUpgrade};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use enzarb_proto::session::{Event, OpenRequest, ServerOptions, Session};
use enzarb_proto::{
    ChannelKind, CredentialKind, Ctrl, HEADER_LEN, MAX_PAYLOAD_LEN, SandboxStats, error_code,
    feature, open_error_code,
};
use futures_util::{Sink, Stream};
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::Message as WsMsg;

use crate::auth::{self, AuthError};
use crate::capture::{LegacyPromotion, read_legacy_valid};
use crate::chat::ChatManager;
use crate::config::Config;
use crate::docker_prune::Pruner;
use crate::dockerd::Dockerd;
use crate::observer::{PtyInfo, PtyObserverFactory};
use crate::pty::{self, Pty, PtyConfig, PtyControl};
use crate::reaper::Reaper;
use crate::repos::RepoLocks;
use crate::stats::{self, SystemSource};
use crate::steps::StepManager;

/// How long a client has to complete the hello/welcome handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared server state.
pub struct AppState {
    pub config: Config,
    pub reaper: Arc<Reaper>,
    pub steps: StepManager,
    pub observers: Vec<Arc<dyn PtyObserverFactory>>,
    pub chats: Arc<ChatManager>,
    /// The supervised Docker daemon, reported in `/healthz` (never gates readiness).
    pub dockerd: Option<Arc<Dockerd>>,
    /// Docker pruning, reported in `/healthz` as `docker_disk` and `docker_prune`.
    pub pruner: Option<Arc<Pruner>>,
    next_session: AtomicU64,
}

impl AppState {
    /// State with the stub run manager, no PTY observers and a chat manager
    /// with its own clone locks (see [`with_chats`](Self::with_chats)).
    pub fn new(config: Config, reaper: Arc<Reaper>) -> Self {
        let locks = RepoLocks::default();
        let chats = ChatManager::new(config.clone(), reaper.clone(), locks.clone());
        let steps = StepManager::new(config.clone(), reaper.clone(), locks);
        Self {
            config,
            reaper,
            steps,
            observers: Vec::new(),
            chats,
            dockerd: None,
            pruner: None,
            next_session: AtomicU64::new(1),
        }
    }

    pub fn with_pruner(mut self, p: Arc<Pruner>) -> Self {
        self.pruner = Some(p);
        self
    }

    pub fn with_dockerd(mut self, d: Arc<Dockerd>) -> Self {
        self.dockerd = Some(d);
        self
    }

    pub fn with_chats(mut self, chats: Arc<ChatManager>) -> Self {
        self.chats = chats;
        self
    }

    pub fn with_steps(mut self, steps: StepManager) -> Self {
        self.steps = steps;
        self
    }

    pub fn with_observer(mut self, o: Arc<dyn PtyObserverFactory>) -> Self {
        self.observers.push(o);
        self
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let mux = Router::new()
        .route("/v1/mux", get(mux))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));
    Router::new()
        .route("/healthz", get(healthz))
        .merge(mux)
        .with_state(state)
}

/// Always 200 (the readiness probe must not depend on Docker); the body is JSON:
/// `{"status":"ok","dockerd":{"state":"running"},"docker_disk":{..,"low_disk":false},
/// "docker_prune":{"enabled":true,"last":null}}`. `dockerd` is absent without a supervisor, and
/// `docker_disk` / `docker_prune` without a pruner (`docker_disk` is null when the data root
/// cannot be read).
async fn healthz(State(state): State<Arc<AppState>>) -> Response {
    let mut body = serde_json::json!({"status": "ok"});
    if let Some(d) = &state.dockerd {
        body["dockerd"] = serde_json::to_value(d.state()).unwrap_or_default();
    }
    if let Some(p) = &state.pruner {
        body["docker_disk"] = serde_json::to_value(p.disk_report()).unwrap_or_default();
        body["docker_prune"] = serde_json::to_value(p.prune_report()).unwrap_or_default();
    }
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Serve until the listener fails.
pub async fn serve(listener: tokio::net::TcpListener, state: Arc<AppState>) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
}

async fn require_bearer(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    match auth::check(header, &state.config.auth_token_file) {
        Ok(()) => next.run(req).await,
        Err(AuthError::Unauthorized) => {
            let mut r = (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            r.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            r
        }
        Err(AuthError::NotConfigured) => {
            (StatusCode::SERVICE_UNAVAILABLE, "auth token not configured").into_response()
        }
    }
}

async fn mux(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(HEADER_LEN + MAX_PAYLOAD_LEN)
        .on_upgrade(move |socket| session_loop(state, socket))
}

/// Presents an axum `WebSocket` as a tokio-tungstenite message stream/sink,
/// which is what `enzarb_proto::session` consumes.
struct Adapter(WebSocket);

impl Stream for Adapter {
    type Item = Result<WsMsg, axum::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx).map(|o| {
            o.map(|r| {
                r.map(|m| match m {
                    AxumMsg::Text(t) => WsMsg::text(t.as_str()),
                    AxumMsg::Binary(b) => WsMsg::Binary(b),
                    AxumMsg::Ping(b) => WsMsg::Ping(b),
                    AxumMsg::Pong(b) => WsMsg::Pong(b),
                    AxumMsg::Close(_) => WsMsg::Close(None),
                })
            })
        })
    }
}

impl Sink<WsMsg> for Adapter {
    type Error = axum::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, m: WsMsg) -> Result<(), Self::Error> {
        let m = match m {
            WsMsg::Text(t) => AxumMsg::Text(t.as_str().into()),
            WsMsg::Binary(b) => AxumMsg::Binary(b),
            WsMsg::Ping(b) => AxumMsg::Ping(b),
            WsMsg::Pong(b) => AxumMsg::Pong(b),
            WsMsg::Close(_) => AxumMsg::Close(None),
            WsMsg::Frame(_) => return Ok(()),
        };
        Pin::new(&mut self.0).start_send(m)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

type Controls = Arc<Mutex<HashMap<u32, PtyControl>>>;

/// One mux connection.
#[derive(Clone)]
struct SessionCtx {
    id: u64,
    session: Session,
}

async fn session_loop(state: Arc<AppState>, socket: WebSocket) {
    let accepted = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        Session::accept(
            Adapter(socket),
            ServerOptions {
                features: {
                    let mut f = state.chats.features();
                    f.push(feature::STATS.to_string());
                    f.push(feature::FILE_READ.to_string());
                    f.push(feature::STEPS.to_string());
                    f
                },
                ..ServerOptions::default()
            },
        ),
    )
    .await;
    let (session, mut events) = match accepted {
        Ok(Ok(x)) => x,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "mux handshake failed");
            return;
        }
        Err(_) => {
            tracing::warn!("mux handshake timed out");
            return;
        }
    };
    let id = state.next_session.fetch_add(1, Ordering::Relaxed);
    let ctx = SessionCtx {
        id,
        session: session.clone(),
    };
    tracing::info!(session = id, client = ?session.peer_kind(), "mux session opened");

    // Promote a legacy token file to the proxy; deleted only on its ack.
    let mut promotion = LegacyPromotion::default();
    promotion.promote(&session, &state.config.claude_token_file);

    let controls: Controls = Arc::default();
    let mut tasks: JoinSet<()> = JoinSet::new();
    while let Some(event) = events.next().await {
        while tasks.try_join_next().is_some() {}
        match event {
            Event::Open(req) => open(&state, &ctx, &controls, &mut tasks, req),
            Event::Ctrl(Ctrl::Resize {
                channel,
                cols,
                rows,
            }) => {
                let control = controls.lock().unwrap().get(&channel).cloned();
                match control {
                    Some(c) => {
                        if let Err(e) = c.resize(cols, rows) {
                            tracing::warn!(session = id, channel, error = %e, "resize failed");
                        }
                    }
                    None => {
                        let _ = session.send_error(
                            error_code::UNKNOWN_CHANNEL,
                            &format!("channel {channel} is not a pty"),
                        );
                    }
                }
            }
            Event::Ctrl(Ctrl::StepStart(step)) => state.steps.start(id, &session, *step),
            Event::Ctrl(Ctrl::StepStatus { run_id, after_seq }) => {
                state.steps.status(id, &session, &run_id, after_seq)
            }
            Event::Ctrl(Ctrl::StepSteer { run_id, text }) => {
                state.steps.steer(&session, &run_id, text)
            }
            Event::Ctrl(Ctrl::StepStop { run_id }) => state.steps.stop(&session, &run_id),
            Event::Ctrl(Ctrl::StepResume { run_id }) => state.steps.resume(&session, &run_id),
            Event::Ctrl(Ctrl::CredentialStatus) => {
                promotion.promote(&session, &state.config.claude_token_file);
                // File-based: a per-request token cannot be known here.
                let configured = read_legacy_valid(&state.config.claude_token_file).is_some();
                if let Err(e) = session.send_ctrl(&Ctrl::CredentialState {
                    kind: CredentialKind::ClaudeOauth,
                    configured,
                    scope: None,
                }) {
                    tracing::warn!(session = id, error = %e, "credential_state send failed");
                }
            }
            // On demand only: a few small file reads, off the event loop. A
            // failure never ends the session; the reply just lacks the fields.
            Event::Ctrl(Ctrl::StatsRequest) => {
                let src = SystemSource {
                    cgroup_root: state.config.cgroup_root.clone(),
                    data_path: state.config.workspace.clone(),
                };
                let session = session.clone();
                tasks.spawn(async move {
                    let stats =
                        match tokio::task::spawn_blocking(move || stats::snapshot(&src)).await {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::debug!(session = id, error = %e, "stats snapshot failed");
                                SandboxStats {
                                    ts_ms: stats::unix_ms(),
                                    ..SandboxStats::default()
                                }
                            }
                        };
                    if let Err(e) = session.send_ctrl(&Ctrl::Stats(stats)) {
                        tracing::debug!(session = id, error = %e, "stats send failed");
                    }
                });
            }
            // A few KiB at most, off the event loop; the reply carries failures.
            Event::Ctrl(Ctrl::FileRead { id: rid, path }) => {
                let root = state.config.workspace.clone();
                let base = state.chats.worktree_of(id);
                let session = session.clone();
                tasks.spawn(async move {
                    let reply = tokio::task::spawn_blocking(move || {
                        crate::file_read::read(&root, base.as_deref(), rid, path)
                    })
                    .await;
                    if let Ok(reply) = reply
                        && let Err(e) = session.send_ctrl(&reply)
                    {
                        tracing::debug!(session = id, error = %e, "file send failed");
                    }
                });
            }
            // The proxy's ack of a legacy `credential_captured`.
            Event::Ctrl(Ctrl::CredentialState {
                kind: CredentialKind::ClaudeOauth,
                configured,
                ..
            }) => {
                promotion.ack(configured, &state.config.claude_token_file);
            }
            Event::Ctrl(other) => tracing::debug!(session = id, ?other, "ignored ctrl"),
        }
    }
    // Per-session cleanup: aborting the bridges drops the `Pty`s, which kills their process groups.
    tasks.shutdown().await;
    state.steps.session_closed(id);
    tracing::info!(session = id, "mux session closed");
}

fn open(
    state: &AppState,
    ctx: &SessionCtx,
    controls: &Controls,
    tasks: &mut JoinSet<()>,
    req: OpenRequest,
) {
    match req.kind {
        ChannelKind::Pty => {}
        // Chats run outside this session's task set: their cleanup (agent,
        // worktree) must finish even when the socket drops.
        ChannelKind::Acp => return state.chats.open(ctx.id, req),
        _ => {
            return req.reject_with_code(
                open_error_code::UNSUPPORTED_KIND,
                "unsupported channel kind",
            );
        }
    }
    let info = PtyInfo {
        session_id: ctx.id,
        session: ctx.session.clone(),
        channel: req.channel,
        command: req.command.clone(),
    };
    // Only the first observer that wants this PTY is attached; compose in the factory if more are needed.
    let observer = state.observers.iter().find_map(|f| f.observe(&info));
    let cfg = PtyConfig {
        shell: state.config.shell.clone(),
        workspace: state.config.workspace.clone(),
    };
    let pty = match Pty::spawn(
        &state.reaper,
        &cfg,
        req.cols,
        req.rows,
        req.command.as_deref(),
        observer,
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(session = ctx.id, error = %e, "pty spawn failed");
            req.reject(&format!("cannot start process: {e}"));
            return;
        }
    };
    let channel_id = req.channel;
    let handle = match req.accept() {
        Ok(h) => h,
        Err(_) => return, // session ended; dropping `pty` kills it
    };
    tracing::info!(
        session = ctx.id,
        channel = channel_id,
        pid = pty.pid(),
        "pty opened"
    );
    controls.lock().unwrap().insert(channel_id, pty.control());
    let controls = controls.clone();
    tasks.spawn(async move {
        pty::bridge(handle, pty).await;
        controls.lock().unwrap().remove(&channel_id);
    });
}
