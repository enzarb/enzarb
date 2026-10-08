//! Lookup of Project ownership and sandbox coordinates, behind a trait so tests
//! can inject a fake.

use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;

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

#[async_trait]
pub trait Resolver: Send + Sync + 'static {
    /// The Project named `name`, or `NotFound`.
    async fn project(&self, name: &str) -> Result<ProjectInfo, ResolveError>;
    /// The sandbox bearer token. Callers should `invalidate_token` when the
    /// sandbox rejects it.
    async fn token(&self, sandbox: &SandboxRef) -> Result<String, ResolveError>;
    fn invalidate_token(&self, _sandbox: &SandboxRef) {}
}

/// In-memory resolver for tests.
#[derive(Default)]
pub struct FakeResolver {
    projects: Mutex<HashMap<String, ProjectInfo>>,
    tokens: Mutex<HashMap<(String, String, String), String>>,
}

impl FakeResolver {
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
}
