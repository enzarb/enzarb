//! Environment configuration (contracts section 9).

use enzarb_connector_github::BotIdentity;

use crate::secrets::SecretsKey;

/// Project secret holding the custom-event HMAC key, unless `ENZARB_EVENTS_SECRET_NAME`.
pub const DEFAULT_EVENTS_SECRET_NAME: &str = "events_hmac";

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid {0}: {1}")]
    Invalid(&'static str, String),
}

#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub webhook_secret: Vec<u8>,
    pub bot: BotIdentity,
    /// `RECEIVER_LISTEN`, default `0.0.0.0:8080`.
    pub listen: String,
    /// `ENZARB_EVENTS_SECRET_NAME`, default [`DEFAULT_EVENTS_SECRET_NAME`].
    pub events_secret_name: String,
    /// `ENZARB_SECRETS_KEY` (base64 of 32 bytes); optional.
    pub secrets_key: Option<SecretsKey>,
}

fn optional(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

fn required(key: &'static str) -> Result<String, ConfigError> {
    optional(key).ok_or(ConfigError::Missing(key))
}

impl Config {
    /// `DATABASE_URL` only; `migrate` needs nothing else.
    pub fn database_url() -> Result<String, ConfigError> {
        required("DATABASE_URL")
    }

    pub fn from_env() -> Result<Self, ConfigError> {
        let secrets_key = optional("ENZARB_SECRETS_KEY")
            .map(|k| {
                SecretsKey::from_base64(&k)
                    .map_err(|e| ConfigError::Invalid("ENZARB_SECRETS_KEY", e))
            })
            .transpose()?;
        Ok(Self {
            database_url: Self::database_url()?,
            webhook_secret: required("GITHUB_WEBHOOK_SECRET")?.into_bytes(),
            bot: BotIdentity::from_env(),
            listen: optional("RECEIVER_LISTEN").unwrap_or_else(|| "0.0.0.0:8080".into()),
            events_secret_name: optional("ENZARB_EVENTS_SECRET_NAME")
                .unwrap_or_else(|| DEFAULT_EVENTS_SECRET_NAME.into()),
            secrets_key,
        })
    }
}
