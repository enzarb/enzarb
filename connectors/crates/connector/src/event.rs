use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Actor, CommentRef, ItemRef, RepoKey, WorkItemKey};

pub const SPECVERSION: &str = "1.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    WorkItemOpened,
    CommentAdded,
    Mentioned,
    ApprovalGranted,
    ChangeRequestOpened,
    ChangeRequestUpdated,
    ReviewSubmitted,
    WorkItemClosed,
}

impl EventType {
    pub const ALL: [EventType; 8] = [
        EventType::WorkItemOpened,
        EventType::CommentAdded,
        EventType::Mentioned,
        EventType::ApprovalGranted,
        EventType::ChangeRequestOpened,
        EventType::ChangeRequestUpdated,
        EventType::ReviewSubmitted,
        EventType::WorkItemClosed,
    ];

    /// The wire name, as in the envelope's `type` attribute.
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::WorkItemOpened => "WorkItemOpened",
            EventType::CommentAdded => "CommentAdded",
            EventType::Mentioned => "Mentioned",
            EventType::ApprovalGranted => "ApprovalGranted",
            EventType::ChangeRequestOpened => "ChangeRequestOpened",
            EventType::ChangeRequestUpdated => "ChangeRequestUpdated",
            EventType::ReviewSubmitted => "ReviewSubmitted",
            EventType::WorkItemClosed => "WorkItemClosed",
        }
    }
}

/// `data.review` of a `ReviewSubmitted` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub id: u64,
    pub state: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventData {
    pub repo: RepoKey,
    pub actor: Actor,
    pub item: ItemRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<CommentRef>,
    /// `WorkItemClosed` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
    /// `ReviewSubmitted` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Review>,
}

/// CloudEvents-style envelope; this is `workflows.events.payload`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub specversion: String,
    pub source: String,
    /// Provider delivery id; used to build the dedupe key.
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub subject: WorkItemKey,
    pub time: DateTime<Utc>,
    pub data: EventData,
}

impl Event {
    pub fn new(
        source: impl Into<String>,
        id: impl Into<String>,
        event_type: EventType,
        time: DateTime<Utc>,
        data: EventData,
    ) -> Self {
        let subject = WorkItemKey::new(data.repo.clone(), data.item.number);
        Self {
            specversion: SPECVERSION.to_string(),
            source: source.into(),
            id: id.into(),
            event_type,
            subject,
            time,
            data,
        }
    }

    /// Dedupe key for `workflows_api.ingest_event`: `<source>:<delivery id>`.
    pub fn dedupe_key(&self) -> String {
        format!("{}:{}", self.source, self.id)
    }
}
