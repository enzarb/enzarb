//! `sandbox-init`: the sandbox's pid-1 supervisor plus the enzarb-proto mux
//! server (contracts §3, §4).
//!
//! Modules:
//! * [`config`]: environment configuration;
//! * [`reaper`]: zombie reaping and child-exit delivery (the only place that waits on children);
//! * [`auth`]: bearer-token check against the token file;
//! * [`pty`]: PTY channels (`portable-pty` bridged to an enzarb-proto channel);
//! * [`observer`]: hook for watching PTY output;
//! * [`capture`]: Claude OAuth token capture from `claude setup-token` PTYs;
//! * [`privs`]: root pid 1 and the drop to uid 1000 for every spawned child (ADR-0016);
//! * [`dockerd`]: the supervised in-container Docker daemon (ADR-0016);
//! * [`docker_prune`]: automatic pruning of Docker images, build cache and volumes (ADR-0016);
//! * [`exec`]: running `git` under the reaper;
//! * [`acp`]: one agent run over the Agent Client Protocol (`claude-agent-acp`);
//! * [`chat`]: interactive chat over ACP (one agent per `acp` channel behind a filtering relay);
//! * [`policy`]: the permission policy that answers the agent's `session/request_permission`;
//! * [`agent`]: the agent [`RunManager`](runs::RunManager) (worktrees, ACP agent, callbacks);
//! * [`runs`]: the [`RunManager`](runs::RunManager) extension point for `run_*` messages;
//! * [`server`]: axum app, websocket adapter and the per-session event loop;
//! * [`stats`]: the on-demand cgroup/statvfs stats snapshot (`stats_request`).

pub mod acp;
pub mod agent;
pub mod auth;
pub mod capture;
pub mod chat;
pub mod config;
pub mod docker_prune;
pub mod dockerd;
pub mod exec;
pub mod netgit;
pub mod observer;
pub mod policy;
pub mod prepare;
pub mod privs;
pub mod pty;
pub mod reaper;
pub mod retarget;
pub mod runs;
pub mod segment;
pub mod server;
pub mod stats;

pub use config::{ChatMode, Config};
pub use server::{AppState, router, serve};
