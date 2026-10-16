//! Environment configuration with validation.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use std::time::Duration;

use engine::sandbox_poller::{MAX_INTERVAL_SECS, MIN_INTERVAL_SECS, PollerConfig};
use engine::secrets::SecretsKey;

use crate::cli::Role;

/// `ENZARB_DEFAULT_LIBRARY` default.
pub const DEFAULT_LIBRARY: &str = "enzarb/workflows@v1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{0} is required")]
    Missing(&'static str),
    #[error("{var} is invalid: {msg}")]
    Invalid { var: &'static str, msg: String },
}

#[derive(Clone)]
pub struct Config {
    pub roles: BTreeSet<Role>,
    pub database_url: String,
    pub db_max_connections: u32,
    pub bot_login: String,
    pub bot_handle: String,
    /// Health server (`/healthz`).
    pub health_listen: SocketAddr,
    /// Sandbox stats poller (ticker role): `ENZARB_STATS_INTERVAL`, `ENZARB_STATS_TIMEOUT`,
    /// `ENZARB_STATS_CONCURRENCY`, `ENZARB_STATS_RETENTION`.
    pub stats: PollerConfig,
    /// `ENZARB_DEFAULT_LIBRARY`: `owner/repo@ref` of the instance library.
    pub default_library: String,
    /// `ENZARB_LIBRARY_REPOS`: comma-separated extra allow-listed definition repos.
    pub library_repos: Vec<String>,
    /// `ENZARB_SECRETS_KEY`: 32 bytes, base64. Without it only not-yet-encrypted secrets resolve.
    pub secrets_key: Option<SecretsKey>,
    /// `ENZARB_CLAUDE_KEYRING_DIR` (the mounted `enzarb-claude-key` Secret) plus
    /// `ENZARB_CLAUDE_SECRET_NAME` (default `claude_token`): the user-scope secret answered by
    /// the UI-captured per-user Claude token.
    pub claude: Option<engine::secrets::ClaudeSource>,
    /// `ENZARB_PUBLIC_URL`: base URL of the UI (`instance.url` in task documents).
    pub public_url: String,
    /// `ENZARB_STALL_SECS` (default 1800).
    pub stall_secs: u64,
    /// `ENZARB_MAX_STEPS_PER_PROJECT` (default 4).
    pub max_steps_per_project: u64,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("roles", &self.roles)
            .field("database_url", &"<redacted>")
            .field("default_library", &self.default_library)
            .field("library_repos", &self.library_repos)
            .field(
                "secrets_key",
                &self.secrets_key.as_ref().map(|_| "<redacted>"),
            )
            .field("stall_secs", &self.stall_secs)
            .finish_non_exhaustive()
    }
}

fn nonempty(get: &dyn Fn(&str) -> Option<String>, var: &str) -> Option<String> {
    get(var)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn addr(
    get: &dyn Fn(&str) -> Option<String>,
    var: &'static str,
    default: &str,
) -> Result<SocketAddr, ConfigError> {
    let v = nonempty(get, var).unwrap_or_else(|| default.to_string());
    v.parse().map_err(|e| ConfigError::Invalid {
        var,
        msg: format!("`{v}`: {e}"),
    })
}

/// A positive integer setting; unset or blank is `default`.
fn positive(
    get: &dyn Fn(&str) -> Option<String>,
    var: &'static str,
    default: u64,
) -> Result<u64, ConfigError> {
    match nonempty(get, var) {
        None => Ok(default),
        Some(v) => match v.parse::<u64>() {
            Ok(n) if n > 0 => Ok(n),
            _ => Err(ConfigError::Invalid {
                var,
                msg: format!("`{v}` is not a positive integer"),
            }),
        },
    }
}

/// The `ENZARB_STATS_*` settings. A non-numeric value is an error; an interval outside the
/// accepted range is clamped (and logged), not rejected.
fn stats(get: &dyn Fn(&str) -> Option<String>) -> Result<PollerConfig, ConfigError> {
    let asked = match nonempty(get, "ENZARB_STATS_INTERVAL") {
        None => 30,
        Some(v) => v.parse::<u64>().map_err(|_| ConfigError::Invalid {
            var: "ENZARB_STATS_INTERVAL",
            msg: format!("`{v}` is not a non-negative integer"),
        })?,
    };
    let mut cfg = PollerConfig::with_interval_secs(asked);
    if cfg.interval.as_secs() != asked {
        tracing::warn!(
            asked,
            used = cfg.interval.as_secs(),
            "ENZARB_STATS_INTERVAL is outside {MIN_INTERVAL_SECS}-{MAX_INTERVAL_SECS} seconds; clamped"
        );
    }
    cfg.timeout = Duration::from_secs(positive(get, "ENZARB_STATS_TIMEOUT", 5)?);
    cfg.concurrency =
        usize::try_from(positive(get, "ENZARB_STATS_CONCURRENCY", 8)?).unwrap_or(usize::MAX);
    cfg.retention_days =
        i32::try_from(positive(get, "ENZARB_STATS_RETENTION", 90)?).map_err(|_| {
            ConfigError::Invalid {
                var: "ENZARB_STATS_RETENTION",
                msg: "too many days".into(),
            }
        })?;
    Ok(cfg)
}

/// `ENZARB_NPM_TOKEN_FILE` (preferred) or `ENZARB_NPM_TOKEN`; unset or blank is `None`.
/// Errors never include the value.
impl Config {
    /// Build from the process environment.
    pub fn from_env(roles: BTreeSet<Role>) -> Result<Self, ConfigError> {
        Self::from_lookup(roles, &|k| std::env::var(k).ok())
    }

    /// Build from an arbitrary variable lookup (tests).
    pub fn from_lookup(
        roles: BTreeSet<Role>,
        get: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        if roles.is_empty() {
            return Err(ConfigError::Missing("--roles"));
        }
        let database_url =
            nonempty(get, "DATABASE_URL").ok_or(ConfigError::Missing("DATABASE_URL"))?;
        let db_max_connections = u32::try_from(positive(get, "ENZARB_DB_MAX_CONNECTIONS", 10)?)
            .map_err(|_| ConfigError::Invalid {
                var: "ENZARB_DB_MAX_CONNECTIONS",
                msg: "too large".into(),
            })?;
        let default_library =
            nonempty(get, "ENZARB_DEFAULT_LIBRARY").unwrap_or_else(|| DEFAULT_LIBRARY.into());
        if !default_library.contains('/') || !default_library.contains('@') {
            return Err(ConfigError::Invalid {
                var: "ENZARB_DEFAULT_LIBRARY",
                msg: format!("`{default_library}` must be owner/repo@ref"),
            });
        }
        let library_repos = nonempty(get, "ENZARB_LIBRARY_REPOS")
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let secrets_key = match nonempty(get, "ENZARB_SECRETS_KEY") {
            None => None,
            Some(v) => Some(
                SecretsKey::from_base64(&v).map_err(|msg| ConfigError::Invalid {
                    var: "ENZARB_SECRETS_KEY",
                    msg,
                })?,
            ),
        };
        let claude = match nonempty(get, "ENZARB_CLAUDE_KEYRING_DIR") {
            None => None,
            Some(dir) => Some(engine::secrets::ClaudeSource {
                name: nonempty(get, "ENZARB_CLAUDE_SECRET_NAME")
                    .unwrap_or_else(|| "claude_token".into()),
                keyring: engine::claude_credential::ClaudeKeyring::load_dir(std::path::Path::new(
                    &dir,
                ))
                .map_err(|e| ConfigError::Invalid {
                    var: "ENZARB_CLAUDE_KEYRING_DIR",
                    msg: e.to_string(),
                })?,
            }),
        };
        Ok(Config {
            public_url: nonempty(get, "ENZARB_PUBLIC_URL")
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default(),
            claude,
            roles,
            database_url,
            db_max_connections,
            bot_login: nonempty(get, "ENZARB_BOT_LOGIN").unwrap_or_else(|| "enzarb[bot]".into()),
            bot_handle: nonempty(get, "ENZARB_BOT_HANDLE").unwrap_or_else(|| "enzarb".into()),
            health_listen: addr(get, "ENZARB_HEALTH_LISTEN", "0.0.0.0:8082")?,
            stats: stats(get)?,
            default_library,
            library_repos,
            secrets_key,
            stall_secs: positive(get, "ENZARB_STALL_SECS", 1800)?,
            max_steps_per_project: positive(get, "ENZARB_MAX_STEPS_PER_PROJECT", 4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(kv: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| kv.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn defaults_and_validation() {
        let roles: BTreeSet<Role> = [Role::Reducer].into();
        let c =
            Config::from_lookup(roles.clone(), &env(&[("DATABASE_URL", "postgres://x")])).unwrap();
        assert_eq!(c.default_library, DEFAULT_LIBRARY);
        assert_eq!(c.stall_secs, 1800);
        assert_eq!(c.health_listen.port(), 8082);
        assert!(c.secrets_key.is_none());
        let bad = Config::from_lookup(
            roles.clone(),
            &env(&[
                ("DATABASE_URL", "postgres://x"),
                ("ENZARB_DEFAULT_LIBRARY", "nope"),
            ]),
        );
        assert!(bad.is_err());
        let c = Config::from_lookup(
            roles,
            &env(&[
                ("DATABASE_URL", "postgres://x"),
                ("ENZARB_LIBRARY_REPOS", "Enzarb/Workflows, a/b"),
                ("ENZARB_STALL_SECS", "60"),
            ]),
        )
        .unwrap();
        assert_eq!(c.library_repos, vec!["enzarb/workflows", "a/b"]);
        assert_eq!(c.stall_secs, 60);
    }
}
