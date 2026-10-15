//! Environment configuration with validation.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use std::time::Duration;

use engine::sandbox_poller::{MAX_INTERVAL_SECS, MIN_INTERVAL_SECS, PollerConfig};
use engine::{ClaudeKeyring, Secret};

use crate::cli::Role;

pub const DEFAULT_EXECUTOR_URL: &str = "http://enzarb-workflows-executor.enzarb.svc:8081";

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
    pub executor_url: String,
    /// `ENZARB_MAX_AUTO_ROUNDS`: automatic fix rounds on enzarb's own review findings per change
    /// request before it waits for a human (default 3, `0` disables).
    pub max_auto_rounds: u32,
    /// Empty unless the executor role is enabled.
    pub executor_callback_secret: Vec<u8>,
    /// Optional `read:packages` token for private npm packages, handed to agent runs
    /// (executor role only). From `ENZARB_NPM_TOKEN`, or the file named by
    /// `ENZARB_NPM_TOKEN_FILE` (takes precedence). Never logged.
    pub npm_token: Option<Secret>,
    /// Per-user Claude token keyring (executor role only), loaded from the directory named by
    /// `ENZARB_CLAUDE_KEYRING_DIR` (the mounted `enzarb-claude-key` Secret). Unset is `None`:
    /// every agent run then fails `credential_undecryptable`. Set but unreadable, empty or with
    /// a key that is not 32 bytes is a startup error.
    pub claude_keyring: Option<ClaudeKeyring>,
    /// Executor callback + health server.
    pub executor_listen: SocketAddr,
    /// Health server for processes without the executor role.
    pub health_listen: SocketAddr,
    /// Sandbox stats poller (ticker role): `ENZARB_STATS_INTERVAL` (seconds, default 30, clamped
    /// to 15-300), `ENZARB_STATS_TIMEOUT` (seconds, default 5), `ENZARB_STATS_CONCURRENCY`
    /// (default 8) and `ENZARB_STATS_RETENTION` (days, default 90).
    pub stats: PollerConfig,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("roles", &self.roles)
            .field("database_url", &"<redacted>")
            .field("executor_url", &self.executor_url)
            .field("executor_callback_secret", &"<redacted>")
            .field("npm_token", &self.npm_token.as_ref().map(|_| "<redacted>"))
            .field("claude_keyring", &self.claude_keyring)
            .field("executor_listen", &self.executor_listen)
            .field("health_listen", &self.health_listen)
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
fn npm_token(get: &dyn Fn(&str) -> Option<String>) -> Result<Option<Secret>, ConfigError> {
    let (var, raw) = match nonempty(get, "ENZARB_NPM_TOKEN_FILE") {
        Some(path) => (
            "ENZARB_NPM_TOKEN_FILE",
            std::fs::read_to_string(&path).map_err(|e| ConfigError::Invalid {
                var: "ENZARB_NPM_TOKEN_FILE",
                msg: format!("cannot read `{path}`: {}", e.kind()),
            })?,
        ),
        None => (
            "ENZARB_NPM_TOKEN",
            get("ENZARB_NPM_TOKEN").unwrap_or_default(),
        ),
    };
    let t = raw.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConfigError::Invalid {
            var,
            msg: "token contains whitespace or control characters".into(),
        });
    }
    Ok(Some(Secret::new(t)))
}

/// `ENZARB_CLAUDE_KEYRING_DIR`; unset or blank is `None`. Errors never include key bytes.
fn claude_keyring(
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<ClaudeKeyring>, ConfigError> {
    let Some(dir) = nonempty(get, "ENZARB_CLAUDE_KEYRING_DIR") else {
        return Ok(None);
    };
    ClaudeKeyring::load_dir(std::path::Path::new(&dir))
        .map(Some)
        .map_err(|e| ConfigError::Invalid {
            var: "ENZARB_CLAUDE_KEYRING_DIR",
            msg: e.to_string(),
        })
}

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
        let db_max_connections = match nonempty(get, "ENZARB_DB_MAX_CONNECTIONS") {
            None => 10,
            Some(v) => match v.parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => {
                    return Err(ConfigError::Invalid {
                        var: "ENZARB_DB_MAX_CONNECTIONS",
                        msg: format!("`{v}` is not a positive integer"),
                    });
                }
            },
        };
        let max_auto_rounds = match nonempty(get, "ENZARB_MAX_AUTO_ROUNDS") {
            None => 3,
            Some(v) => v.parse::<u32>().map_err(|_| ConfigError::Invalid {
                var: "ENZARB_MAX_AUTO_ROUNDS",
                msg: format!("`{v}` is not a non-negative integer"),
            })?,
        };
        let executor_url = nonempty(get, "ENZARB_EXECUTOR_URL")
            .unwrap_or_else(|| DEFAULT_EXECUTOR_URL.to_string());
        if roles.contains(&Role::Executor)
            && !(executor_url.starts_with("http://") || executor_url.starts_with("https://"))
        {
            return Err(ConfigError::Invalid {
                var: "ENZARB_EXECUTOR_URL",
                msg: format!("`{executor_url}` must start with http:// or https://"),
            });
        }
        let executor_callback_secret = if roles.contains(&Role::Executor) {
            nonempty(get, "ENZARB_EXECUTOR_CALLBACK_SECRET")
                .ok_or(ConfigError::Missing("ENZARB_EXECUTOR_CALLBACK_SECRET"))?
                .into_bytes()
        } else {
            Vec::new()
        };
        let npm_token = if roles.contains(&Role::Executor) {
            npm_token(get)?
        } else {
            None
        };
        let claude_keyring = if roles.contains(&Role::Executor) {
            claude_keyring(get)?
        } else {
            None
        };
        let executor_listen = addr(get, "ENZARB_EXECUTOR_LISTEN", "0.0.0.0:8081")?;
        let health_listen = addr(get, "ENZARB_HEALTH_LISTEN", "0.0.0.0:8082")?;
        Ok(Config {
            roles,
            database_url,
            db_max_connections,
            bot_login: nonempty(get, "ENZARB_BOT_LOGIN").unwrap_or_else(|| "enzarb[bot]".into()),
            bot_handle: nonempty(get, "ENZARB_BOT_HANDLE").unwrap_or_else(|| "enzarb".into()),
            executor_url,
            max_auto_rounds,
            executor_callback_secret,
            npm_token,
            claude_keyring,
            executor_listen,
            health_listen,
            stats: stats(get)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    fn roles(r: &[Role]) -> BTreeSet<Role> {
        r.iter().copied().collect()
    }

    #[test]
    fn defaults() {
        let c = Config::from_lookup(
            roles(&[Role::Ticker]),
            &env(&[("DATABASE_URL", "postgres://x")]),
        )
        .unwrap();
        assert_eq!(c.bot_login, "enzarb[bot]");
        assert_eq!(c.bot_handle, "enzarb");
        assert_eq!(c.executor_url, DEFAULT_EXECUTOR_URL);
        assert_eq!(c.max_auto_rounds, 3);
        assert_eq!(c.executor_listen.port(), 8081);
        assert_eq!(c.health_listen.port(), 8082);
        assert!(c.executor_callback_secret.is_empty());
    }

    fn stats_env(extra: &[(&str, &str)]) -> Result<PollerConfig, ConfigError> {
        let mut pairs = vec![("DATABASE_URL", "postgres://x")];
        pairs.extend_from_slice(extra);
        Config::from_lookup(roles(&[Role::Ticker]), &env(&pairs)).map(|c| c.stats)
    }

    #[test]
    fn stats_defaults_and_overrides() {
        let d = stats_env(&[]).unwrap();
        assert_eq!(d.interval, Duration::from_secs(30));
        assert_eq!(d.timeout, Duration::from_secs(5));
        assert_eq!((d.concurrency, d.retention_days), (8, 90));
        let c = stats_env(&[
            ("ENZARB_STATS_INTERVAL", "60"),
            ("ENZARB_STATS_TIMEOUT", "2"),
            ("ENZARB_STATS_CONCURRENCY", "4"),
            ("ENZARB_STATS_RETENTION", "7"),
        ])
        .unwrap();
        assert_eq!(c.interval, Duration::from_secs(60));
        assert_eq!(c.timeout, Duration::from_secs(2));
        assert_eq!((c.concurrency, c.retention_days), (4, 7));
    }

    #[test]
    fn stats_interval_is_clamped_not_rejected() {
        let secs = |v| stats_env(&[("ENZARB_STATS_INTERVAL", v)]).unwrap().interval;
        assert_eq!(secs("1"), Duration::from_secs(15));
        assert_eq!(secs("0"), Duration::from_secs(15));
        assert_eq!(secs("86400"), Duration::from_secs(300));
    }

    #[test]
    fn stats_rejects_zero_and_garbage() {
        for (var, v) in [
            ("ENZARB_STATS_INTERVAL", "soon"),
            ("ENZARB_STATS_INTERVAL", "-5"),
            ("ENZARB_STATS_TIMEOUT", "0"),
            ("ENZARB_STATS_TIMEOUT", "5s"),
            ("ENZARB_STATS_CONCURRENCY", "0"),
            ("ENZARB_STATS_RETENTION", "0"),
            ("ENZARB_STATS_RETENTION", "99999999999"),
        ] {
            assert!(
                matches!(stats_env(&[(var, v)]), Err(ConfigError::Invalid { var: e, .. }) if e == var),
                "{var}={v}"
            );
        }
    }

    #[test]
    fn database_url_required() {
        let e = Config::from_lookup(roles(&[Role::Ticker]), &env(&[])).unwrap_err();
        assert_eq!(e, ConfigError::Missing("DATABASE_URL"));
        assert_eq!(e.to_string(), "DATABASE_URL is required");
    }

    #[test]
    fn executor_needs_secret() {
        let e = Config::from_lookup(
            roles(&[Role::Executor]),
            &env(&[("DATABASE_URL", "postgres://x")]),
        )
        .unwrap_err();
        assert_eq!(e, ConfigError::Missing("ENZARB_EXECUTOR_CALLBACK_SECRET"));
        let c = Config::from_lookup(
            roles(&[Role::Executor]),
            &env(&[
                ("DATABASE_URL", "postgres://x"),
                ("ENZARB_EXECUTOR_CALLBACK_SECRET", "s3"),
                ("ENZARB_EXECUTOR_URL", "https://exec.example"),
            ]),
        )
        .unwrap();
        assert_eq!(c.executor_callback_secret, b"s3");
    }

    #[test]
    fn validates_values() {
        let base = [
            ("DATABASE_URL", "postgres://x"),
            ("ENZARB_EXECUTOR_CALLBACK_SECRET", "s"),
        ];
        let with = |extra: (&str, &str)| {
            let mut v = base.to_vec();
            v.push(extra);
            Config::from_lookup(roles(&[Role::Executor]), &env(&v)).unwrap_err()
        };
        assert!(matches!(
            with(("ENZARB_EXECUTOR_URL", "ftp://x")),
            ConfigError::Invalid {
                var: "ENZARB_EXECUTOR_URL",
                ..
            }
        ));
        assert!(matches!(
            with(("ENZARB_HEALTH_LISTEN", "nope")),
            ConfigError::Invalid {
                var: "ENZARB_HEALTH_LISTEN",
                ..
            }
        ));
        assert!(matches!(
            with(("ENZARB_DB_MAX_CONNECTIONS", "0")),
            ConfigError::Invalid {
                var: "ENZARB_DB_MAX_CONNECTIONS",
                ..
            }
        ));
    }

    fn exec_env<'a>(extra: &[(&'a str, &'a str)]) -> Result<Config, ConfigError> {
        let mut pairs = vec![
            ("DATABASE_URL", "postgres://h/db"),
            ("ENZARB_EXECUTOR_CALLBACK_SECRET", "s"),
        ];
        pairs.extend_from_slice(extra);
        Config::from_lookup(roles(&[Role::Executor]), &env(&pairs))
    }

    #[test]
    fn npm_token_is_optional_trimmed_and_executor_only() {
        assert!(exec_env(&[]).unwrap().npm_token.is_none());
        assert!(
            exec_env(&[("ENZARB_NPM_TOKEN", "  \n")])
                .unwrap()
                .npm_token
                .is_none()
        );
        let c = exec_env(&[("ENZARB_NPM_TOKEN", " ghp_x\n")]).unwrap();
        assert_eq!(c.npm_token.unwrap().expose(), "ghp_x");
        // not read for other roles
        let c = Config::from_lookup(
            roles(&[Role::Reducer]),
            &env(&[("DATABASE_URL", "p"), ("ENZARB_NPM_TOKEN", "ghp_x")]),
        )
        .unwrap();
        assert!(c.npm_token.is_none());
    }

    #[test]
    fn npm_token_file_wins_and_errors_do_not_leak() {
        let dir = std::env::temp_dir().join(format!("enzarb-npm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("tok");
        std::fs::write(&f, "ghp_file\n").unwrap();
        let p = f.to_str().unwrap();
        let c = exec_env(&[
            ("ENZARB_NPM_TOKEN", "ghp_env"),
            ("ENZARB_NPM_TOKEN_FILE", p),
        ])
        .unwrap();
        assert_eq!(c.npm_token.unwrap().expose(), "ghp_file");
        // unreadable file is an error, not a silent fallback
        let e = exec_env(&[
            ("ENZARB_NPM_TOKEN", "ghp_env"),
            (
                "ENZARB_NPM_TOKEN_FILE",
                dir.join("missing").to_str().unwrap(),
            ),
        ])
        .unwrap_err();
        assert!(matches!(
            e,
            ConfigError::Invalid {
                var: "ENZARB_NPM_TOKEN_FILE",
                ..
            }
        ));
        assert!(!e.to_string().contains("ghp_env"));
        // embedded whitespace is rejected without echoing the value
        let e = exec_env(&[("ENZARB_NPM_TOKEN", "ghp_a ghp_secretb")]).unwrap_err();
        assert!(!e.to_string().contains("secretb"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claude_keyring_is_executor_only_and_bad_dirs_fail_startup() {
        let dir = std::env::temp_dir().join(format!("enzarb-ckey-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("k1"), [0x5au8; 32]).unwrap();
        std::fs::write(dir.join("active"), "k1").unwrap();
        let p = dir.to_str().unwrap();
        assert!(exec_env(&[]).unwrap().claude_keyring.is_none());
        let c = exec_env(&[("ENZARB_CLAUDE_KEYRING_DIR", p)]).unwrap();
        let d = format!("{c:?}");
        assert!(d.contains("k1") && !d.contains("90"), "{d}");
        assert!(c.claude_keyring.is_some());
        let c = Config::from_lookup(
            roles(&[Role::Reducer]),
            &env(&[("DATABASE_URL", "p"), ("ENZARB_CLAUDE_KEYRING_DIR", p)]),
        )
        .unwrap();
        assert!(c.claude_keyring.is_none());
        std::fs::write(dir.join("k2"), b"short").unwrap();
        assert!(matches!(
            exec_env(&[("ENZARB_CLAUDE_KEYRING_DIR", p)]),
            Err(ConfigError::Invalid {
                var: "ENZARB_CLAUDE_KEYRING_DIR",
                ..
            })
        ));
        std::fs::remove_dir_all(&dir).ok();
        assert!(exec_env(&[("ENZARB_CLAUDE_KEYRING_DIR", p)]).is_err());
    }

    #[test]
    fn debug_redacts_secrets() {
        let c = Config::from_lookup(
            roles(&[Role::Executor]),
            &env(&[
                ("DATABASE_URL", "postgres://user:pw@h/db"),
                ("ENZARB_EXECUTOR_CALLBACK_SECRET", "topsecret"),
                ("ENZARB_NPM_TOKEN", "ghp_npmsecret"),
            ]),
        )
        .unwrap();
        let d = format!("{c:?}");
        assert!(!d.contains("pw@") && !d.contains("topsecret"), "{d}");
        assert!(!d.contains("ghp_npmsecret"), "{d}");
        let e = format!(
            "{:?}",
            engine::ExecutorConfig::new("http://x", b"s".to_vec())
        );
        assert!(!e.contains("ghp_"));
        let mut ec = engine::ExecutorConfig::new("http://x", b"s".to_vec());
        ec.npm_token = c.npm_token.clone();
        assert!(!format!("{ec:?}").contains("ghp_npmsecret"));
    }
}
