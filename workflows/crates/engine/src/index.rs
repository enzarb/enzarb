//! Project lookup by repo key.
//!
//! [`KubeProjectIndex`] keeps an in-memory copy of all `Project` custom resources current with a
//! kube watcher. [`StaticProjectIndex`] is the fake used by tests and local runs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use enzarb_crds::project::cmp_precedence;
use enzarb_crds::{Project, RepoKey};
use futures::StreamExt;
use kube::runtime::WatchStreamExt;
use kube::runtime::reflector::{self, Store};
use kube::runtime::watcher::{self, Config as WatcherConfig};
use kube::{Api, Client};

/// What the engine needs to know about a Project.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProjectInfo {
    /// `Project.metadata.name`.
    pub name: String,
    /// `spec.owner`.
    pub owner: String,
    /// `owner/repo` (lowercase) -> default branch, for every repository of the Project.
    pub repos: Vec<(String, String)>,
    /// `spec.workflows.library` (`owner/repo@ref`).
    pub library: Option<String>,
    /// `spec.workflows.bindings` (a bindings file as YAML text).
    pub bindings: Option<String>,
}

impl ProjectInfo {
    pub fn default_branch(&self, repo: &str) -> Option<&str> {
        self.repos
            .iter()
            .find(|(r, _)| r.eq_ignore_ascii_case(repo))
            .map(|(_, b)| b.as_str())
    }
}

/// Resolves repositories and names to Projects.
pub trait ProjectIndex: Send + Sync {
    /// The Project owning `repo` (`owner/repo`, any case).
    fn lookup(&self, repo: &str) -> Option<ProjectInfo>;
    fn get(&self, project: &str) -> Option<ProjectInfo>;
    fn all(&self) -> Vec<ProjectInfo>;
}

fn info(p: &Project) -> Option<ProjectInfo> {
    let repos = p
        .spec
        .repositories
        .iter()
        .filter_map(|r| {
            let k = RepoKey::from_connector_url(&r.connector, &r.url).ok()?;
            let s = k.as_str();
            let repo = s.split_once(':').map(|(_, r)| r).unwrap_or(s);
            Some((repo.to_ascii_lowercase(), r.default_branch.clone()))
        })
        .collect();
    let w = p.spec.workflows.clone().unwrap_or_default();
    Some(ProjectInfo {
        name: p.metadata.name.clone()?,
        owner: p.spec.owner.clone(),
        repos,
        library: w.library.filter(|s| !s.is_empty()),
        bindings: w.bindings.filter(|s| !s.trim().is_empty()),
    })
}

/// Pure lookup: when a repository is linked from several Projects the oldest wins by
/// `(creationTimestamp, name)` (the operator's [`cmp_precedence`]).
pub fn lookup_in<'a>(
    projects: impl IntoIterator<Item = &'a Project>,
    repo: &str,
) -> Option<ProjectInfo> {
    projects
        .into_iter()
        .filter(|p| p.metadata.deletion_timestamp.is_none())
        .filter_map(|p| {
            info(p)
                .filter(|i| i.default_branch(repo).is_some())
                .map(|i| (p, i))
        })
        .min_by(|(a, _), (b, _)| cmp_precedence(a, b))
        .map(|(_, i)| i)
}

#[derive(Clone)]
pub struct KubeProjectIndex {
    store: Store<Project>,
}

impl KubeProjectIndex {
    /// Start watching. Keep the returned task alive.
    pub fn start(client: Client) -> (Self, tokio::task::JoinHandle<()>) {
        let api: Api<Project> = Api::all(client);
        let (store, writer) = reflector::store();
        let stream = reflector::reflector(writer, watcher::watcher(api, WatcherConfig::default()))
            .default_backoff()
            .applied_objects();
        let task = tokio::spawn(async move {
            let mut stream = std::pin::pin!(stream);
            while let Some(item) = stream.next().await {
                if let Err(e) = item {
                    tracing::warn!(error = %e, "project watcher error");
                }
            }
        });
        (Self { store }, task)
    }

    pub async fn wait_ready(&self) {
        let _ = self.store.wait_until_ready().await;
    }
}

impl ProjectIndex for KubeProjectIndex {
    fn lookup(&self, repo: &str) -> Option<ProjectInfo> {
        let all = self.store.state();
        lookup_in(all.iter().map(|p| p.as_ref()), repo)
    }

    fn get(&self, project: &str) -> Option<ProjectInfo> {
        self.store
            .state()
            .iter()
            .find(|p| p.metadata.name.as_deref() == Some(project))
            .and_then(|p| info(p))
    }

    fn all(&self) -> Vec<ProjectInfo> {
        self.store
            .state()
            .iter()
            .filter(|p| p.metadata.deletion_timestamp.is_none())
            .filter_map(|p| info(p))
            .collect()
    }
}

/// Fixed, mutable index for tests and local runs.
#[derive(Default)]
pub struct StaticProjectIndex {
    projects: RwLock<HashMap<String, ProjectInfo>>,
}

impl StaticProjectIndex {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn insert(&self, info: ProjectInfo) {
        self.projects
            .write()
            .expect("index lock")
            .insert(info.name.clone(), info);
    }
}

impl ProjectIndex for StaticProjectIndex {
    fn lookup(&self, repo: &str) -> Option<ProjectInfo> {
        let m = self.projects.read().expect("index lock");
        let mut v: Vec<_> = m
            .values()
            .filter(|p| p.default_branch(repo).is_some())
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v.first().map(|p| (*p).clone())
    }

    fn get(&self, project: &str) -> Option<ProjectInfo> {
        self.projects
            .read()
            .expect("index lock")
            .get(project)
            .cloned()
    }

    fn all(&self) -> Vec<ProjectInfo> {
        let mut v: Vec<_> = self
            .projects
            .read()
            .expect("index lock")
            .values()
            .cloned()
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_crds::ProjectSpec;

    #[test]
    fn lookup_is_case_insensitive_and_carries_workflow_settings() {
        let spec: ProjectSpec = serde_json::from_value(serde_json::json!({
            "owner": "ruckc",
            "repositories": [{"url": "https://github.com/Enzarb/Enzarb", "connector": "github", "defaultBranch": "trunk"}],
            "workflows": {"library": "enzarb/workflows@v2"},
        }))
        .unwrap();
        let ps = [Project::new("a", spec)];
        let got = lookup_in(&ps, "ENZARB/enzarb").unwrap();
        assert_eq!(got.name, "a");
        assert_eq!(got.default_branch("enzarb/enzarb"), Some("trunk"));
        assert_eq!(got.library.as_deref(), Some("enzarb/workflows@v2"));
        assert!(lookup_in(&ps, "enzarb/other").is_none());
    }
}
