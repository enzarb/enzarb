//! Environment configuration.

use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};
use enzarb_proto::MAX_PAYLOAD_LEN;

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    pub jwt_secret: Vec<u8>,
    /// Close a connection with no traffic in either direction for this long.
    pub idle_timeout: Duration,
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
            idle_timeout: Duration::from_secs(
                var("PROXY_IDLE_TIMEOUT_SECS", "900")?
                    .parse()
                    .context("PROXY_IDLE_TIMEOUT_SECS")?,
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
