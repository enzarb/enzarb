use crate::permission::PermissionError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Permission(#[from] PermissionError),
    /// A stored task this build cannot read, or an unreadable engine event.
    #[error("state: {0}")]
    State(String),
}
