use crate::permission::PermissionError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Permission(#[from] PermissionError),
    /// A stored work item that this build cannot read (unknown workflow, state or version).
    #[error("state: {0}")]
    State(String),
    #[error("job {0} not found")]
    JobNotFound(i64),
}
