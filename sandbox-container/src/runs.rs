//! Extension point for agent runs (CTRL `run_*`, contracts §4).
//!
//! This issue ships only [`UnimplementedRunManager`]; the next issue
//! implements the real manager (worktrees, ACP agent, callbacks) behind the same
//! trait. Implementations must return quickly from the trait methods (spawn
//! long work) because they are called from the session's event loop.
//!
//! Replying: `start` must answer with [`Session::accept_run`] (which opens the
//! `log_channel` when requested) or [`Session::send_error`]; `status` answers
//! with [`Session::send_run_state`] (or `send_error("unknown_run", ..)`).
//! State changes are pushed with `send_run_state` on any live session; keep
//! the [`SessionCtx`] clones you need and drop them in `session_closed`.

use async_trait::async_trait;
use enzarb_proto::RunStart;
use enzarb_proto::session::Session;

/// One mux connection.
#[derive(Clone)]
pub struct SessionCtx {
    /// Process-unique id, passed again to [`RunManager::session_closed`].
    pub id: u64,
    pub session: Session,
}

#[async_trait]
pub trait RunManager: Send + Sync + 'static {
    /// CTRL `run_start`. Idempotent by `run_id` (answer `existing: true`).
    async fn start(&self, ctx: &SessionCtx, run: RunStart);

    /// CTRL `run_status`.
    async fn status(&self, ctx: &SessionCtx, run_id: &str);

    /// CTRL `run_cancel`.
    async fn cancel(&self, ctx: &SessionCtx, run_id: &str);

    /// The session ended. Runs are not tied to sessions and keep going.
    async fn session_closed(&self, _session_id: u64) {}
}

/// Answers every `run_*` message with `error {code: "unimplemented"}`.
pub struct UnimplementedRunManager;

fn unimplemented(ctx: &SessionCtx, what: &str) {
    let _ = ctx
        .session
        .send_error("unimplemented", &format!("{what} is not implemented"));
}

#[async_trait]
impl RunManager for UnimplementedRunManager {
    async fn start(&self, ctx: &SessionCtx, _run: RunStart) {
        unimplemented(ctx, "run_start");
    }
    async fn status(&self, ctx: &SessionCtx, _run_id: &str) {
        unimplemented(ctx, "run_status");
    }
    async fn cancel(&self, ctx: &SessionCtx, _run_id: &str) {
        unimplemented(ctx, "run_cancel");
    }
}
