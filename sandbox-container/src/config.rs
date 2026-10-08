//! Environment configuration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Runtime configuration of sandbox-init.
#[derive(Debug, Clone)]
pub struct Config {
    /// `ENZARB_LISTEN`, default `0.0.0.0:7070`.
    pub listen: SocketAddr,
    /// `ENZARB_AUTH_TOKEN_FILE`, default `/var/run/enzarb/auth/token`.
    pub auth_token_file: PathBuf,
    /// `ENZARB_WORKSPACE`, default `/workspace`: working directory of PTYs.
    pub workspace: PathBuf,
    /// `ENZARB_SHELL`, then `$SHELL`, then `/bin/bash`: the login shell.
    pub shell: String,
    /// `ENZARB_GRACE_SECS`, default 25: how long children get after SIGTERM before SIGKILL.
    pub grace: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([0, 0, 0, 0], 7070).into(),
            auth_token_file: "/var/run/enzarb/auth/token".into(),
            workspace: "/workspace".into(),
            shell: "/bin/bash".into(),
            grace: Duration::from_secs(25),
        }
    }
}

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`from_env`](Self::from_env) with an injectable lookup (for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let mut c = Self::default();
        if let Some(v) = get("ENZARB_LISTEN") {
            c.listen = v
                .parse()
                .map_err(|e| anyhow::anyhow!("ENZARB_LISTEN={v:?}: {e}"))?;
        }
        if let Some(v) = get("ENZARB_AUTH_TOKEN_FILE") {
            c.auth_token_file = v.into();
        }
        if let Some(v) = get("ENZARB_WORKSPACE") {
            c.workspace = v.into();
        }
        if let Some(v) = get("ENZARB_SHELL")
            .or_else(|| get("SHELL"))
            .filter(|s| !s.is_empty())
        {
            c.shell = v;
        }
        if let Some(v) = get("ENZARB_GRACE_SECS") {
            let secs: u64 = v
                .parse()
                .map_err(|e| anyhow::anyhow!("ENZARB_GRACE_SECS={v:?}: {e}"))?;
            c.grace = Duration::from_secs(secs);
        }
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert_eq!(c.listen.port(), 7070);
        assert_eq!(
            c.auth_token_file,
            PathBuf::from("/var/run/enzarb/auth/token")
        );
        let c = Config::from_lookup(|k| match k {
            "ENZARB_AUTH_TOKEN_FILE" => Some("/t".into()),
            "ENZARB_GRACE_SECS" => Some("3".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.auth_token_file, PathBuf::from("/t"));
        assert_eq!(c.grace, Duration::from_secs(3));
        assert!(Config::from_lookup(|_| Some("x".into())).is_err());
    }
}
