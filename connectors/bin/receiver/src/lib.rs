//! Webhook receiver: `POST /hooks/github`, `POST /v1/events/<project>`, `/healthz`, `/readyz`.

pub mod config;
pub mod db;
pub mod handler;
pub mod installations;
pub mod secrets;

pub use config::{Config, ConfigError};
pub use handler::{AppState, router};
