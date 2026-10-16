use std::time::Duration;

use crate::normalize::{NormalizeError, Normalized, normalize};

pub const DEFAULT_API_URL: &str = "https://api.github.com";
pub const DEFAULT_BOT_HANDLE: &str = "enzarb";
pub const DEFAULT_BOT_LOGIN: &str = "enzarb[bot]";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing environment variable {0}")]
    Missing(&'static str),
    #[error("invalid {var}: {reason}")]
    Invalid { var: &'static str, reason: String },
}

/// How the bot is addressed (`@handle`) and who it is (`login`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotIdentity {
    /// Mention handle without `@`. `ENZARB_BOT_HANDLE`, default `enzarb`.
    pub handle: String,
    /// The App's login. `ENZARB_BOT_LOGIN`, default `enzarb[bot]`.
    pub login: String,
}

impl Default for BotIdentity {
    fn default() -> Self {
        Self {
            handle: DEFAULT_BOT_HANDLE.into(),
            login: DEFAULT_BOT_LOGIN.into(),
        }
    }
}

impl BotIdentity {
    /// Reads `ENZARB_BOT_HANDLE` and `ENZARB_BOT_LOGIN` (contracts section 9).
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| d.to_string())
        };
        Self {
            handle: get("ENZARB_BOT_HANDLE", DEFAULT_BOT_HANDLE),
            login: get("ENZARB_BOT_LOGIN", DEFAULT_BOT_LOGIN),
        }
    }

    /// [`normalize`] with this identity. The receiver can use this without
    /// any App credentials.
    pub fn normalize(
        &self,
        event_name: &str,
        delivery_id: &str,
        body: &[u8],
    ) -> Result<Normalized, NormalizeError> {
        normalize(event_name, delivery_id, body, &self.handle, &self.login)
    }
}

/// Retry behaviour for 5xx and rate-limit responses.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts including the first.
    pub max_attempts: u32,
    /// First backoff; doubles each retry.
    pub base_delay: Duration,
    /// Upper bound for any single wait, including `Retry-After`.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// Everything a [`GitHubConnector`](crate::GitHubConnector) needs.
#[derive(Clone)]
pub struct GitHubConfig {
    pub app_id: String,
    /// PEM private key of the App.
    pub private_key_pem: String,
    pub bot: BotIdentity,
    /// REST base URL; override for GitHub Enterprise or tests.
    pub api_url: String,
    pub retry: RetryPolicy,
    /// Checks and statuses left out of the merge-readiness CI aggregate.
    pub ci_ignore: enzarb_connector::CiIgnore,
}

impl std::fmt::Debug for GitHubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubConfig")
            .field("app_id", &self.app_id)
            .field("private_key_pem", &"<redacted>")
            .field("bot", &self.bot)
            .field("api_url", &self.api_url)
            .field("retry", &self.retry)
            .field("ci_ignore", &self.ci_ignore)
            .finish()
    }
}

impl GitHubConfig {
    pub fn new(app_id: impl Into<String>, private_key_pem: impl Into<String>) -> Self {
        Self {
            app_id: app_id.into(),
            private_key_pem: private_key_pem.into(),
            bot: BotIdentity::default(),
            api_url: DEFAULT_API_URL.into(),
            retry: RetryPolicy::default(),
            ci_ignore: enzarb_connector::CiIgnore::default(),
        }
    }

    /// Reads `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY` (PEM; literal `\n`
    /// sequences are accepted for single-line env files), `ENZARB_BOT_HANDLE`,
    /// `ENZARB_BOT_LOGIN` and optionally `GITHUB_API_URL`.
    pub fn from_env() -> Result<Self, ConfigError> {
        let app_id =
            std::env::var("GITHUB_APP_ID").map_err(|_| ConfigError::Missing("GITHUB_APP_ID"))?;
        if app_id.trim().is_empty() {
            return Err(ConfigError::Missing("GITHUB_APP_ID"));
        }
        let pem = std::env::var("GITHUB_APP_PRIVATE_KEY")
            .map_err(|_| ConfigError::Missing("GITHUB_APP_PRIVATE_KEY"))?;
        let mut cfg = Self::new(app_id.trim(), pem.replace("\\n", "\n"));
        cfg.bot = BotIdentity::from_env();
        cfg.ci_ignore =
            enzarb_connector::CiIgnore::from_env().map_err(|e| ConfigError::Invalid {
                var: enzarb_connector::CI_IGNORE_CHECKS_ENV,
                reason: e.to_string(),
            })?;
        if let Ok(u) = std::env::var("GITHUB_API_URL")
            && !u.trim().is_empty()
        {
            cfg.api_url = u.trim().trim_end_matches('/').to_string();
        }
        Ok(cfg)
    }
}
