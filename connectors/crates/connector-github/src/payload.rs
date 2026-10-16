//! GitHub REST/webhook payload shapes the connector reads, and installation webhooks.

use chrono::{DateTime, Utc};
use enzarb_connector::{Actor, CommentKind, CommentRef, ItemKind, ItemRef, RepoKey};
use serde::{Deserialize, Serialize};

use crate::catalog::MapError;

/// `RepoKey` connector name.
pub const SOURCE: &str = "github";

/// An `installation` or `installation_repositories` delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallationChange {
    /// Delivery id.
    pub id: String,
    /// `installation` or `installation_repositories`.
    pub event: String,
    /// `created`, `deleted`, `suspend`, `unsuspend`, `new_permissions_accepted`,
    /// `added`, `removed`.
    pub action: String,
    pub installation_id: u64,
    pub account_login: String,
    pub account_id: u64,
    /// `User` or `Organization`.
    pub account_type: String,
    /// `all` or `selected`.
    pub repository_selection: String,
    /// Repos now available (`installation.created`) or added.
    pub repositories_added: Vec<RepoKey>,
    /// Repos removed (`installation.deleted`: all of them).
    pub repositories_removed: Vec<RepoKey>,
    pub sender: Actor,
}

// ---- payload shapes (only the fields we read) ----

#[derive(Deserialize)]
pub(crate) struct User {
    pub(crate) login: String,
    pub(crate) id: u64,
    #[serde(default, rename = "type")]
    pub(crate) kind: Option<String>,
}

impl User {
    pub(crate) fn actor(&self) -> Actor {
        Actor {
            login: self.login.clone(),
            id: self.id,
            is_bot: self.kind.as_deref() == Some("Bot") || self.login.ends_with("[bot]"),
        }
    }
}

#[derive(Deserialize)]
struct RepoStub {
    full_name: String,
}

impl RepoStub {
    fn key(&self) -> Option<RepoKey> {
        let (o, r) = self.full_name.split_once('/')?;
        Some(RepoKey::new(SOURCE, o, r))
    }
}

#[derive(Deserialize)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
    pub(crate) user: User,
    /// Present when the "issue" is a pull request (issue_comment events).
    #[serde(default)]
    pub(crate) pull_request: Option<serde_json::Value>,
}

impl Issue {
    pub(crate) fn item(&self) -> ItemRef {
        ItemRef {
            kind: if self.pull_request.is_some() {
                ItemKind::ChangeRequest
            } else {
                ItemKind::Issue
            },
            number: self.number,
            url: self.html_url.clone(),
            title: self.title.clone(),
            body: self.body.clone().unwrap_or_default(),
            author: self.user.login.clone(),
            head_ref: None,
            base_ref: None,
            head_sha: None,
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Branch {
    #[serde(rename = "ref")]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) sha: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct PullRequest {
    pub(crate) number: u64,
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) body: Option<String>,
    pub(crate) user: User,
    pub(crate) head: Branch,
    pub(crate) base: Branch,
}

impl PullRequest {
    pub(crate) fn item(&self) -> ItemRef {
        ItemRef {
            kind: ItemKind::ChangeRequest,
            number: self.number,
            url: self.html_url.clone(),
            title: self.title.clone(),
            body: self.body.clone().unwrap_or_default(),
            author: self.user.login.clone(),
            head_ref: Some(self.head.name.clone()),
            base_ref: Some(self.base.name.clone()),
            head_sha: self.head.sha.clone(),
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Comment {
    pub(crate) id: u64,
    #[serde(default)]
    pub(crate) body: String,
    #[serde(default)]
    pub(crate) html_url: String,
    #[serde(default)]
    pub(crate) created_at: Option<DateTime<Utc>>,
}

impl Comment {
    pub(crate) fn comment_ref(&self) -> CommentRef {
        self.comment_ref_kind(CommentKind::Conversation)
    }

    pub(crate) fn comment_ref_kind(&self, kind: CommentKind) -> CommentRef {
        CommentRef {
            id: self.id,
            kind,
            body: self.body.clone(),
            url: self.html_url.clone(),
        }
    }
}

#[derive(Deserialize)]
struct Account {
    login: String,
    id: u64,
    #[serde(default, rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct InstallationPayload {
    id: u64,
    /// Absent on the minimal `installation` stub that rides on other events.
    #[serde(default)]
    account: Option<Account>,
    #[serde(default)]
    repository_selection: String,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    action: String,
    #[serde(default)]
    sender: Option<User>,
    #[serde(default)]
    installation: Option<InstallationPayload>,
    #[serde(default)]
    repositories: Vec<RepoStub>,
    #[serde(default)]
    repositories_added: Vec<RepoStub>,
    #[serde(default)]
    repositories_removed: Vec<RepoStub>,
}

fn missing(event: &str, field: &'static str) -> MapError {
    MapError::Missing {
        event: event.to_string(),
        field,
    }
}

/// An `installation` / `installation_repositories` delivery as an [`InstallationChange`];
/// `None` for any other event.
pub fn installation_change(
    event_name: &str,
    delivery_id: &str,
    body: &[u8],
) -> Result<Option<InstallationChange>, MapError> {
    if !matches!(event_name, "installation" | "installation_repositories") {
        return Ok(None);
    }
    let p: Payload = serde_json::from_slice(body).map_err(|source| MapError::Malformed {
        event: event_name.to_string(),
        source,
    })?;
    installation(event_name, delivery_id, p).map(Some)
}

fn installation(
    event_name: &str,
    delivery_id: &str,
    p: Payload,
) -> Result<InstallationChange, MapError> {
    let inst = p
        .installation
        .ok_or_else(|| missing(event_name, "installation"))?;
    let account = inst
        .account
        .ok_or_else(|| missing(event_name, "installation.account"))?;
    let sender = p.sender.ok_or_else(|| missing(event_name, "sender"))?;
    let keys = |v: &[RepoStub]| v.iter().filter_map(RepoStub::key).collect::<Vec<_>>();
    let (added, removed) = match (event_name, p.action.as_str()) {
        ("installation", "created") => (keys(&p.repositories), vec![]),
        ("installation", "deleted") => (vec![], keys(&p.repositories)),
        _ => (keys(&p.repositories_added), keys(&p.repositories_removed)),
    };
    Ok(InstallationChange {
        id: delivery_id.to_string(),
        event: event_name.to_string(),
        action: p.action,
        installation_id: inst.id,
        account_login: account.login,
        account_id: account.id,
        account_type: account.kind,
        repository_selection: inst.repository_selection,
        repositories_added: added,
        repositories_removed: removed,
        sender: sender.actor(),
    })
}
