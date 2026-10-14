//! enzarb mux protocol: binary frame codec, typed CTRL messages and a pure
//! per-channel credit-based flow-control state machine.
//!
//! The wire format is specified in `docs/protocol.md`. Everything here is
//! free of I/O so it can be reused by the proxy, sandbox-init, the executor
//! and (mirrored) by the TypeScript package.
//!
//! With the `tokio` feature, [`session`] adds an async session runtime over a
//! websocket: handshake, channel table, flow-controlled channels, CTRL routing
//! and clean close. See its module docs for server and client examples.

pub mod ctrl;
pub mod flow;
pub mod frame;
#[cfg(feature = "tokio")]
pub mod session;

pub use ctrl::{
    ChannelKind, ClientKind, CredentialKind, Ctrl, MAX_REPO_LEN, MAX_RESUME_LEN, Retarget,
    RunFinished, RunOutcome, RunPhase, RunStart, Secret, Usage, close_reason, credential_scope,
    error_code, feature, open_error_code, sanitize_features,
};
pub use flow::{FlowControl, FlowError};
pub use frame::{
    CloseInfo, Frame, FrameError, FrameHeader, FrameType, HEADER_LEN, INITIAL_WINDOW, MAX_CREDIT,
    MAX_PAYLOAD_LEN, close_code,
};

/// Current (and only) protocol version.
pub const PROTOCOL_VERSION: u16 = 1;
/// Versions this implementation can speak, ascending.
pub const SUPPORTED_VERSIONS: &[u16] = &[1];

/// Pick the highest version offered by the client that we also support.
pub fn negotiate_version(client_versions: &[u16], supported: &[u16]) -> Option<u16> {
    client_versions
        .iter()
        .copied()
        .filter(|v| supported.contains(v))
        .max()
}
