//! Session state and the reader/writer tasks.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Display;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Message;

use super::channel::{Chan, ChannelHandle, lock};
use super::handshake::{client_handshake, server_handshake};
use super::{HandshakeError, SessionEnd, SessionError};
use crate::{
    ChannelKind, ClientKind, Ctrl, Frame, FrameType, RunPhase, RunStart, SUPPORTED_VERSIONS,
    close_code, error_code,
};

/// Which side of the protocol a session plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Sent `hello`: ui, proxy, executor. Picks channel ids, opens channels, starts runs.
    Client,
    /// Sent `welcome`: sandbox-init. Answers `open`/`run_*`.
    Server,
}

/// Options for [`Session::accept`].
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Versions this server speaks.
    pub supported_versions: Vec<u16>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            supported_versions: SUPPORTED_VERSIONS.to_vec(),
        }
    }
}

/// `run_accepted` as seen by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunAccepted {
    pub run_id: String,
    pub state: RunPhase,
    /// The `run_id` was already known (idempotent `run_start`).
    pub existing: bool,
}

/// `run_state` as seen by the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInfo {
    pub run_id: String,
    pub state: RunPhase,
    pub exit_code: Option<i32>,
    pub summary: Option<String>,
}

/// Result of [`Session::run_start`].
pub struct RunStarted {
    pub accepted: RunAccepted,
    /// The transcript channel (`DATA_JSON` lines), if `log_channel` was set.
    pub log: Option<ChannelHandle>,
}

/// Unsolicited inbound CTRL traffic, in arrival order.
pub enum Event {
    /// (server) The client wants a channel: [`OpenRequest::accept`] or [`OpenRequest::reject`].
    Open(OpenRequest),
    /// Any other CTRL message not consumed internally:
    /// * server: `run_start`, `run_status`, `run_cancel`, `resize` (for an open channel), `error`;
    /// * client: `run_state` pushes, `credential_captured`, unattributed `error`.
    Ctrl(Ctrl),
}

/// Stream of [`Event`]s; ends when the session ends. Unbounded, so it may be
/// ignored (or dropped) by users who only need channels.
pub struct Events {
    rx: mpsc::UnboundedReceiver<Event>,
}

impl Events {
    pub async fn next(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

impl Stream for Events {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        self.rx.poll_recv(cx)
    }
}

/// A pending client `open`. Dropping it undecided rejects the open.
pub struct OpenRequest {
    session: Session,
    pub channel: u32,
    pub kind: ChannelKind,
    pub cols: u16,
    pub rows: u16,
    pub command: Option<Vec<String>>,
    decided: bool,
}

impl OpenRequest {
    /// Open the channel: registers it, sends `opened`, and returns the handle.
    pub fn accept(mut self) -> Result<ChannelHandle, SessionError> {
        self.decided = true;
        let inner = &self.session.inner;
        let chan = Chan::new(self.channel, inner);
        {
            let mut st = lock(&inner.state);
            inner.check_alive()?;
            st.reserved.remove(&self.channel);
            st.channels.insert(self.channel, chan.clone());
        }
        inner.send_ctrl(&Ctrl::Opened {
            channel: self.channel,
        })?;
        Ok(ChannelHandle::new(chan))
    }

    /// Refuse the open with `open_error`.
    pub fn reject(mut self, message: &str) {
        self.decided = true;
        self.session.inner.reject_open(self.channel, message);
    }
}

impl Drop for OpenRequest {
    fn drop(&mut self) {
        if !self.decided {
            self.session
                .inner
                .reject_open(self.channel, "open request was not handled");
        }
    }
}

type PendingOpen = (Arc<Chan>, oneshot::Sender<Result<(), SessionError>>);
type RunStartReply = Result<(RunAccepted, Option<Arc<Chan>>), SessionError>;

enum Pending {
    RunStart {
        run_id: String,
        log: Option<Arc<Chan>>,
        tx: oneshot::Sender<RunStartReply>,
    },
    RunStatus {
        run_id: String,
        tx: oneshot::Sender<Result<RunInfo, SessionError>>,
    },
}

#[derive(Default)]
pub(crate) struct State {
    channels: HashMap<u32, Arc<Chan>>,
    /// Ids being opened (client: awaiting `opened`/`run_accepted`; server: awaiting accept).
    reserved: HashSet<u32>,
    pending_open: HashMap<u32, PendingOpen>,
    pending: VecDeque<Pending>,
    next_id: u32,
}

impl State {
    fn in_use(&self, id: u32) -> bool {
        self.channels.contains_key(&id) || self.reserved.contains(&id)
    }

    fn alloc(&mut self) -> Result<u32, SessionError> {
        for _ in 0..1024u32 {
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            if !self.in_use(self.next_id) {
                return Ok(self.next_id);
            }
        }
        Err(SessionError::NoFreeChannel)
    }
}

pub(crate) enum Out {
    Frame(Bytes),
    Close,
}

pub(crate) struct Inner {
    role: Role,
    version: u16,
    peer: Option<ClientKind>,
    pub(crate) out: mpsc::UnboundedSender<Out>,
    pub(crate) state: Mutex<State>,
    end: watch::Sender<Option<SessionEnd>>,
    handles: AtomicUsize,
}

impl Inner {
    fn ended(&self) -> SessionError {
        SessionError::Ended(self.end.borrow().clone().unwrap_or(SessionEnd::Closed))
    }

    pub(crate) fn check_alive(&self) -> Result<(), SessionError> {
        if self.end.borrow().is_some() {
            return Err(self.ended());
        }
        Ok(())
    }

    fn send_frame(&self, f: &Frame) -> Result<(), SessionError> {
        let b = f.to_bytes()?;
        self.out.send(Out::Frame(b)).map_err(|_| self.ended())
    }

    pub(crate) fn send_ctrl(&self, m: &Ctrl) -> Result<(), SessionError> {
        self.send_frame(&Frame::ctrl(m)?)
    }

    fn send_error(&self, code: &str, message: &str) {
        let _ = self.send_ctrl(&Ctrl::Error {
            code: code.into(),
            message: message.into(),
        });
    }

    pub(crate) fn forget_channel(&self, chan: &Arc<Chan>) {
        let mut st = lock(&self.state);
        if st
            .channels
            .get(&chan.id)
            .is_some_and(|c| Arc::ptr_eq(c, chan))
        {
            st.channels.remove(&chan.id);
        }
    }

    fn reject_open(&self, channel: u32, message: &str) {
        lock(&self.state).reserved.remove(&channel);
        let _ = self.send_ctrl(&Ctrl::OpenError {
            channel,
            message: message.into(),
        });
    }

    /// Graceful local close: say goodbye on open channels, flush, close the websocket.
    fn begin_close(&self) {
        let chans: Vec<_> = lock(&self.state).channels.values().cloned().collect();
        for c in chans {
            let _ = c.close_local(close_code::GOING_AWAY, "going away");
        }
        let _ = self.out.send(Out::Close);
    }

    /// End the session now (idempotent): fail every channel and pending request.
    fn finish(&self, end: SessionEnd) {
        self.end.send_if_modified(|e| {
            if e.is_none() {
                *e = Some(end);
                true
            } else {
                false
            }
        });
        let end = self.end.borrow().clone().unwrap_or(SessionEnd::Closed);
        let (chans, pend_open, pend) = {
            let mut st = lock(&self.state);
            st.reserved.clear();
            (
                std::mem::take(&mut st.channels),
                std::mem::take(&mut st.pending_open),
                std::mem::take(&mut st.pending),
            )
        };
        for c in chans.values() {
            c.fail(&end);
        }
        for (c, tx) in pend_open.into_values() {
            c.fail(&end);
            let _ = tx.send(Err(SessionError::Ended(end.clone())));
        }
        for p in pend {
            p.fail(SessionError::Ended(end.clone()));
        }
        let _ = self.out.send(Out::Close);
    }

    fn fatal(&self, reason: String) {
        self.send_error(error_code::PROTOCOL_VIOLATION, &reason);
        self.finish(SessionEnd::Protocol(reason));
    }

    fn find(&self, id: u32) -> Option<Arc<Chan>> {
        lock(&self.state).channels.get(&id).cloned()
    }

    // ---- inbound ----

    fn handle_frame(self: &Arc<Self>, f: Frame, events: &mpsc::UnboundedSender<Event>) {
        match f.frame_type {
            FrameType::Ctrl => match f.ctrl_message() {
                Ok(m) => self.handle_ctrl(m, events),
                Err(e) => {
                    let msg = e.to_string();
                    let code = if msg.contains("unknown variant") {
                        error_code::UNKNOWN_MESSAGE
                    } else {
                        error_code::PROTOCOL_VIOLATION
                    };
                    self.send_error(code, &msg);
                }
            },
            FrameType::DataBin | FrameType::DataJson => {
                let Some(c) = self.find(f.channel) else {
                    self.send_error(
                        error_code::UNKNOWN_CHANNEL,
                        &format!("data for unknown channel {}", f.channel),
                    );
                    return;
                };
                let json = f.frame_type == FrameType::DataJson;
                if let Err(e) = c.on_data(json, f.payload) {
                    self.violation(&c, error_code::FLOW_CONTROL, &e.to_string());
                }
            }
            FrameType::Window => {
                let Some(c) = self.find(f.channel) else {
                    return;
                };
                let credit = f.window_credit().unwrap_or(0);
                if let Err(e) = c.on_window(credit) {
                    self.violation(&c, error_code::FLOW_CONTROL, &e.to_string());
                }
            }
            FrameType::Close => {
                let Some(c) = self.find(f.channel) else {
                    return;
                };
                if let Ok(info) = f.close_info() {
                    c.on_close(info);
                }
            }
        }
    }

    fn violation(&self, c: &Arc<Chan>, code: &str, msg: &str) {
        self.send_error(code, &format!("channel {}: {msg}", c.id));
        c.abort(msg);
    }

    fn handle_ctrl(self: &Arc<Self>, m: Ctrl, events: &mpsc::UnboundedSender<Event>) {
        let emit = |e: Event| {
            let _ = events.send(e);
        };
        match (self.role, m) {
            // Messages valid for either role.
            (_, Ctrl::Hello { .. }) | (Role::Client, Ctrl::Welcome { .. }) => {
                self.send_error(
                    error_code::PROTOCOL_VIOLATION,
                    "unexpected handshake message",
                );
            }
            (_, m @ Ctrl::Error { .. }) if !self.attribute_error(&m) => emit(Event::Ctrl(m)),
            (_, Ctrl::Error { .. }) => {}
            // ---- server ----
            (
                Role::Server,
                Ctrl::Open {
                    channel,
                    kind,
                    cols,
                    rows,
                    command,
                },
            ) => {
                let busy = {
                    let mut st = lock(&self.state);
                    if channel == 0 || st.in_use(channel) {
                        true
                    } else {
                        st.reserved.insert(channel);
                        false
                    }
                };
                if busy {
                    let _ = self.send_ctrl(&Ctrl::OpenError {
                        channel,
                        message: if channel == 0 {
                            "channel 0 is reserved".into()
                        } else {
                            "channel already in use".into()
                        },
                    });
                    return;
                }
                emit(Event::Open(OpenRequest {
                    session: Session::from_inner(self.clone()),
                    channel,
                    kind,
                    cols,
                    rows,
                    command,
                    decided: false,
                }));
            }
            (Role::Server, m @ Ctrl::Resize { .. }) => {
                let Ctrl::Resize { channel, .. } = &m else {
                    unreachable!()
                };
                if self.find(*channel).is_some() {
                    emit(Event::Ctrl(m));
                } else {
                    self.send_error(
                        error_code::UNKNOWN_CHANNEL,
                        &format!("resize for unknown channel {channel}"),
                    );
                }
            }
            (
                Role::Server,
                m @ (Ctrl::RunStart(_) | Ctrl::RunStatus { .. } | Ctrl::RunCancel { .. }),
            ) => {
                emit(Event::Ctrl(m));
            }
            // ---- client ----
            (Role::Client, Ctrl::Opened { channel }) => {
                let entry = {
                    let mut st = lock(&self.state);
                    let e = st.pending_open.remove(&channel);
                    if let Some((c, _)) = &e {
                        st.reserved.remove(&channel);
                        st.channels.insert(channel, c.clone());
                    }
                    e
                };
                match entry {
                    Some((c, tx)) => {
                        if tx.send(Ok(())).is_err() {
                            // The opener gave up; don't leave the channel dangling.
                            let _ = c.close_local(close_code::CANCELLED, "open cancelled");
                        }
                    }
                    None => self.send_error(
                        error_code::PROTOCOL_VIOLATION,
                        &format!("unsolicited opened for channel {channel}"),
                    ),
                }
            }
            (Role::Client, Ctrl::OpenError { channel, message }) => {
                let entry = {
                    let mut st = lock(&self.state);
                    st.reserved.remove(&channel);
                    st.pending_open.remove(&channel)
                };
                if let Some((_, tx)) = entry {
                    let _ = tx.send(Err(SessionError::OpenRejected(message)));
                }
            }
            (
                Role::Client,
                Ctrl::RunAccepted {
                    run_id,
                    state,
                    existing,
                },
            ) => {
                let found = {
                    let mut st = lock(&self.state);
                    let pos = st.pending.iter().position(
                        |p| matches!(p, Pending::RunStart { run_id: r, .. } if *r == run_id),
                    );
                    let p = pos.and_then(|i| st.pending.remove(i));
                    if let Some(Pending::RunStart { log: Some(c), .. }) = &p {
                        st.reserved.remove(&c.id);
                        st.channels.insert(c.id, c.clone());
                    }
                    p
                };
                match found {
                    Some(Pending::RunStart { log, tx, .. }) => {
                        let acc = RunAccepted {
                            run_id,
                            state,
                            existing,
                        };
                        if let Err(Ok((_, Some(c)))) = tx.send(Ok((acc, log))) {
                            let _ = c.close_local(close_code::CANCELLED, "run_start cancelled");
                        }
                    }
                    _ => self.send_error(
                        error_code::PROTOCOL_VIOLATION,
                        &format!("unsolicited run_accepted for {run_id}"),
                    ),
                }
            }
            (
                Role::Client,
                Ctrl::RunState {
                    run_id,
                    state,
                    exit_code,
                    summary,
                },
            ) => {
                let waiter = {
                    let mut st = lock(&self.state);
                    let pos = st.pending.iter().position(
                        |p| matches!(p, Pending::RunStatus { run_id: r, .. } if *r == run_id),
                    );
                    pos.and_then(|i| st.pending.remove(i))
                };
                let info = RunInfo {
                    run_id,
                    state,
                    exit_code,
                    summary,
                };
                match waiter {
                    Some(Pending::RunStatus { tx, .. }) => {
                        let _ = tx.send(Ok(info));
                    }
                    _ => emit(Event::Ctrl(Ctrl::RunState {
                        run_id: info.run_id,
                        state: info.state,
                        exit_code: info.exit_code,
                        summary: info.summary,
                    })),
                }
            }
            (Role::Client, m @ Ctrl::CredentialCaptured { .. }) => emit(Event::Ctrl(m)),
            (_, m) => {
                self.send_error(
                    error_code::PROTOCOL_VIOLATION,
                    &format!(
                        "unexpected message for the {:?} side: {}",
                        self.role,
                        ctrl_name(&m)
                    ),
                );
            }
        }
    }

    /// Route an `error` reply to the oldest pending request. Returns true when consumed.
    fn attribute_error(&self, m: &Ctrl) -> bool {
        let Ctrl::Error { code, message } = m else {
            return false;
        };
        if !matches!(
            code.as_str(),
            error_code::UNKNOWN_RUN
                | error_code::INTERNAL
                | error_code::UNAUTHORIZED
                | error_code::UNKNOWN_MESSAGE
        ) {
            return false;
        }
        let p = {
            let mut st = lock(&self.state);
            let p = st.pending.pop_front();
            if let Some(Pending::RunStart { log: Some(c), .. }) = &p {
                st.reserved.remove(&c.id);
            }
            p
        };
        match p {
            Some(p) => {
                p.fail(SessionError::Remote {
                    code: code.clone(),
                    message: message.clone(),
                });
                true
            }
            None => false,
        }
    }
}

fn ctrl_name(m: &Ctrl) -> &'static str {
    match m {
        Ctrl::Hello { .. } => "hello",
        Ctrl::Welcome { .. } => "welcome",
        Ctrl::Open { .. } => "open",
        Ctrl::Opened { .. } => "opened",
        Ctrl::OpenError { .. } => "open_error",
        Ctrl::Resize { .. } => "resize",
        Ctrl::RunStart(_) => "run_start",
        Ctrl::RunAccepted { .. } => "run_accepted",
        Ctrl::RunStatus { .. } => "run_status",
        Ctrl::RunState { .. } => "run_state",
        Ctrl::RunCancel { .. } => "run_cancel",
        Ctrl::CredentialCaptured { .. } => "credential_captured",
        Ctrl::Error { .. } => "error",
    }
}

impl Pending {
    fn fail(self, e: SessionError) {
        match self {
            Pending::RunStart { tx, .. } => {
                let _ = tx.send(Err(e));
            }
            Pending::RunStatus { tx, .. } => {
                let _ = tx.send(Err(e));
            }
        }
    }
}

async fn read_loop<R, E>(mut stream: R, inner: Arc<Inner>, events: mpsc::UnboundedSender<Event>)
where
    R: Stream<Item = Result<Message, E>> + Unpin,
    E: Display,
{
    let mut end = inner.end.subscribe();
    loop {
        let msg = tokio::select! {
            m = stream.next() => m,
            _ = end.wait_for(|e| e.is_some()) => return,
        };
        match msg {
            None | Some(Ok(Message::Close(_))) => {
                inner.finish(SessionEnd::PeerClosed);
                return;
            }
            Some(Err(e)) => {
                inner.finish(SessionEnd::Transport(e.to_string()));
                return;
            }
            Some(Ok(Message::Binary(b))) => match Frame::from_bytes(b) {
                Ok(f) => inner.handle_frame(f, &events),
                Err(e) => {
                    inner.fatal(format!("bad frame: {e}"));
                    return;
                }
            },
            Some(Ok(Message::Text(_))) => {
                inner.fatal("text websocket message".into());
                return;
            }
            Some(Ok(_)) => {} // ping/pong/raw
        }
    }
}

async fn write_loop<W, E>(mut sink: W, mut rx: mpsc::UnboundedReceiver<Out>, inner: Arc<Inner>)
where
    W: Sink<Message, Error = E> + Unpin,
    E: Display,
{
    while let Some(o) = rx.recv().await {
        match o {
            Out::Frame(b) => {
                if let Err(e) = sink.send(Message::Binary(b)).await {
                    inner.finish(SessionEnd::Transport(e.to_string()));
                    return;
                }
            }
            Out::Close => {
                let _ = sink.close().await;
                break;
            }
        }
    }
    inner.finish(SessionEnd::Closed);
}

/// A handshaken mux session. Cheap to clone; when the last clone is dropped
/// the session is closed gracefully. Channel handles do not keep it alive.
///
/// See the [module docs](super) for usage.
pub struct Session {
    pub(crate) inner: Arc<Inner>,
}

impl Clone for Session {
    fn clone(&self) -> Self {
        Self::from_inner(self.inner.clone())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.inner.handles.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.begin_close();
        }
    }
}

impl Session {
    fn from_inner(inner: Arc<Inner>) -> Self {
        inner.handles.fetch_add(1, Ordering::AcqRel);
        Self { inner }
    }

    fn start<S, E>(ws: S, role: Role, version: u16, peer: Option<ClientKind>) -> (Self, Events)
    where
        S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin + Send + 'static,
        E: Display + Send + 'static,
    {
        let (out, out_rx) = mpsc::unbounded_channel();
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            role,
            version,
            peer,
            out,
            state: Mutex::new(State::default()),
            end: watch::channel(None).0,
            handles: AtomicUsize::new(0),
        });
        let (sink, stream) = ws.split();
        tokio::spawn(write_loop(sink, out_rx, inner.clone()));
        tokio::spawn(read_loop(stream, inner.clone(), ev_tx));
        (Self::from_inner(inner), Events { rx: ev_rx })
    }

    /// Client side: send `hello`, wait for `welcome`, start the session.
    pub async fn connect<S, E>(
        mut ws: S,
        kind: ClientKind,
    ) -> Result<(Self, Events), HandshakeError>
    where
        S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin + Send + 'static,
        E: Display + Send + 'static,
    {
        let version = client_handshake(&mut ws, kind).await?;
        Ok(Self::start(ws, Role::Client, version, None))
    }

    /// Server side: wait for `hello`, negotiate, reply `welcome`, start the session.
    pub async fn accept<S, E>(
        mut ws: S,
        opts: ServerOptions,
    ) -> Result<(Self, Events), HandshakeError>
    where
        S: Stream<Item = Result<Message, E>> + Sink<Message, Error = E> + Unpin + Send + 'static,
        E: Display + Send + 'static,
    {
        let hello = server_handshake(&mut ws, &opts.supported_versions).await?;
        Ok(Self::start(
            ws,
            Role::Server,
            hello.version,
            Some(hello.client),
        ))
    }

    pub fn role(&self) -> Role {
        self.inner.role
    }

    /// Negotiated protocol version.
    pub fn version(&self) -> u16 {
        self.inner.version
    }

    /// (server) The `client` kind from `hello`.
    pub fn peer_kind(&self) -> Option<ClientKind> {
        self.inner.peer
    }

    pub fn is_closed(&self) -> bool {
        self.inner.end.borrow().is_some()
    }

    /// Resolves when the session has ended.
    pub async fn closed(&self) -> SessionEnd {
        let mut rx = self.inner.end.subscribe();
        match rx.wait_for(|e| e.is_some()).await {
            Ok(e) => e.clone().unwrap_or(SessionEnd::Closed),
            Err(_) => SessionEnd::Closed,
        }
    }

    /// Close gracefully: `CLOSE` (going away) on open channels, then the
    /// websocket. Resolves once the writer has flushed and finished.
    pub async fn close(&self) -> SessionEnd {
        self.inner.begin_close();
        self.closed().await
    }

    /// Send any CTRL message (escape hatch; prefer the typed methods).
    pub fn send_ctrl(&self, m: &Ctrl) -> Result<(), SessionError> {
        self.inner.send_ctrl(m)
    }

    /// Send an `error` CTRL message.
    pub fn send_error(&self, code: &str, message: &str) -> Result<(), SessionError> {
        self.inner.send_ctrl(&Ctrl::Error {
            code: code.into(),
            message: message.into(),
        })
    }

    fn require(&self, role: Role) -> Result<(), SessionError> {
        if self.inner.role == role {
            Ok(())
        } else {
            Err(SessionError::WrongRole(role))
        }
    }

    // ---- client API ----

    /// (client) Open a pty channel with a fresh id and wait for `opened`.
    pub async fn open_pty(
        &self,
        cols: u16,
        rows: u16,
        command: Option<Vec<String>>,
    ) -> Result<ChannelHandle, SessionError> {
        self.require(Role::Client)?;
        let (tx, rx) = oneshot::channel();
        let (id, chan) = {
            let mut st = lock(&self.inner.state);
            self.inner.check_alive()?;
            let id = st.alloc()?;
            let chan = Chan::new(id, &self.inner);
            st.reserved.insert(id);
            st.pending_open.insert(id, (chan.clone(), tx));
            (id, chan)
        };
        self.inner.send_ctrl(&Ctrl::Open {
            channel: id,
            kind: ChannelKind::Pty,
            cols,
            rows,
            command,
        })?;
        match rx.await {
            Ok(Ok(())) => Ok(ChannelHandle::new(chan)),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(self.inner.ended()),
        }
    }

    /// (client) Ask the server to resize a pty channel. No reply; failures
    /// arrive as `error` events.
    pub fn resize(&self, channel: u32, cols: u16, rows: u16) -> Result<(), SessionError> {
        self.require(Role::Client)?;
        self.inner.send_ctrl(&Ctrl::Resize {
            channel,
            cols,
            rows,
        })
    }

    /// (client) `run_start`. If `run.log_channel` is `Some(id)` the id must be
    /// unused; the returned [`RunStarted::log`] is that channel.
    pub async fn run_start(&self, run: RunStart) -> Result<RunStarted, SessionError> {
        self.run_start_inner(run, false).await
    }

    /// (client) Like [`run_start`](Self::run_start) but allocates the log
    /// channel id for you (overwriting `run.log_channel`).
    pub async fn run_start_logged(&self, run: RunStart) -> Result<RunStarted, SessionError> {
        self.run_start_inner(run, true).await
    }

    async fn run_start_inner(
        &self,
        mut run: RunStart,
        alloc: bool,
    ) -> Result<RunStarted, SessionError> {
        self.require(Role::Client)?;
        let (tx, rx) = oneshot::channel();
        let log = {
            let mut st = lock(&self.inner.state);
            self.inner.check_alive()?;
            if alloc {
                run.log_channel = Some(st.alloc()?);
            }
            let log = match run.log_channel {
                None => None,
                Some(0) => return Err(SessionError::ReservedChannel),
                Some(id) if st.in_use(id) => return Err(SessionError::ChannelInUse(id)),
                Some(id) => {
                    st.reserved.insert(id);
                    Some(Chan::new(id, &self.inner))
                }
            };
            st.pending.push_back(Pending::RunStart {
                run_id: run.run_id.clone(),
                log: log.clone(),
                tx,
            });
            log
        };
        let log_id = log.as_ref().map(|c| c.id);
        if let Err(e) = self.inner.send_ctrl(&Ctrl::RunStart(run)) {
            if let Some(id) = log_id {
                lock(&self.inner.state).reserved.remove(&id);
            }
            return Err(e);
        }
        match rx.await {
            Ok(Ok((accepted, log))) => Ok(RunStarted {
                accepted,
                log: log.map(ChannelHandle::new),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(self.inner.ended()),
        }
    }

    /// (client) `run_status`, resolved by the matching `run_state` reply.
    /// An `unknown_run` error reply becomes [`SessionError::Remote`].
    pub async fn run_status(&self, run_id: &str) -> Result<RunInfo, SessionError> {
        self.require(Role::Client)?;
        let (tx, rx) = oneshot::channel();
        {
            let mut st = lock(&self.inner.state);
            self.inner.check_alive()?;
            st.pending.push_back(Pending::RunStatus {
                run_id: run_id.into(),
                tx,
            });
        }
        self.inner.send_ctrl(&Ctrl::RunStatus {
            run_id: run_id.into(),
        })?;
        match rx.await {
            Ok(r) => r,
            Err(_) => Err(self.inner.ended()),
        }
    }

    /// (client) `run_cancel`. The new state arrives as a pushed `run_state` event.
    pub fn run_cancel(&self, run_id: &str) -> Result<(), SessionError> {
        self.require(Role::Client)?;
        self.inner.send_ctrl(&Ctrl::RunCancel {
            run_id: run_id.into(),
        })
    }

    // ---- server API ----

    /// (server) Answer `run_start` with `run_accepted`. When `log_channel` is
    /// `Some`, the transcript channel is registered (implicitly opened) first
    /// and returned; on `Err(ChannelInUse | ReservedChannel)` nothing was sent
    /// and the caller should reply with [`send_error`](Self::send_error).
    pub fn accept_run(
        &self,
        run_id: &str,
        state: RunPhase,
        existing: bool,
        log_channel: Option<u32>,
    ) -> Result<Option<ChannelHandle>, SessionError> {
        self.require(Role::Server)?;
        let chan = {
            let mut st = lock(&self.inner.state);
            self.inner.check_alive()?;
            match log_channel {
                None => None,
                Some(0) => return Err(SessionError::ReservedChannel),
                Some(id) if st.in_use(id) => return Err(SessionError::ChannelInUse(id)),
                Some(id) => {
                    let c = Chan::new(id, &self.inner);
                    st.channels.insert(id, c.clone());
                    Some(c)
                }
            }
        };
        self.inner.send_ctrl(&Ctrl::RunAccepted {
            run_id: run_id.into(),
            state,
            existing,
        })?;
        Ok(chan.map(ChannelHandle::new))
    }

    /// (server) Send `run_state`: the reply to `run_status`, or a push on change.
    pub fn send_run_state(&self, info: &RunInfo) -> Result<(), SessionError> {
        self.require(Role::Server)?;
        self.inner.send_ctrl(&Ctrl::RunState {
            run_id: info.run_id.clone(),
            state: info.state,
            exit_code: info.exit_code,
            summary: info.summary.clone(),
        })
    }
}
