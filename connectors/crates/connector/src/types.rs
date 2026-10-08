use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{RepoKey, WorkItemKey};

/// Who did something. `id` is the provider's numeric user id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub login: String,
    pub id: u64,
    #[serde(default)]
    pub is_bot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Issue,
    ChangeRequest,
}

/// An issue or change request as it appears in an event envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemRef {
    pub kind: ItemKind,
    pub number: u64,
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub author: String,
    /// Change requests only.
    #[serde(default)]
    pub head_ref: Option<String>,
    #[serde(default)]
    pub base_ref: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentRef {
    pub id: u64,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub url: String,
}

/// A comment in a fetched [`Thread`], with its author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadComment {
    pub author: Actor,
    #[serde(flatten)]
    pub comment: CommentRef,
    pub created_at: Option<DateTime<Utc>>,
}

/// An item plus its comments, oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub item: ItemRef,
    pub comments: Vec<ThreadComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitRef {
    pub repo: RepoKey,
    pub sha: String,
}

/// What a progress marker attaches to: a work item, or one comment on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgressTarget {
    pub item: WorkItemKey,
    #[serde(default)]
    pub comment_id: Option<u64>,
}

/// Acknowledgement state shown to people (a reaction on GitHub).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Progress {
    Received,
    Working,
    Done,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Pending,
    Success,
    Failure,
    Error,
}

/// A commit status / check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub state: StatusState,
    pub context: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub target_url: Option<String>,
}

/// A user's access to a repository, lowest to highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    None,
    Read,
    Triage,
    Write,
    Maintain,
    Admin,
}

impl Permission {
    pub fn can_write(self) -> bool {
        self >= Permission::Write
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeRequestSpec {
    pub repo: RepoKey,
    pub title: String,
    pub body: String,
    pub head_ref: String,
    pub base_ref: String,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredScope {
    /// Clone / fetch only.
    Read,
    /// Clone, fetch and push.
    ReadWrite,
}

/// A short-lived, single-repo clone credential. `Debug` redacts the token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortLivedCred {
    pub token: String,
    pub repo: RepoKey,
    pub scope: CredScope,
    pub expires_at: DateTime<Utc>,
}

impl fmt::Debug for ShortLivedCred {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShortLivedCred")
            .field("token", &"<redacted>")
            .field("repo", &self.repo)
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
