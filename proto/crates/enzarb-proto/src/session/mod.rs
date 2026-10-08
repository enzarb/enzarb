//! Async session runtime over a websocket (feature `tokio`).
//!
//! A [`Session`] turns any websocket `Stream<Item = Result<Message, E>> +
//! Sink<Message>` (tokio-tungstenite types, or the in-memory
//! [`memory::pair`]) into a handshaken, multiplexed connection:
//!
//! * the hello/welcome handshake and version negotiation (client or server role);
//! * a channel table; each data channel is a [`ChannelHandle`]
//!   (`AsyncRead`/`AsyncWrite` for pty bytes, `send_json`/`recv_json` for `DATA_JSON`);
//! * credit-based flow control with backpressure: writers wait for `WINDOW`
//!   credit, and credit is returned automatically as the application reads;
//! * CTRL routing: requests/replies are correlated for you, everything else is
//!   delivered as an [`Event`];
//! * error replies for violations (`unknown_channel`, `flow_control`, ...) and a
//!   clean close.
//!
//! Frame errors, text messages, and a bad handshake are fatal for the session.
//! Per-channel violations (credit overrun, data after the peer's `CLOSE`) send
//! `error` and `CLOSE` (code 1) for that channel only.
//!
//! # Server (sandbox-init)
//!
//! ```no_run
//! use enzarb_proto::session::{Event, ServerOptions, Session};
//! use enzarb_proto::{Ctrl, RunPhase};
//! use tokio::io::{AsyncReadExt, AsyncWriteExt};
//!
//! # async fn serve<S, E>(ws: S) -> Result<(), Box<dyn std::error::Error>>
//! # where S: futures_util::Stream<Item = Result<tokio_tungstenite::tungstenite::Message, E>>
//! #     + futures_util::Sink<tokio_tungstenite::tungstenite::Message, Error = E> + Unpin + Send + 'static,
//! #     E: std::fmt::Display + Send + 'static {
//! // `ws` is an upgraded websocket whose bearer token you already checked.
//! let (session, mut events) = Session::accept(ws, ServerOptions::default()).await?;
//! while let Some(event) = events.next().await {
//!     match event {
//!         Event::Open(req) => {
//!             let (cols, rows) = (req.cols, req.rows);
//!             let mut pty = req.accept()?; // sends `opened`
//!             tokio::spawn(async move {
//!                 // Echo the channel; spawn the real pty process in sandbox-init.
//!                 let mut buf = [0u8; 4096];
//!                 while let Ok(n) = pty.read(&mut buf).await {
//!                     if n == 0 { break; }
//!                     if pty.write_all(&buf[..n]).await.is_err() { break; }
//!                 }
//!                 let _ = (cols, rows);
//!             });
//!         }
//!         Event::Ctrl(Ctrl::RunStart(run)) => {
//!             let mut log = session.accept_run(&run.run_id, RunPhase::Queued, false, run.log_channel)?;
//!             if let Some(log) = log.as_mut() {
//!                 log.send.send_json(&serde_json::json!({"line": "started"})).await?;
//!             }
//!         }
//!         Event::Ctrl(Ctrl::Resize { channel, cols, rows }) => { /* ioctl(TIOCSWINSZ) */ }
//!         Event::Ctrl(_) => {}
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! # Client (executor, browser-side equivalents live in the TS package)
//!
//! ```no_run
//! use enzarb_proto::session::Client;
//! use enzarb_proto::{ClientKind, RunStart};
//! use tokio::io::AsyncWriteExt;
//!
//! # async fn go(run: RunStart) -> Result<(), Box<dyn std::error::Error>> {
//! let (session, _events) =
//!     Client::connect("ws://sandbox:7070/v1/mux", "token", ClientKind::Executor).await?;
//!
//! let mut pty = session.open_pty(80, 24, None).await?;
//! pty.write_all(b"ls\n").await?;
//!
//! let started = session.run_start_logged(run).await?; // allocates the log channel
//! let mut log = started.log.unwrap();
//! while let Some(line) = log.recv.recv_json::<serde_json::Value>().await? {
//!     println!("{line}");
//! }
//! let state = session.run_status(&started.accepted.run_id).await?;
//! println!("{:?}", state.state);
//! session.close().await;
//! # Ok(()) }
//! ```
//!
//! # Forwarding (sandbox-proxy)
//!
//! A forwarder does not need a `Session` after the handshakes: call
//! [`handshake::server_handshake`] on the browser socket and
//! [`handshake::client_handshake`] on the sandbox socket, then relay each
//! binary message, using [`FrameHeader::parse`](crate::FrameHeader::parse) when
//! it needs the type or channel. Flow control is end to end, so relaying
//! frames unmodified preserves it. Use a `Session` on both legs only if the
//! proxy must terminate channels itself.
//!
//! # Other websocket libraries
//!
//! The runtime speaks tokio-tungstenite's `Message`. For axum's `WebSocket`
//! adapt it with `StreamExt::map` / `SinkExt::with` (convert
//! `axum::extract::ws::Message::Binary` <-> `tungstenite::Message::Binary`).
//!
//! # Notes
//!
//! * Dropping the last [`Session`] clone closes the session; channel handles
//!   do not keep it alive.
//! * Dropping a [`ChannelSender`] sends `CLOSE`; dropping a [`ChannelReceiver`]
//!   discards (and credits back) further data.
//! * CTRL `error` replies are matched to the oldest pending `run_start` /
//!   `run_status` when the code is `unknown_run`, `internal`, `unauthorized` or
//!   `unknown_message`; other errors are delivered as events.

mod channel;
mod client;
mod core;
mod error;
pub mod handshake;
pub mod memory;

pub use channel::{ChannelHandle, ChannelReceiver, ChannelSender, Data, SEND_CHUNK};
pub use client::Client;
pub use core::{
    Event, Events, OpenRequest, Role, RunAccepted, RunInfo, RunStarted, ServerOptions, Session,
};
pub use error::{ConnectError, HandshakeError, SessionEnd, SessionError};
