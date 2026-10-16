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
//! use enzarb_proto::{Ctrl, StepPhase};
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
//!         Event::Ctrl(Ctrl::StepStart(step)) => {
//!             session.accept_step(&step.run_id, StepPhase::Running, false)?;
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
//! use enzarb_proto::{ClientKind, Ctrl, StepStart};
//! use enzarb_proto::session::Event;
//! use tokio::io::AsyncWriteExt;
//!
//! # async fn go(step: StepStart) -> Result<(), Box<dyn std::error::Error>> {
//! let (session, mut events) =
//!     Client::connect("ws://sandbox:7070/v1/mux", "token", ClientKind::Executor).await?;
//!
//! let mut pty = session.open_pty(80, 24, None).await?;
//! pty.write_all(b"ls\n").await?;
//!
//! let run_id = step.run_id.clone();
//! session.send_ctrl(&Ctrl::StepStart(Box::new(step)))?;
//! session.send_ctrl(&Ctrl::StepStatus { run_id, after_seq: 0 })?;
//! while let Some(ev) = events.next().await {
//!     match ev {
//!         Event::Ctrl(Ctrl::SessionEvent(e)) => println!("{} {}", e.seq, e.kind),
//!         Event::Ctrl(Ctrl::StepFinished(f)) => { println!("{}", f.exit); break; }
//!         _ => {}
//!     }
//! }
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
//! * CTRL `error` replies are matched to the oldest pending `step_start` /
//!   `step_status` when the code is `unknown_run`, `internal`, `unauthorized`,
//!   `unknown_message` or `unimplemented`; an `error` carrying `run_id` is instead attributed to the pending request for that run (any code); other errors are delivered as events.

mod channel;
mod client;
mod core;
mod error;
pub mod handshake;
pub mod memory;

pub use channel::{ChannelHandle, ChannelReceiver, ChannelSender, Data, SEND_CHUNK};
pub use client::Client;
pub use core::{
    CredentialInfo, Event, Events, OpenRequest, Role, ServerOptions, Session, StepAccepted,
    StepInfo,
};
pub use error::{ConnectError, HandshakeError, SessionEnd, SessionError};
