//! Hook point for watching PTY output.
//!
//! The next issue installs an observer that captures the Claude OAuth token
//! printed by `claude setup-token` and answers with `credential_captured`.
//! An [`PtyObserverFactory`] is asked once per PTY; the observer it returns
//! sees every output chunk, in order, before it is forwarded to the client.

use enzarb_proto::session::Session;

/// What an observer factory is told about a new PTY.
pub struct PtyInfo {
    /// Id of the mux session that opened it.
    pub session_id: u64,
    /// The session itself, e.g. to send `credential_captured`.
    pub session: Session,
    /// The channel id of the PTY.
    pub channel: u32,
    /// The requested command (`None` = login shell).
    pub command: Option<Vec<String>>,
}

/// Sees the raw output of one PTY. Called on the PTY's reader thread, so it
/// must not block for long. Chunk boundaries are arbitrary (a token may be
/// split across chunks); keep state in `self`.
pub trait PtyObserver: Send {
    fn on_output(&mut self, chunk: &[u8]);

    /// The PTY's output ended (the process and its descendants closed the terminal).
    fn on_close(&mut self) {}
}

/// Creates a [`PtyObserver`] per PTY, or `None` to skip that PTY.
pub trait PtyObserverFactory: Send + Sync + 'static {
    fn observe(&self, pty: &PtyInfo) -> Option<Box<dyn PtyObserver>>;
}
