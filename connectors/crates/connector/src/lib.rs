//! Provider-neutral connector interface and normalized event types.
//!
//! See `docs/connectors.md` and `docs/contracts.md` section 5 in the top-level
//! `enzarb` repo. The inbound side is [`Event`] (a CloudEvents envelope); the
//! outbound side is the [`Connector`] trait.

pub mod command;
mod connector;
mod error;
mod event;
#[cfg(feature = "fake")]
pub mod fake;
mod keys;
mod types;

pub use connector::Connector;
pub use error::{Error, Result};
pub use event::{Event, EventData, EventType, Review};
pub use keys::{KeyError, RepoKey, WorkItemKey};
pub use types::{
    Actor, ChangeRequestSpec, CommentRef, CommitRef, CredScope, ItemKind, ItemRef, Permission,
    Progress, ProgressTarget, ShortLivedCred, Status, StatusState, Thread, ThreadComment,
};
