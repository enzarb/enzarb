//! Webhook receiver: `POST /hooks/github`, `/healthz`, `/readyz`.

pub mod config;
pub mod db;
pub mod handler;
pub mod signature;

pub use config::{Config, ConfigError};
pub use handler::{AppState, router};
