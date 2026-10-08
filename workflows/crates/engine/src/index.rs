//! Project lookup by repo key.
//!
//! The reducer asks a [`ProjectIndex`] which Project owns a repository. The real
//! implementation, [`KubeProjectIndex`], keeps an in-memory copy of all `Project`
//! custom resources current with a kube watcher. [`StaticProjectIndex`] is the
//! fake used by tests and local runs.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use enzarb_crds::{Project, RepoKey};
use futures::StreamExt;
use kube::runtime::WatchStreamExt;
use kube::runtime::reflector::{self, Store};
use kube::runtime::watcher::{self, Config as WatcherConfig};
use kube::{Api, Client};

use crate::machine::Triggers;

/// What the engine needs to know about the Project a repository belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectInfo {
    /// `Project.metadata.name`; stored in `work_items.project_id`.
    pub name: String,
    pub triggers: Triggers,
    /// `spec.repositories[].defaultBranch` of the matching repository.
    pub default_branch: String,
}

/// Resolves a repo key (`github:owner/repo`, matched case-insensitively) to its Project.
pub trait ProjectIndex: Send + Sync {
    fn lookup(&self, repo_key: &str) -> Option<ProjectInfo>;
}

/// Pure lookup over a set of Projects. Ties (a repository linked from several
/// Projects) go to the lexicographically smallest Project name, so the answer is stable.
pub fn lookup_in<'a>(
    projects: impl IntoIterator<Item = &'a Project>,
    repo_key: &str,
) -> Option<ProjectInfo> {
    let want = RepoKey::parse(repo_key).ok()?;
    projects
        .into_iter()
        .filter(|p| p.metadata.deletion_timestamp.is_none())
        .filter_map(|p| {
            let repo = p.spec.repositories.iter().find(|r| {
                RepoKey::from_connector_url(&r.connector, &r.url).is_ok_and(|k| k == want)
            })?;
            Some(ProjectInfo {
                name: p.metadata.name.clone()?,
                triggers: Triggers {
                    issues: p.spec.triggers.issues,
                    change_requests: p.spec.triggers.change_requests,
                    mentions: p.spec.triggers.mentions,
                },
                default_branch: repo.default_branch.clone(),
            })
        })
        .min_by(|a, b| a.name.cmp(&b.name))
}

/// `ProjectIndex` backed by a kube reflector over all `Project`s.
#[derive(Clone)]
pub struct KubeProjectIndex {
    store: Store<Project>,
}

impl KubeProjectIndex {
    /// Start watching. The returned task drives the watcher and must be kept alive
    /// (abort it to stop). Use [`KubeProjectIndex::wait_ready`] before relying on lookups.
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

    /// Resolves once the initial list has been loaded.
    pub async fn wait_ready(&self) {
        let _ = self.store.wait_until_ready().await;
    }
}

impl ProjectIndex for KubeProjectIndex {
    fn lookup(&self, repo_key: &str) -> Option<ProjectInfo> {
        let all = self.store.state();
        lookup_in(all.iter().map(|p| p.as_ref()), repo_key)
    }
}

/// Fixed, mutable index for tests and local runs: repo key (lowercase) -> info.
#[derive(Default)]
pub struct StaticProjectIndex {
    map: RwLock<HashMap<String, ProjectInfo>>,
}

impl StaticProjectIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }

    /// Map `repo_key` (any case) to a Project named `name` with all triggers on and branch `main`.
    pub fn with_repo(self, repo_key: &str, name: &str) -> Self {
        self.insert(
            repo_key,
            ProjectInfo {
                name: name.into(),
                triggers: Triggers::default(),
                default_branch: "main".into(),
            },
        );
        self
    }

    pub fn insert(&self, repo_key: &str, info: ProjectInfo) {
        self.map
            .write()
            .expect("index lock")
            .insert(repo_key.to_ascii_lowercase(), info);
    }

    pub fn remove(&self, repo_key: &str) {
        self.map
            .write()
            .expect("index lock")
            .remove(&repo_key.to_ascii_lowercase());
    }
}

impl ProjectIndex for StaticProjectIndex {
    fn lookup(&self, repo_key: &str) -> Option<ProjectInfo> {
        self.map
            .read()
            .expect("index lock")
            .get(&repo_key.to_ascii_lowercase())
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_crds::{ProjectSpec, Repository};

    fn project(name: &str, urls: &[&str], issues: bool) -> Project {
        let mut spec: ProjectSpec = serde_json::from_value(serde_json::json!({
            "owner": "ruckc",
            "triggers": {"issues": issues},
        }))
        .unwrap();
        spec.repositories = urls
            .iter()
            .map(|u| Repository {
                url: (*u).into(),
                connector: "github".into(),
                default_branch: "trunk".into(),
            })
            .collect();
        Project::new(name, spec)
    }

    #[test]
    fn matches_case_insensitively_and_carries_settings() {
        let ps = [
            project("a", &["https://github.com/Enzarb/Enzarb-UI"], false),
            project("b", &["https://github.com/enzarb/other.git"], true),
        ];
        let got = lookup_in(&ps, "github:enzarb/enzarb-ui").unwrap();
        assert_eq!(got.name, "a");
        assert_eq!(got.default_branch, "trunk");
        assert!(!got.triggers.issues && got.triggers.mentions);
        assert_eq!(lookup_in(&ps, "github:ENZARB/ENZARB-UI").unwrap().name, "a");
        assert_eq!(lookup_in(&ps, "github:enzarb/other").unwrap().name, "b");
        assert!(lookup_in(&ps, "github:enzarb/missing").is_none());
        assert!(lookup_in(&ps, "garbage").is_none());
    }

    #[test]
    fn ties_pick_smallest_name_and_deleting_projects_are_skipped() {
        let mut dying = project("a", &["https://github.com/x/y"], true);
        dying.metadata.deletion_timestamp = Some(k8s_time());
        let ps = [project("c", &["https://github.com/x/y"], true), dying];
        assert_eq!(lookup_in(&ps, "github:x/y").unwrap().name, "c");
        let ps = [
            project("z", &["https://github.com/x/y"], true),
            project("m", &["https://github.com/x/y"], true),
        ];
        assert_eq!(lookup_in(&ps, "github:x/y").unwrap().name, "m");
    }

    fn k8s_time() -> k8s_openapi::apimachinery::pkg::apis::meta::v1::Time {
        k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(
            k8s_openapi::jiff::Timestamp::UNIX_EPOCH,
        )
    }
}
