//! GitHub App implementation of [`enzarb_connector::Connector`], plus webhook
//! signature verification and normalization.
//!
//! * Receiver (no App credentials needed): [`verify_signature`] and
//!   [`normalize`], or [`WebhookConfig`] if the bot identity comes from env.
//! * Executor: [`GitHubConnector::from_env`] and the `Connector` trait.
//!
//! See `docs/connectors.md` and contracts sections 5, 9 and 10 in the
//! top-level `enzarb` repo.

mod config;
mod connector;
mod http;
mod normalize;
mod webhook;

pub use config::{BotIdentity, ConfigError, GitHubConfig, RetryPolicy};
pub use connector::GitHubConnector;
pub use normalize::{
    APPROVED_LABEL, CommitStatusChange, InstallationChange, NormalizeError, Normalized, SOURCE,
    normalize,
};
pub use webhook::{sign, verify_signature};
