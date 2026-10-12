use crate::frame::FrameError;

/// Why a session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// This side closed the session (`Session::close` or the last handle was dropped).
    Closed,
    /// The peer closed the websocket (or the stream ended).
    PeerClosed,
    /// A fatal protocol violation (by either side); the connection was dropped.
    Protocol(String),
    /// The underlying websocket failed.
    Transport(String),
}

/// Errors returned by session and channel operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session ended: {0:?}")]
    Ended(SessionEnd),
    #[error("channel {0} is already in use")]
    ChannelInUse(u32),
    #[error("channel id 0 is reserved for control")]
    ReservedChannel,
    #[error("no free channel id")]
    NoFreeChannel,
    #[error("open rejected: {0}")]
    OpenRejected(String),
    /// `open_error` that carried a `code` (see [`crate::open_error_code`]).
    #[error("open rejected ({code}): {message}")]
    OpenRejectedCode { code: String, message: String },
    #[error("peer error {code}: {message}")]
    Remote { code: String, message: String },
    #[error("operation requires the {0:?} role")]
    WrongRole(super::Role),
    #[error("channel is closed for sending")]
    ChannelClosed,
    #[error("channel aborted: {0}")]
    ChannelAborted(String),
    #[error("json payload of {0} bytes exceeds the 256 KiB flow-control window")]
    JsonTooLarge(usize),
    #[error(transparent)]
    Frame(#[from] FrameError),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl SessionError {
    /// The `open_error.code`, when this is a coded open rejection.
    pub fn open_error_code(&self) -> Option<&str> {
        match self {
            Self::OpenRejectedCode { code, .. } => Some(code),
            _ => None,
        }
    }
}

/// Errors during the hello/welcome handshake.
#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("websocket error: {0}")]
    Transport(String),
    #[error("protocol violation: {0}")]
    Protocol(String),
    #[error("no common protocol version (offered {offered:?})")]
    UnsupportedVersion { offered: Vec<u16> },
    #[error("peer refused the handshake: {code}: {message}")]
    Rejected { code: String, message: String },
    #[error("connection closed during handshake")]
    Closed,
}

/// Errors from [`Client::connect`](super::Client::connect).
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("websocket connect: {0}")]
    Connect(#[from] tokio_tungstenite::tungstenite::Error),
    #[error("bearer token is not a valid header value")]
    BadToken,
    #[error(transparent)]
    Handshake(#[from] HandshakeError),
}

pub(crate) fn io_err(e: SessionError) -> std::io::Error {
    use std::io::ErrorKind;
    let kind = match &e {
        SessionError::Ended(_) | SessionError::ChannelAborted(_) => ErrorKind::ConnectionAborted,
        SessionError::ChannelClosed => ErrorKind::BrokenPipe,
        _ => ErrorKind::Other,
    };
    std::io::Error::new(kind, e.to_string())
}
