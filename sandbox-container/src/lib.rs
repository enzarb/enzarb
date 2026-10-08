//! `sandbox-init`: the sandbox's pid-1 supervisor plus the enzarb-proto mux
//! server (contracts §3, §4).
//!
//! Modules:
//! * [`config`]: environment configuration;
//! * [`reaper`]: zombie reaping and child-exit delivery (the only place that waits on children);
//! * [`auth`]: bearer-token check against the token file;
//! * [`pty`]: PTY channels (`portable-pty` bridged to an enzarb-proto channel);
//! * [`observer`]: hook for watching PTY output (token capture, next issue);
//! * [`runs`]: the [`RunManager`](runs::RunManager) extension point for `run_*` messages;
//! * [`server`]: axum app, websocket adapter and the per-session event loop.

pub mod auth;
pub mod config;
pub mod observer;
pub mod pty;
pub mod reaper;
pub mod runs;
pub mod server;

pub use config::Config;
pub use server::{AppState, router, serve};
