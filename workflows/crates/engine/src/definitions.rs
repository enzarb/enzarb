//! Reading graph workflow definitions (ADR-0020) from a repository's default branch.
//!
//! The reducer calls a [`DefinitionSource`] before the event's transaction, like the
//! permission lookup, and hands the result to the transition in `Ctx::graph`. Production
//! wires [`ConnectorDefinitions`]: `Connector::branch_head` of the default branch, then
//! `Connector::get_file` of `.enzarb/workflows/<name>.yaml` at that sha. Only the default
//! branch is read, so a definition changes only through a merged change request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use enzarb_connector::{Connector, Error, RepoKey};

use crate::machine::graph::{GraphSource, definition_path};

#[async_trait]
pub trait DefinitionSource: Send + Sync {
    /// The definition `name` on `branch` of `repo`, or `None` when there is none (or the
    /// connector cannot read files).
    async fn fetch(
        &self,
        repo: &RepoKey,
        branch: &str,
        name: &str,
    ) -> Result<Option<GraphSource>, Error>;
}

/// How long "no definition at this commit" is remembered.
pub const MISS_TTL: Duration = Duration::from_secs(5 * 60);

/// Routes to the `Connector` named by the repo key. Remembers, per repository and head sha,
/// that there is no definition, so most issues cost one `branch_head` call.
#[derive(Default)]
pub struct ConnectorDefinitions {
    connectors: HashMap<String, Arc<dyn Connector>>,
    misses: Mutex<HashMap<(String, String), Instant>>,
}

impl ConnectorDefinitions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, connector_name: &str, c: Arc<dyn Connector>) -> Self {
        self.connectors.insert(connector_name.to_string(), c);
        self
    }
}

#[async_trait]
impl DefinitionSource for ConnectorDefinitions {
    async fn fetch(
        &self,
        repo: &RepoKey,
        branch: &str,
        name: &str,
    ) -> Result<Option<GraphSource>, Error> {
        let Some(c) = self.connectors.get(&repo.connector) else {
            return Ok(None);
        };
        // Pin the commit: the definition is read, and recorded, at the branch head.
        let sha = c
            .branch_head(repo, branch)
            .await?
            .unwrap_or_else(|| branch.to_string());
        let path = definition_path(name);
        let miss_key = (format!("{repo}:{path}"), sha.clone());
        {
            let mut misses = self.misses.lock().unwrap_or_else(|e| e.into_inner());
            misses.retain(|_, at| at.elapsed() < MISS_TTL);
            if misses.contains_key(&miss_key) {
                return Ok(None);
            }
        }
        let remember = || {
            self.misses
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(miss_key.clone(), Instant::now());
        };
        match c.get_file(repo, &sha, &path).await {
            Ok(Some(source)) => Ok(Some(GraphSource {
                name: name.to_string(),
                path,
                sha,
                source,
            })),
            Ok(None) => {
                remember();
                Ok(None)
            }
            // The connector cannot read files: no graph workflows there.
            Err(Error::Invalid(_)) => {
                remember();
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_connector::fake::FakeConnector;

    #[tokio::test]
    async fn reads_at_the_branch_head() {
        let f = Arc::new(FakeConnector::new());
        let repo = RepoKey::new("github", "o", "r");
        let defs = ConnectorDefinitions::new().with("github", f.clone());
        assert_eq!(defs.fetch(&repo, "main", "issue").await.unwrap(), None);
        // A miss is remembered per head sha.
        f.set_file(repo.clone(), "main", ".enzarb/workflows/issue.yaml", "y");
        assert_eq!(defs.fetch(&repo, "main", "issue").await.unwrap(), None);
        f.set_branch_head(repo.clone(), "main", "abc");
        f.set_file(repo.clone(), "abc", ".enzarb/workflows/issue.yaml", "x");
        let g = defs.fetch(&repo, "main", "issue").await.unwrap().unwrap();
        assert_eq!((g.sha.as_str(), g.source.as_str()), ("abc", "x"));
        assert_eq!(g.path, ".enzarb/workflows/issue.yaml");
        let other = RepoKey::new("gitlab", "o", "r");
        assert_eq!(defs.fetch(&other, "main", "issue").await.unwrap(), None);
    }
}
