//! Lookup of Project ownership and sandbox coordinates, behind a trait so tests
//! can inject a fake.

use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;
use tokio::sync::broadcast;

/// Where a Project's sandbox lives and where its bearer token is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxRef {
    /// `ws://...` mux endpoint (`status.sandbox.endpoint`).
    pub endpoint: String,
    pub secret_namespace: String,
    pub secret_name: String,
    pub secret_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    /// `spec.owner`.
    pub owner: String,
    /// `None` until the operator has published `status.sandbox`.
    pub sandbox: Option<SandboxRef>,
}

#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("not found")]
    NotFound,
    #[error("lookup failed: {0}")]
    Unavailable(String),
}

/// Capacity of the change-notification channels. A lagging receiver only causes
/// an extra re-check, never a missed close.
pub const CHANGE_CAPACITY: usize = 256;

/// Whether a relay authorised for `owner` must be closed given the Project's
/// current state: the owner changed, or the sandbox status is gone (ADR-0013).
pub fn revoked(info: &ProjectInfo, owner: &str) -> bool {
    info.owner != owner || info.sandbox.is_none()
}

#[async_trait]
pub trait Resolver: Send + Sync + 'static {
    /// The Project named `name`, or `NotFound`.
    async fn project(&self, name: &str) -> Result<ProjectInfo, ResolveError>;
    /// The sandbox bearer token. Callers should `invalidate_token` when the
    /// sandbox rejects it.
    async fn token(&self, sandbox: &SandboxRef) -> Result<String, ResolveError>;
    fn invalidate_token(&self, _sandbox: &SandboxRef) {}
    /// Names of Projects that changed or were deleted. The default never fires.
    fn subscribe(&self) -> broadcast::Receiver<String> {
        broadcast::channel(1).1
    }
}

/// In-memory resolver for tests.
pub struct FakeResolver {
    changes: broadcast::Sender<String>,
    projects: Mutex<HashMap<String, ProjectInfo>>,
    tokens: Mutex<HashMap<(String, String, String), String>>,
}

impl Default for FakeResolver {
    fn default() -> Self {
        Self {
            changes: broadcast::channel(CHANGE_CAPACITY).0,
            projects: Default::default(),
            tokens: Default::default(),
        }
    }
}

impl FakeResolver {
    /// Replace a Project's info and notify subscribers (same as `insert`).
    pub fn update(&self, name: &str, info: ProjectInfo, token: &str) {
        self.insert(name, info, token);
    }

    /// Delete a Project and notify subscribers.
    pub fn remove(&self, name: &str) {
        self.projects.lock().unwrap().remove(name);
        let _ = self.changes.send(name.to_string());
    }

    pub fn insert(&self, name: &str, info: ProjectInfo, token: &str) {
        if let Some(s) = &info.sandbox {
            self.tokens.lock().unwrap().insert(
                (
                    s.secret_namespace.clone(),
                    s.secret_name.clone(),
                    s.secret_key.clone(),
                ),
                token.to_string(),
            );
        }
        self.projects.lock().unwrap().insert(name.to_string(), info);
        let _ = self.changes.send(name.to_string());
    }
}

#[async_trait]
impl Resolver for FakeResolver {
    async fn project(&self, name: &str) -> Result<ProjectInfo, ResolveError> {
        self.projects
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or(ResolveError::NotFound)
    }

    async fn token(&self, s: &SandboxRef) -> Result<String, ResolveError> {
        self.tokens
            .lock()
            .unwrap()
            .get(&(
                s.secret_namespace.clone(),
                s.secret_name.clone(),
                s.secret_key.clone(),
            ))
            .cloned()
            .ok_or(ResolveError::NotFound)
    }

    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.changes.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sb() -> SandboxRef {
        SandboxRef {
            endpoint: "ws://x/v1/mux".into(),
            secret_namespace: "ns".into(),
            secret_name: "n".into(),
            secret_key: "k".into(),
        }
    }

    #[test]
    fn revoked_truth_table() {
        let ready = |o: &str| ProjectInfo {
            owner: o.into(),
            sandbox: Some(sb()),
        };
        assert!(!revoked(&ready("a"), "a"));
        assert!(revoked(&ready("b"), "a"));
        let not_ready = ProjectInfo {
            owner: "a".into(),
            sandbox: None,
        };
        assert!(revoked(&not_ready, "a"));
        assert!(revoked(&not_ready, "b"));
        // Endpoint/secret changes are not revocations.
        let mut moved = ready("a");
        moved.sandbox.as_mut().unwrap().endpoint = "ws://y/v1/mux".into();
        assert!(!revoked(&moved, "a"));
    }
}
