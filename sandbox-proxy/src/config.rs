//! Environment configuration.

use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use enzarb_proto::MAX_PAYLOAD_LEN;

/// Default `PROXY_IDLE_TIMEOUT_SECS`: 35 min, above the default chat idle.
pub const DEFAULT_IDLE_SECS: u64 = 2100;
/// Default `ENZARB_CHAT_IDLE_SECS` as set on the sandbox (the proxy only reads
/// it to validate its own idle timeout).
pub const DEFAULT_CHAT_IDLE_SECS: u64 = 1800;

/// Enforce `idle >= chat_idle`: an idle chat would otherwise be cut by the
/// proxy before the sandbox's own idle close. A smaller value is raised (and
/// logged) rather than refused, so an old chart value of 900 cannot crash-loop
/// the proxy on upgrade.
pub fn effective_idle(configured: Duration, chat_idle: Duration) -> Duration {
    if configured < chat_idle {
        tracing::warn!(
            configured_secs = configured.as_secs(),
            chat_idle_secs = chat_idle.as_secs(),
            "PROXY_IDLE_TIMEOUT_SECS is below ENZARB_CHAT_IDLE_SECS; raising it to the chat idle"
        );
        chat_idle
    } else {
        tracing::info!(
            idle_secs = configured.as_secs(),
            chat_idle_secs = chat_idle.as_secs(),
            "idle timeout"
        );
        configured
    }
}

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub jwt_secret: Vec<u8>,
    /// Close a connection with no traffic in either direction for this long.
    pub idle_timeout: Duration,
    /// Websocket ping interval towards the browser.
    pub ping_interval: Duration,
    /// Largest websocket message accepted from either side.
    pub max_message_bytes: usize,
    /// How long an auth token read from a Secret is reused.
    pub secret_ttl: Duration,
    /// How long to wait for the sandbox to accept and complete the handshake.
    pub dial_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let secret = std::env::var("ENZARB_SESSION_JWT_SECRET")
            .context("ENZARB_SESSION_JWT_SECRET is required")?;
        anyhow::ensure!(
            !secret.is_empty(),
            "ENZARB_SESSION_JWT_SECRET must not be empty"
        );
        Ok(Self {
            listen: var("PROXY_LISTEN", "0.0.0.0:8080")?
                .parse()
                .context("PROXY_LISTEN")?,
            jwt_secret: secret.into_bytes(),
            idle_timeout: effective_idle(
                Duration::from_secs(
                    var("PROXY_IDLE_TIMEOUT_SECS", &DEFAULT_IDLE_SECS.to_string())?
                        .parse()
                        .context("PROXY_IDLE_TIMEOUT_SECS")?,
                ),
                Duration::from_secs(
                    var("ENZARB_CHAT_IDLE_SECS", &DEFAULT_CHAT_IDLE_SECS.to_string())?
                        .parse()
                        .context("ENZARB_CHAT_IDLE_SECS")?,
                ),
            ),
            ping_interval: Duration::from_secs(
                var("PROXY_PING_INTERVAL_SECS", "30")?
                    .parse::<u64>()
                    .context("PROXY_PING_INTERVAL_SECS")?
                    .max(1),
            ),
            max_message_bytes: var("PROXY_MAX_MESSAGE_BYTES", &max_default().to_string())?
                .parse()
                .context("PROXY_MAX_MESSAGE_BYTES")?,
            secret_ttl: Duration::from_secs(
                var("PROXY_SECRET_TTL_SECS", "30")?
                    .parse()
                    .context("PROXY_SECRET_TTL_SECS")?,
            ),
            dial_timeout: Duration::from_secs(10),
        })
    }

    pub fn for_tests(secret: &[u8]) -> Self {
        Self {
            listen: "127.0.0.1:0".parse().unwrap(),
            jwt_secret: secret.to_vec(),
            idle_timeout: Duration::from_secs(30),
            ping_interval: Duration::from_secs(30),
            max_message_bytes: max_default(),
            secret_ttl: Duration::from_secs(30),
            dial_timeout: Duration::from_secs(5),
        }
    }
}

/// One maximum-size frame (header + 1 MiB payload) per websocket message.
fn max_default() -> usize {
    MAX_PAYLOAD_LEN + enzarb_proto::HEADER_LEN
}

fn var(name: &str, default: &str) -> Result<String> {
    Ok(std::env::var(name).unwrap_or_else(|_| default.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_at_least_chat_idle() {
        let d = Duration::from_secs;
        assert_eq!(effective_idle(d(900), d(1800)), d(1800));
        assert_eq!(effective_idle(d(2100), d(1800)), d(2100));
        assert_eq!(effective_idle(d(1800), d(1800)), d(1800));
        const { assert!(DEFAULT_IDLE_SECS >= DEFAULT_CHAT_IDLE_SECS) };
    }
}
