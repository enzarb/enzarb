//! GitHub App implementation of [`enzarb_connector::Connector`], plus webhook
//! signature verification and mapping to the standard event catalog.
//!
//! * Receiver (no App credentials needed): [`verify_signature`],
//!   [`catalog::map`] / [`BotIdentity::map`] and [`installation_change`].
//! * Executor: [`GitHubConnector::from_env`] and the `Connector` trait.

pub mod catalog;
mod config;
mod connector;
mod http;
mod payload;
mod resolve;
mod webhook;

pub use catalog::{MapError, Mapped};
pub use config::{BotIdentity, ConfigError, GitHubConfig, RetryPolicy};
pub use connector::GitHubConnector;
pub use payload::{InstallationChange, SOURCE, installation_change};
pub use resolve::{CommentPermissions, PERMISSION_TTL};
pub use webhook::{sign, verify_signature};
