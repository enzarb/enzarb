use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result};

/// Minimum HS256 secret length in bytes.
pub const MIN_SECRET_BYTES: usize = 32;

/// Runtime configuration, read from the environment.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub jwt_secret: Vec<u8>,
    pub listen: SocketAddr,
    pub ping_interval: Duration,
    pub buffer: usize,
    pub max_token_ttl: Duration,
    pub max_connections: usize,
}

fn parsed<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    k: &str,
    default: T,
) -> Result<T>
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
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Build from an arbitrary variable lookup (empty values count as unset).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| lookup(k).filter(|v| !v.is_empty());
        let buffer: usize = parsed(&get, "ENZARB_REALTIME_BUFFER", 256)?;
        anyhow::ensure!(buffer > 0, "ENZARB_REALTIME_BUFFER must be > 0");
        let ping: u64 = parsed(&get, "ENZARB_REALTIME_PING_SECS", 15)?;
        let ttl: u64 = parsed(&get, "ENZARB_REALTIME_MAX_TOKEN_TTL_SECS", 300)?;
        anyhow::ensure!(ping > 0 && ttl > 0, "ping and token ttl must be > 0");
        let max_connections: usize = parsed(&get, "ENZARB_REALTIME_MAX_CONNECTIONS", 1000)?;
        anyhow::ensure!(
            max_connections > 0,
            "ENZARB_REALTIME_MAX_CONNECTIONS must be > 0"
        );
        let secret = get("ENZARB_REALTIME_JWT_SECRET")
            .context("ENZARB_REALTIME_JWT_SECRET is required")?
            .into_bytes();
        anyhow::ensure!(
            secret.len() >= MIN_SECRET_BYTES,
            "ENZARB_REALTIME_JWT_SECRET must be at least {MIN_SECRET_BYTES} bytes (got {})",
            secret.len()
        );
        Ok(Self {
            database_url: get("DATABASE_URL").context("DATABASE_URL is required")?,
            jwt_secret: secret,
            listen: parsed(&get, "ENZARB_REALTIME_LISTEN", "0.0.0.0:8080".parse()?)?,
            ping_interval: Duration::from_secs(ping),
            buffer,
            max_token_ttl: Duration::from_secs(ttl),
            max_connections,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(secret: &str, extra: &[(&str, &str)]) -> Result<Config> {
        let extra: Vec<(String, String)> = extra
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let secret = secret.to_string();
        Config::from_lookup(move |k| match k {
            "DATABASE_URL" => Some("postgres:///x".into()),
            "ENZARB_REALTIME_JWT_SECRET" => Some(secret.clone()),
            _ => extra.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()),
        })
    }

    #[test]
    fn short_secret_rejected_without_leaking_it() {
        let secret = "a".repeat(31);
        let err = cfg(&secret, &[]).err().expect("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("at least 32 bytes"), "{msg}");
        assert!(!msg.contains(&secret));
    }

    #[test]
    fn min_length_secret_accepted_and_defaults() {
        let c = cfg(&"a".repeat(32), &[]).unwrap();
        assert_eq!(c.max_connections, 1000);
    }

    #[test]
    fn zero_cap_rejected() {
        let r = cfg(&"a".repeat(32), &[("ENZARB_REALTIME_MAX_CONNECTIONS", "0")]);
        assert!(r.is_err());
    }

    #[test]
    fn cap_is_configurable() {
        let c = cfg(&"a".repeat(32), &[("ENZARB_REALTIME_MAX_CONNECTIONS", "5")]).unwrap();
        assert_eq!(c.max_connections, 5);
    }
}
