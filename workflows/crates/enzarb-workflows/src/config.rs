//! Environment configuration with validation.

use std::collections::BTreeSet;
use std::net::SocketAddr;

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
    /// Empty unless the executor role is enabled.
    pub executor_callback_secret: Vec<u8>,
    /// Executor callback + health server.
    pub executor_listen: SocketAddr,
    /// Health server for processes without the executor role.
    pub health_listen: SocketAddr,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("roles", &self.roles)
            .field("database_url", &"<redacted>")
            .field("executor_url", &self.executor_url)
            .field("executor_callback_secret", &"<redacted>")
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
        let executor_listen = addr(get, "ENZARB_EXECUTOR_LISTEN", "0.0.0.0:8081")?;
        let health_listen = addr(get, "ENZARB_HEALTH_LISTEN", "0.0.0.0:8082")?;
        Ok(Config {
            roles,
            database_url,
            db_max_connections,
            bot_login: nonempty(get, "ENZARB_BOT_LOGIN").unwrap_or_else(|| "enzarb[bot]".into()),
            bot_handle: nonempty(get, "ENZARB_BOT_HANDLE").unwrap_or_else(|| "enzarb".into()),
            executor_url,
            executor_callback_secret,
            executor_listen,
            health_listen,
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
        assert_eq!(c.executor_listen.port(), 8081);
        assert_eq!(c.health_listen.port(), 8082);
        assert!(c.executor_callback_secret.is_empty());
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

    #[test]
    fn debug_redacts_secrets() {
        let c = Config::from_lookup(
            roles(&[Role::Executor]),
            &env(&[
                ("DATABASE_URL", "postgres://user:pw@h/db"),
                ("ENZARB_EXECUTOR_CALLBACK_SECRET", "topsecret"),
            ]),
        )
        .unwrap();
        let d = format!("{c:?}");
        assert!(!d.contains("pw@") && !d.contains("topsecret"), "{d}");
    }
}
