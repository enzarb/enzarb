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
    /// `ENZARB_CLAUDE_BIN`, default `claude`.
    pub claude_bin: String,
    /// `ENZARB_CLAUDE_ARGS` (whitespace separated), default
    /// `--output-format json --permission-mode bypassPermissions`. `-p` is always added;
    /// the prompt is written to claude's stdin.
    pub claude_args: Vec<String>,
    /// `ENZARB_CLAUDE_TOKEN_FILE`, default `<workspace>/.enzarb/claude-token`.
    pub claude_token_file: PathBuf,
    /// `ENZARB_GIT_BIN`, default `git`.
    pub git_bin: String,
    /// `ENZARB_GIT_NAME` / `ENZARB_GIT_EMAIL`: identity for commits made in runs.
    pub git_name: String,
    pub git_email: String,
    /// `ENZARB_MAX_RUNS`, default 2: runs executing at once (others stay `queued`).
    pub max_runs: usize,
    /// `ENZARB_RUN_TIMEOUT_SECS`, default 3600: used when `run_start.timeout_secs` is 0.
    pub default_timeout: Duration,
    /// `ENZARB_CALLBACK_BACKOFF_MS`, default 1000: first retry delay of the
    /// run-finished callback (doubles up to 60 s).
    pub callback_backoff: Duration,
}

/// Default for [`Config::claude_args`].
pub const DEFAULT_CLAUDE_ARGS: &str = "--output-format json --permission-mode bypassPermissions";

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([0, 0, 0, 0], 7070).into(),
            auth_token_file: "/var/run/enzarb/auth/token".into(),
            workspace: "/workspace".into(),
            shell: "/bin/bash".into(),
            grace: Duration::from_secs(25),
            claude_bin: "claude".into(),
            claude_args: DEFAULT_CLAUDE_ARGS
                .split_whitespace()
                .map(String::from)
                .collect(),
            claude_token_file: "/workspace/.enzarb/claude-token".into(),
            git_bin: "git".into(),
            git_name: "enzarb-agent".into(),
            git_email: "agent@enzarb.dev".into(),
            max_runs: 2,
            default_timeout: Duration::from_secs(3600),
            callback_backoff: Duration::from_secs(1),
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
        c.claude_token_file = c.workspace.join(".enzarb/claude-token");
        if let Some(v) = get("ENZARB_CLAUDE_TOKEN_FILE") {
            c.claude_token_file = v.into();
        }
        if let Some(v) = get("ENZARB_CLAUDE_BIN").filter(|s| !s.is_empty()) {
            c.claude_bin = v;
        }
        if let Some(v) = get("ENZARB_CLAUDE_ARGS") {
            c.claude_args = v.split_whitespace().map(String::from).collect();
        }
        if let Some(v) = get("ENZARB_GIT_BIN").filter(|s| !s.is_empty()) {
            c.git_bin = v;
        }
        if let Some(v) = get("ENZARB_GIT_NAME").filter(|s| !s.is_empty()) {
            c.git_name = v;
        }
        if let Some(v) = get("ENZARB_GIT_EMAIL").filter(|s| !s.is_empty()) {
            c.git_email = v;
        }
        if let Some(v) = get("ENZARB_MAX_RUNS") {
            c.max_runs = v
                .parse::<usize>()
                .map_err(|e| anyhow::anyhow!("ENZARB_MAX_RUNS={v:?}: {e}"))?
                .max(1);
        }
        if let Some(v) = get("ENZARB_RUN_TIMEOUT_SECS") {
            c.default_timeout = Duration::from_secs(
                v.parse()
                    .map_err(|e| anyhow::anyhow!("ENZARB_RUN_TIMEOUT_SECS={v:?}: {e}"))?,
            );
        }
        if let Some(v) = get("ENZARB_CALLBACK_BACKOFF_MS") {
            c.callback_backoff = Duration::from_millis(
                v.parse()
                    .map_err(|e| anyhow::anyhow!("ENZARB_CALLBACK_BACKOFF_MS={v:?}: {e}"))?,
            );
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
