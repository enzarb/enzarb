use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};

/// Runtime configuration, read from the environment.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: Vec<u8>,
    pub listen: SocketAddr,
    pub ping_interval: Duration,
    pub buffer: usize,
    pub max_token_ttl: Duration,
}

fn get(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn parsed<T: std::str::FromStr>(k: &str, default: T) -> Result<T>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    match get(k) {
        Some(v) => v.parse().with_context(|| format!("invalid {k}")),
        None => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let buffer: usize = parsed("ENZARB_REALTIME_BUFFER", 256)?;
        anyhow::ensure!(buffer > 0, "ENZARB_REALTIME_BUFFER must be > 0");
        let ping: u64 = parsed("ENZARB_REALTIME_PING_SECS", 15)?;
        let ttl: u64 = parsed("ENZARB_REALTIME_MAX_TOKEN_TTL_SECS", 300)?;
        anyhow::ensure!(ping > 0 && ttl > 0, "ping and token ttl must be > 0");
        Ok(Self {
            database_url: get("DATABASE_URL").context("DATABASE_URL is required")?,
            jwt_secret: get("ENZARB_REALTIME_JWT_SECRET")
                .context("ENZARB_REALTIME_JWT_SECRET is required")?
                .into_bytes(),
            listen: parsed("ENZARB_REALTIME_LISTEN", "0.0.0.0:8080".parse()?)?,
            ping_interval: Duration::from_secs(ping),
            buffer,
            max_token_ttl: Duration::from_secs(ttl),
        })
    }
}
