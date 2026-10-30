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
    /// Route connector events to workflows (`ENZARB_TRACKS_ROUTING`); needs Postgres.
    pub routing: bool,
    /// GitHub App credentials for reading `.ez/*.toml` (optional; without them routing records
    /// an error decision per event).
    pub github_app_id: Option<String>,
    pub github_app_private_key: Option<String>,
    /// Most events routed at once (`ENZARB_TRACKS_CONCURRENCY`).
    pub concurrency: usize,
    /// Seconds between catch-up sweeps (`ENZARB_TRACKS_SWEEP_SECS`).
    pub sweep_secs: u64,
    /// Execute pending runs in the Projects' sandboxes (`ENZARB_TRACKS_EXECUTOR`); needs routing.
    pub executor: bool,
    /// Runs driven at once (`ENZARB_TRACKS_RUN_CONCURRENCY`).
    pub run_concurrency: usize,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen", &self.listen)
            .field("token", &"<redacted>")
            .field("max_body_bytes", &self.max_body_bytes)
            .field("routing", &self.routing)
            .field("github_app_id", &self.github_app_id)
            .field("github_app_private_key", &"<redacted>")
            .field("concurrency", &self.concurrency)
            .field("sweep_secs", &self.sweep_secs)
            .field("executor", &self.executor)
            .field("run_concurrency", &self.run_concurrency)
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
        let routing = match get("ENZARB_TRACKS_ROUTING").as_deref() {
            None | Some("false") | Some("0") => false,
            Some("true") | Some("1") => true,
            Some(v) => anyhow::bail!("invalid ENZARB_TRACKS_ROUTING {v:?} (true or false)"),
        };
        let concurrency = match get("ENZARB_TRACKS_CONCURRENCY") {
            Some(v) => v.parse().context("invalid ENZARB_TRACKS_CONCURRENCY")?,
            None => 4,
        };
        ensure!(
            (1..=64).contains(&concurrency),
            "ENZARB_TRACKS_CONCURRENCY must be 1-64"
        );
        let sweep_secs = match get("ENZARB_TRACKS_SWEEP_SECS") {
            Some(v) => v.parse().context("invalid ENZARB_TRACKS_SWEEP_SECS")?,
            None => 30,
        };
        ensure!(
            sweep_secs >= 1,
            "ENZARB_TRACKS_SWEEP_SECS must be at least 1"
        );
        let executor = match get("ENZARB_TRACKS_EXECUTOR").as_deref() {
            None | Some("false") | Some("0") => false,
            Some("true") | Some("1") => true,
            Some(v) => anyhow::bail!("invalid ENZARB_TRACKS_EXECUTOR {v:?} (true or false)"),
        };
        ensure!(
            !executor || routing,
            "ENZARB_TRACKS_EXECUTOR needs ENZARB_TRACKS_ROUTING"
        );
        let run_concurrency = match get("ENZARB_TRACKS_RUN_CONCURRENCY") {
            Some(v) => v.parse().context("invalid ENZARB_TRACKS_RUN_CONCURRENCY")?,
            None => 4,
        };
        ensure!(
            (1..=64).contains(&run_concurrency),
            "ENZARB_TRACKS_RUN_CONCURRENCY must be 1-64"
        );
        Ok(Self {
            listen,
            token,
            max_body_bytes,
            routing,
            github_app_id: get("ENZARB_GITHUB_APP_ID"),
            github_app_private_key: get("ENZARB_GITHUB_APP_PRIVATE_KEY"),
            concurrency,
            sweep_secs,
            executor,
            run_concurrency,
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
        assert!(!c.routing);
        assert_eq!((c.concurrency, c.sweep_secs), (4, 30));
        let r = cfg(&[
            ("ENZARB_TRACKS_TOKEN", TOKEN),
            ("ENZARB_TRACKS_ROUTING", "true"),
        ])
        .unwrap();
        assert!(r.routing);
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
