//! Environment configuration.

#[derive(Debug, thiserror::Error)]
#[error("missing required environment variable {0}")]
pub struct ConfigError(pub &'static str);

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub webhook_secret: Vec<u8>,
    /// `RECEIVER_LISTEN`, default `0.0.0.0:8080`.
    pub listen: String,
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or(ConfigError(key))
}

impl Config {
    /// `DATABASE_URL` only; `migrate` needs nothing else.
    pub fn database_url() -> Result<String, ConfigError> {
        required("DATABASE_URL")
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            database_url: Self::database_url()?,
            webhook_secret: required("GITHUB_WEBHOOK_SECRET")?.into_bytes(),
            listen: std::env::var("RECEIVER_LISTEN")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| "0.0.0.0:8080".into()),
        })
    }
}
