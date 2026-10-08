//! Data channels: per-channel state shared between the session driver and the
//! user-facing [`ChannelHandle`].

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll, Waker};

use bytes::{Buf, Bytes};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

use super::core::{Inner, Out};
use super::error::io_err;
use super::{SessionEnd, SessionError};
use crate::flow::{FlowControl, FlowError};
use crate::frame::{CloseInfo, Frame, INITIAL_WINDOW, close_code};

/// Largest `DATA_BIN` frame the runtime emits (the spec recommends <= 16 KiB).
pub const SEND_CHUNK: usize = 16 * 1024;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One received data frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// `DATA_BIN` payload.
    Bin(Bytes),
    /// `DATA_JSON` payload (raw UTF-8 JSON, not yet parsed).
    Json(Bytes),
}

impl Data {
    pub fn bytes(&self) -> &Bytes {
        match self {
            Data::Bin(b) | Data::Json(b) => b,
        }
    }

    pub fn into_bytes(self) -> Bytes {
        match self {
            Data::Bin(b) | Data::Json(b) => b,
        }
    }
}

enum Fail {
    Session(SessionEnd),
    Aborted(String),
}

impl Fail {
    fn error(&self) -> SessionError {
        match self {
            Fail::Session(e) => SessionError::Ended(e.clone()),
            Fail::Aborted(r) => SessionError::ChannelAborted(r.clone()),
        }
    }
}

struct ChanState {
    flow: FlowControl,
    inbox: VecDeque<Data>,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    remote_close: Option<CloseInfo>,
    failed: Option<Fail>,
    recv_dropped: bool,
}

pub(crate) struct Chan {
    pub(crate) id: u32,
    out: mpsc::UnboundedSender<Out>,
    inner: Weak<Inner>,
    st: Mutex<ChanState>,
}

fn wake(w: Option<Waker>) {
    if let Some(w) = w {
        w.wake();
    }
}

impl Chan {
    pub(crate) fn new(id: u32, inner: &Arc<Inner>) -> Arc<Self> {
        Arc::new(Self {
            id,
            out: inner.out.clone(),
            inner: Arc::downgrade(inner),
            st: Mutex::new(ChanState {
                flow: FlowControl::default(),
                inbox: VecDeque::new(),
                read_waker: None,
                write_waker: None,
                remote_close: None,
                failed: None,
                recv_dropped: false,
            }),
        })
    }

    fn push(&self, f: Frame) {
        if let Ok(b) = f.to_bytes() {
            let _ = self.out.send(Out::Frame(b));
        }
    }

    fn grant(&self, g: Option<u32>) {
        if let Some(g) = g {
            self.push(Frame::window(self.id, g));
        }
    }

    fn forget(self: &Arc<Self>) {
        if let Some(i) = self.inner.upgrade() {
            i.forget_channel(self);
        }
    }

    // ---- driver side ----

    pub(crate) fn on_data(&self, json: bool, payload: Bytes) -> Result<(), FlowError> {
        let len = payload.len();
        let mut st = lock(&self.st);
        st.flow.on_data(len)?;
        if st.recv_dropped || st.failed.is_some() {
            // Nobody will read this: consume it immediately so the peer is not stalled.
            let g = st
                .flow
                .release(len)
                .ok()
                .flatten()
                .or_else(|| st.flow.flush_grant());
            drop(st);
            self.grant(g);
            return Ok(());
        }
        st.inbox.push_back(if json {
            Data::Json(payload)
        } else {
            Data::Bin(payload)
        });
        let w = st.read_waker.take();
        drop(st);
        wake(w);
        Ok(())
    }

    pub(crate) fn on_window(&self, credit: u32) -> Result<(), FlowError> {
        let mut st = lock(&self.st);
        st.flow.on_window(credit)?;
        let w = st.write_waker.take();
        drop(st);
        wake(w);
        Ok(())
    }

    pub(crate) fn on_close(self: &Arc<Self>, info: CloseInfo) {
        let mut st = lock(&self.st);
        st.flow.on_close_remote();
        st.remote_close = Some(info);
        let done = st.flow.is_closed();
        let (r, w) = (st.read_waker.take(), st.write_waker.take());
        drop(st);
        wake(r);
        wake(w);
        if done {
            self.forget();
        }
    }

    /// Protocol violation on this channel: close it with code 1 and fail it.
    pub(crate) fn abort(self: &Arc<Self>, reason: &str) {
        let mut st = lock(&self.st);
        let send_close = st.flow.close_local().is_ok();
        st.failed = Some(Fail::Aborted(reason.to_string()));
        st.inbox.clear();
        let (r, w) = (st.read_waker.take(), st.write_waker.take());
        drop(st);
        if send_close {
            self.push(Frame::close(self.id, close_code::PROTOCOL_ERROR, reason));
        }
        wake(r);
        wake(w);
        self.forget();
    }

    /// The session ended.
    pub(crate) fn fail(&self, end: &SessionEnd) {
        let mut st = lock(&self.st);
        if st.failed.is_none() {
            st.failed = Some(Fail::Session(end.clone()));
        }
        let (r, w) = (st.read_waker.take(), st.write_waker.take());
        drop(st);
        wake(r);
        wake(w);
    }

    pub(crate) fn is_local_closed(&self) -> bool {
        lock(&self.st).flow.is_local_closed()
    }

    pub(crate) fn close_local(
        self: &Arc<Self>,
        code: u16,
        reason: &str,
    ) -> Result<(), SessionError> {
        let mut st = lock(&self.st);
        if st.flow.close_local().is_err() {
            return Err(SessionError::ChannelClosed);
        }
        let done = st.flow.is_closed();
        let w = st.write_waker.take();
        drop(st);
        wake(w);
        self.push(Frame::close(self.id, code, reason));
        if done {
            self.forget();
        }
        Ok(())
    }

    // ---- user side ----

    /// Reserve up to `want` bytes of send credit (all of it when `exact`).
    fn poll_reserve(
        &self,
        cx: &mut Context<'_>,
        want: usize,
        exact: bool,
    ) -> Poll<Result<usize, SessionError>> {
        let mut st = lock(&self.st);
        if let Some(f) = &st.failed {
            return Poll::Ready(Err(f.error()));
        }
        if st.flow.is_local_closed() {
            return Poll::Ready(Err(SessionError::ChannelClosed));
        }
        let cap = if exact { want } else { want.min(SEND_CHUNK) };
        let n = st.flow.sendable(cap);
        if n == 0 || (exact && n < want) {
            st.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        st.flow.on_send(n).expect("sendable() fits credit");
        Poll::Ready(Ok(n))
    }

    fn poll_item(&self, cx: &mut Context<'_>) -> Poll<Option<Data>> {
        let mut st = lock(&self.st);
        if let Some(d) = st.inbox.pop_front() {
            let g = st.flow.release(d.bytes().len()).ok().flatten();
            drop(st);
            self.grant(g);
            return Poll::Ready(Some(d));
        }
        if st.remote_close.is_some() || st.failed.is_some() {
            return Poll::Ready(None);
        }
        st.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }

    fn poll_read_into(&self, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let mut st = lock(&self.st);
        if let Some(front) = st.inbox.front_mut() {
            let b = match front {
                Data::Bin(b) | Data::Json(b) => b,
            };
            let n = b.len().min(buf.remaining());
            if n == 0 {
                return Poll::Ready(Ok(()));
            }
            buf.put_slice(&b[..n]);
            b.advance(n);
            if b.is_empty() {
                st.inbox.pop_front();
            }
            let g = st.flow.release(n).ok().flatten();
            drop(st);
            self.grant(g);
            return Poll::Ready(Ok(()));
        }
        if st.remote_close.is_some() {
            return Poll::Ready(Ok(()));
        }
        if let Some(f) = &st.failed {
            return Poll::Ready(Err(io_err(f.error())));
        }
        st.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// Both halves of a channel. Implements [`AsyncRead`] and [`AsyncWrite`] for
/// byte (pty) channels; use [`split`](Self::split) or the public `send`/`recv`
/// fields for independent halves and JSON helpers.
///
/// A handle does not keep the [`Session`](super::Session) alive.
pub struct ChannelHandle {
    pub send: ChannelSender,
    pub recv: ChannelReceiver,
}

impl ChannelHandle {
    pub(crate) fn new(chan: Arc<Chan>) -> Self {
        Self {
            send: ChannelSender { chan: chan.clone() },
            recv: ChannelReceiver { chan },
        }
    }

    pub fn id(&self) -> u32 {
        self.send.chan.id
    }

    pub fn split(self) -> (ChannelSender, ChannelReceiver) {
        (self.send, self.recv)
    }
}

impl AsyncRead for ChannelHandle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for ChannelHandle {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.send).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.send).poll_shutdown(cx)
    }
}

/// Sending half. Writes wait for peer credit (backpressure); nothing is
/// dropped. Dropping it without [`close`](Self::close) sends `CLOSE` (normal),
/// which for a pty channel also terminates the process.
pub struct ChannelSender {
    chan: Arc<Chan>,
}

impl ChannelSender {
    pub fn id(&self) -> u32 {
        self.chan.id
    }

    /// Credit the peer currently allows us to send.
    pub fn credit(&self) -> u64 {
        lock(&self.chan.st).flow.send_credit()
    }

    /// Send `data` as `DATA_BIN` frames of at most [`SEND_CHUNK`] bytes,
    /// waiting for credit as needed.
    pub async fn send(&mut self, data: impl Into<Bytes>) -> Result<(), SessionError> {
        let mut data: Bytes = data.into();
        while !data.is_empty() {
            let n =
                std::future::poll_fn(|cx| self.chan.poll_reserve(cx, data.len(), false)).await?;
            self.chan
                .push(Frame::data_bin(self.chan.id, data.split_to(n)));
        }
        Ok(())
    }

    /// Send one `DATA_JSON` frame. The serialized value must fit in the
    /// initial window (256 KiB); the call waits until enough credit is available.
    pub async fn send_json<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), SessionError> {
        let body = serde_json::to_vec(value)?;
        if body.len() > INITIAL_WINDOW as usize {
            return Err(SessionError::JsonTooLarge(body.len()));
        }
        let n = std::future::poll_fn(|cx| self.chan.poll_reserve(cx, body.len(), true)).await?;
        debug_assert_eq!(n, body.len());
        self.chan
            .push(Frame::new(crate::FrameType::DataJson, self.chan.id, body));
        Ok(())
    }

    /// Half-close: send `CLOSE`. Receiving continues until the peer closes.
    pub fn close(&mut self, code: u16, reason: &str) -> Result<(), SessionError> {
        self.chan.close_local(code, reason)
    }

    pub fn is_closed(&self) -> bool {
        self.chan.is_local_closed()
    }
}

impl AsyncWrite for ChannelSender {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        match self.chan.poll_reserve(cx, buf.len(), false) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e)) => Poll::Ready(Err(io_err(e))),
            Poll::Ready(Ok(n)) => {
                self.chan.push(Frame::data_bin(
                    self.chan.id,
                    Bytes::copy_from_slice(&buf[..n]),
                ));
                Poll::Ready(Ok(n))
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Idempotent: an already-closed channel is fine.
        let _ = self.chan.close_local(close_code::NORMAL, "");
        Poll::Ready(Ok(()))
    }
}

impl Drop for ChannelSender {
    fn drop(&mut self) {
        let _ = self.chan.close_local(close_code::NORMAL, "");
    }
}

/// Receiving half. Credit is returned to the peer (`WINDOW`) as the
/// application consumes data, so a slow reader backpressures the sender.
pub struct ChannelReceiver {
    chan: Arc<Chan>,
}

impl ChannelReceiver {
    pub fn id(&self) -> u32 {
        self.chan.id
    }

    /// Next data frame, or `None` once the peer closed (or the session ended)
    /// and everything was delivered.
    pub async fn recv(&mut self) -> Option<Data> {
        std::future::poll_fn(|cx| self.chan.poll_item(cx)).await
    }

    /// Next `DATA_JSON` (or `DATA_BIN`) frame parsed as JSON.
    pub async fn recv_json<T: DeserializeOwned>(&mut self) -> Result<Option<T>, SessionError> {
        match self.recv().await {
            None => Ok(None),
            Some(d) => Ok(Some(serde_json::from_slice(d.bytes())?)),
        }
    }

    /// The peer's `CLOSE` code and reason, once received.
    pub fn close_info(&self) -> Option<CloseInfo> {
        lock(&self.chan.st).remote_close.clone()
    }
}

impl AsyncRead for ChannelReceiver {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.chan.poll_read_into(cx, buf)
    }
}

impl Drop for ChannelReceiver {
    fn drop(&mut self) {
        let mut st = lock(&self.chan.st);
        st.recv_dropped = true;
        let total: usize = st.inbox.drain(..).map(|d| d.bytes().len()).sum();
        let g = st
            .flow
            .release(total)
            .ok()
            .flatten()
            .or_else(|| st.flow.flush_grant());
        drop(st);
        self.chan.grant(g);
    }
}
