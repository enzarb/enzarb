use std::time::Duration;

/// Errors a connector call can return.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The provider asked us to slow down.
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    /// A failure worth retrying (network, 5xx).
    #[error("transient provider error: {0}")]
    Transient(String),
    #[error("provider error: {0}")]
    Other(String),
}

impl Error {
    /// Whether retrying the same call (same idempotency key) may succeed.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Error::RateLimited { .. } | Error::Transient(_))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
