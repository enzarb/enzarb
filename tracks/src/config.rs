//! Runtime configuration, read from the environment.

use std::net::SocketAddr;

use anyhow::{Context, Result, ensure};

/// Shortest accepted API token, in bytes.
pub const MIN_TOKEN_BYTES: usize = 32;
/// Default request body limit: room for a batch of large webhook payloads.
pub const DEFAULT_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct Config {
    pub listen: SocketAddr,
    /// Bearer token the UI sends (`ENZARB_TRACKS_TOKEN`).
    pub token: Vec<u8>,
    pub max_body_bytes: usize,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("token", &"<redacted>")
            .field("max_body_bytes", &self.max_body_bytes)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Build from an arbitrary variable lookup (empty values count as unset).
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self> {
        let get = |k: &str| lookup(k).filter(|v| !v.is_empty());
        let token = get("ENZARB_TRACKS_TOKEN")
            .context("ENZARB_TRACKS_TOKEN is required")?
            .into_bytes();
        ensure!(
            token.len() >= MIN_TOKEN_BYTES,
            "ENZARB_TRACKS_TOKEN must be at least {MIN_TOKEN_BYTES} bytes (got {})",
            token.len()
        );
        let listen = match get("ENZARB_TRACKS_LISTEN") {
            Some(v) => v.parse().context("invalid ENZARB_TRACKS_LISTEN")?,
            None => "0.0.0.0:8080".parse()?,
        };
        let max_body_bytes = match get("ENZARB_TRACKS_MAX_BODY_BYTES") {
            Some(v) => v.parse().context("invalid ENZARB_TRACKS_MAX_BODY_BYTES")?,
            None => DEFAULT_MAX_BODY_BYTES,
        };
        ensure!(
            max_body_bytes >= 64 * 1024,
            "ENZARB_TRACKS_MAX_BODY_BYTES must be at least 65536"
        );
        Ok(Self {
            listen,
            token,
            max_body_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(vars: &[(&str, &str)]) -> Result<Config> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(|k| m.get(k).cloned())
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn defaults_and_overrides() {
        let c = cfg(&[("ENZARB_TRACKS_TOKEN", TOKEN)]).unwrap();
        assert_eq!(c.listen.to_string(), "0.0.0.0:8080");
        assert_eq!(c.max_body_bytes, DEFAULT_MAX_BODY_BYTES);
        assert!(!format!("{c:?}").contains(TOKEN));
        let c = cfg(&[
            ("ENZARB_TRACKS_TOKEN", TOKEN),
            ("ENZARB_TRACKS_LISTEN", "127.0.0.1:9000"),
            ("ENZARB_TRACKS_MAX_BODY_BYTES", "100000"),
        ])
        .unwrap();
        assert_eq!(c.listen.port(), 9000);
        assert_eq!(c.max_body_bytes, 100_000);
    }

    #[test]
    fn rejects_bad_values() {
        let err = |v: &[(&str, &str)]| cfg(v).unwrap_err().to_string();
        assert!(err(&[]).contains("required"));
        assert!(err(&[("ENZARB_TRACKS_TOKEN", "")]).contains("required"));
        assert!(err(&[("ENZARB_TRACKS_TOKEN", "short")]).contains("at least 32"));
        assert!(
            err(&[
                ("ENZARB_TRACKS_TOKEN", TOKEN),
                ("ENZARB_TRACKS_LISTEN", "x")
            ])
            .contains("LISTEN")
        );
        assert!(
            err(&[
                ("ENZARB_TRACKS_TOKEN", TOKEN),
                ("ENZARB_TRACKS_MAX_BODY_BYTES", "10")
            ])
            .contains("65536")
        );
    }
}
