//! Where a Project's sandbox-init lives and how to authenticate to it.

use std::collections::HashMap;
use std::sync::RwLock;

use super::sandbox_change::{ProjectChangeState, change_state};
use async_trait::async_trait;
use enzarb_connector::RepoKey;
use enzarb_crds::Project;
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};

/// Mux endpoint and bearer token of one sandbox (`contracts.md` section 3).
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxTarget {
    /// `ws://<pod>:7070/v1/mux`.
    pub endpoint: String,
    pub token: String,
}

impl std::fmt::Debug for SandboxTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxTarget")
            .field("endpoint", &self.endpoint)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The Project does not exist, or its sandbox is not provisioned yet. Retryable.
    #[error("sandbox not ready: {0}")]
    NotReady(String),
    #[error("kubernetes: {0}")]
    Kube(String),
}

#[async_trait]
pub trait SandboxResolver: Send + Sync {
    /// `project` is `Project.metadata.name` (`jobs.project_id`).
    async fn resolve(&self, project: &str) -> Result<SandboxTarget, SandboxError>;

    /// The repositories of the Project (`Project.spec.repositories`, never job input), for the
    /// read-only dependency credential. The default is empty: no extra repos.
    async fn project_repos(&self, _project: &str) -> Result<Vec<RepoKey>, SandboxError> {
        Ok(Vec::new())
    }

    /// `Project.spec.maxRuns`, `None` when unset or unknown (the caller applies the default).
    async fn project_max_runs(&self, _project: &str) -> Result<Option<i64>, SandboxError> {
        Ok(None)
    }

    /// `Project.spec.owner` (the GitHub login whose Claude token runs use). `None` when the
    /// Project or its owner is unknown; the caller fails closed, never picks another owner.
    async fn project_owner(&self, _project: &str) -> Result<Option<String>, SandboxError> {
        Ok(None)
    }

    /// Pending-change state of every Project. The default lists none (no holds).
    async fn change_states(&self) -> Result<Vec<ProjectChangeState>, SandboxError> {
        Ok(Vec::new())
    }

    /// Set (`Some`) or remove (`None`) the `enzarb.dev/drained-generation` annotation.
    async fn set_drained(
        &self,
        _project: &str,
        _generation: Option<i64>,
    ) -> Result<(), SandboxError> {
        Ok(())
    }
}

/// `spec.owner` of a Project, exactly as written; blank is `None`. Pure, for tests.
pub fn project_owner(p: &Project) -> Option<String> {
    (!p.spec.owner.trim().is_empty()).then(|| p.spec.owner.clone())
}

/// `spec.maxRuns` of a Project. Pure, for tests.
pub fn max_runs(p: &Project) -> Option<i64> {
    p.spec.max_runs.map(i64::from)
}

/// `RepoKey`s of a Project's GitHub repositories, from `spec.repositories`. Pure, for tests.
/// Entries that are not a canonical `https://github.com/<owner>/<repo>` URL, or whose connector
/// is not `github`, are skipped. Sorted and deduplicated.
pub fn project_repos(p: &Project) -> Vec<RepoKey> {
    let mut out: Vec<RepoKey> = p
        .spec
        .repositories
        .iter()
        .filter(|r| r.connector == "github")
        .filter_map(|r| {
            let rest = r.url.strip_prefix("https://github.com/")?;
            let rest = rest.trim_end_matches('/');
            let rest = rest.strip_suffix(".git").unwrap_or(rest);
            let (owner, repo) = rest.split_once('/')?;
            format!("github:{owner}/{repo}").parse().ok()
        })
        .collect();
    out.sort_by_key(|k| k.full_name());
    out.dedup();
    out
}

/// Reads `Project.status.sandbox` and the auth Secret from the cluster.
///
/// Needs `get` on `projects.enzarb.dev` (cluster-wide) and `get` on `secrets` in the shared
/// sandbox namespace (`Project.status.namespace`, ADR-0013). Sandboxes are found only through
/// the Project status; a Project without `status.namespace`/`status.sandbox` is not ready.
#[derive(Clone)]
pub struct KubeSandboxResolver {
    client: Client,
}

impl KubeSandboxResolver {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

/// Endpoint plus the Secret to read, from a Project's status. Pure, for tests.
pub fn secret_location(p: &Project) -> Result<(String, String, String, String), SandboxError> {
    let name = p.metadata.name.clone().unwrap_or_default();
    let status = p
        .status
        .as_ref()
        .ok_or_else(|| SandboxError::NotReady(format!("project {name} has no status yet")))?;
    let sb = status
        .sandbox
        .as_ref()
        .ok_or_else(|| SandboxError::NotReady(format!("project {name} has no sandbox yet")))?;
    // ADR-0013: no derived fallback. The operator sets `namespace` and `sandbox` only after it
    // verified ownership of the objects, so a Project without them is not ready.
    let ns = status
        .namespace
        .clone()
        .ok_or_else(|| SandboxError::NotReady(format!("project {name} has no namespace yet")))?;
    Ok((
        sb.endpoint.clone(),
        ns,
        sb.auth_secret_ref.name.clone(),
        sb.auth_secret_ref.key.clone(),
    ))
}

#[async_trait]
impl SandboxResolver for KubeSandboxResolver {
    async fn resolve(&self, project: &str) -> Result<SandboxTarget, SandboxError> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let p = projects
            .get_opt(project)
            .await
            .map_err(|e| SandboxError::Kube(e.to_string()))?
            .ok_or_else(|| SandboxError::NotReady(format!("project {project} not found")))?;
        let (endpoint, ns, secret, key) = secret_location(&p)?;
        let secrets: Api<Secret> = Api::namespaced(self.client.clone(), &ns);
        let s = secrets
            .get_opt(&secret)
            .await
            .map_err(|e| SandboxError::Kube(e.to_string()))?
            .ok_or_else(|| SandboxError::NotReady(format!("secret {ns}/{secret} not found")))?;
        let bytes =
            s.data.as_ref().and_then(|d| d.get(&key)).ok_or_else(|| {
                SandboxError::NotReady(format!("secret {ns}/{secret} has no {key}"))
            })?;
        let token = String::from_utf8(bytes.0.clone())
            .map_err(|_| SandboxError::Kube(format!("secret {ns}/{secret} {key} is not utf-8")))?;
        Ok(SandboxTarget {
            endpoint,
            token: token.trim().to_string(),
        })
    }

    async fn project_repos(&self, project: &str) -> Result<Vec<RepoKey>, SandboxError> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let p = projects
            .get_opt(project)
            .await
            .map_err(|e| SandboxError::Kube(e.to_string()))?
            .ok_or_else(|| SandboxError::NotReady(format!("project {project} not found")))?;
        Ok(project_repos(&p))
    }

    async fn project_max_runs(&self, project: &str) -> Result<Option<i64>, SandboxError> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let p = projects
            .get_opt(project)
            .await
            .map_err(|e| SandboxError::Kube(e.to_string()))?;
        Ok(p.as_ref().and_then(max_runs))
    }

    async fn project_owner(&self, project: &str) -> Result<Option<String>, SandboxError> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let p = projects
            .get_opt(project)
            .await
            .map_err(|e| SandboxError::Kube(e.to_string()))?;
        Ok(p.as_ref().and_then(project_owner))
    }

    async fn change_states(&self) -> Result<Vec<ProjectChangeState>, SandboxError> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let list = projects
            .list(&Default::default())
            .await
            .map_err(|e| SandboxError::Kube(e.to_string()))?;
        Ok(list.items.iter().map(change_state).collect())
    }

    async fn set_drained(
        &self,
        project: &str,
        generation: Option<i64>,
    ) -> Result<(), SandboxError> {
        let api: Api<Project> = Api::all(self.client.clone());
        let value = generation.map(|g| g.to_string());
        let patch = serde_json::json!({"metadata": {"annotations": {
            enzarb_crds::ANNOTATION_DRAINED_GENERATION: value
        }}});
        api.patch(
            project,
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&patch),
        )
        .await
        .map_err(|e| SandboxError::Kube(e.to_string()))?;
        Ok(())
    }
}

/// Fixed map for tests and local runs.
#[derive(Default)]
pub struct StaticSandboxResolver {
    map: RwLock<HashMap<String, SandboxTarget>>,
    repos: RwLock<HashMap<String, Vec<RepoKey>>>,
    max_runs: RwLock<HashMap<String, i64>>,
    owners: RwLock<HashMap<String, String>>,
    changes: RwLock<Vec<ProjectChangeState>>,
    drained_calls: RwLock<Vec<(String, Option<i64>)>>,
}

impl StaticSandboxResolver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, project: &str, target: SandboxTarget) {
        self.map
            .write()
            .expect("resolver lock")
            .insert(project.into(), target);
    }

    pub fn set_repos(&self, project: &str, repos: Vec<RepoKey>) {
        self.repos
            .write()
            .expect("resolver lock")
            .insert(project.into(), repos);
    }

    pub fn set_max_runs(&self, project: &str, n: i64) {
        self.max_runs
            .write()
            .expect("resolver lock")
            .insert(project.into(), n);
    }

    pub fn set_owner(&self, project: &str, owner: &str) {
        self.owners
            .write()
            .expect("resolver lock")
            .insert(project.into(), owner.into());
    }

    /// Replace the Projects `change_states` reports.
    pub fn set_change_states(&self, states: Vec<ProjectChangeState>) {
        *self.changes.write().expect("resolver lock") = states;
    }

    /// `set_drained` calls so far, in order.
    pub fn drained_calls(&self) -> Vec<(String, Option<i64>)> {
        self.drained_calls.read().expect("resolver lock").clone()
    }

    pub fn remove(&self, project: &str) {
        self.map.write().expect("resolver lock").remove(project);
    }
}

#[async_trait]
impl SandboxResolver for StaticSandboxResolver {
    async fn resolve(&self, project: &str) -> Result<SandboxTarget, SandboxError> {
        self.map
            .read()
            .expect("resolver lock")
            .get(project)
            .cloned()
            .ok_or_else(|| SandboxError::NotReady(format!("no sandbox for {project}")))
    }

    async fn project_repos(&self, project: &str) -> Result<Vec<RepoKey>, SandboxError> {
        Ok(self
            .repos
            .read()
            .expect("resolver lock")
            .get(project)
            .cloned()
            .unwrap_or_default())
    }

    async fn project_owner(&self, project: &str) -> Result<Option<String>, SandboxError> {
        Ok(self
            .owners
            .read()
            .expect("resolver lock")
            .get(project)
            .cloned())
    }

    async fn change_states(&self) -> Result<Vec<ProjectChangeState>, SandboxError> {
        Ok(self.changes.read().expect("resolver lock").clone())
    }

    async fn set_drained(
        &self,
        project: &str,
        generation: Option<i64>,
    ) -> Result<(), SandboxError> {
        self.drained_calls
            .write()
            .expect("resolver lock")
            .push((project.into(), generation));
        let mut changes = self.changes.write().expect("resolver lock");
        if let Some(c) = changes.iter_mut().find(|c| c.name == project) {
            c.drained = generation.map(|g| g.to_string());
        }
        Ok(())
    }

    async fn project_max_runs(&self, project: &str) -> Result<Option<i64>, SandboxError> {
        Ok(self
            .max_runs
            .read()
            .expect("resolver lock")
            .get(project)
            .copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_crds::{ProjectSpec, ProjectStatus, SandboxStatus, SecretKeyRef};

    #[test]
    fn max_runs_is_read_from_the_spec() {
        let spec: ProjectSpec =
            serde_json::from_value(serde_json::json!({"owner": "ruckc", "maxRuns": 5})).unwrap();
        assert_eq!(max_runs(&Project::new("p", spec)), Some(5));
        assert_eq!(max_runs(&project(None)), None);
    }

    #[test]
    fn owner_is_read_from_the_spec_and_blank_is_none() {
        let spec: ProjectSpec =
            serde_json::from_value(serde_json::json!({"owner": "ruckc"})).unwrap();
        assert_eq!(
            project_owner(&Project::new("p", spec)).as_deref(),
            Some("ruckc")
        );
        let spec: ProjectSpec = serde_json::from_value(serde_json::json!({"owner": " "})).unwrap();
        assert_eq!(project_owner(&Project::new("p", spec)), None);
    }

    fn project(status: Option<ProjectStatus>) -> Project {
        let spec: ProjectSpec =
            serde_json::from_value(serde_json::json!({"owner": "ruckc"})).unwrap();
        let mut p = Project::new("enzarb", spec);
        p.status = status;
        p
    }

    #[test]
    fn project_repos_from_spec() {
        let spec: ProjectSpec = serde_json::from_value(serde_json::json!({
            "owner": "ruckc",
            "repositories": [
                {"url": "https://github.com/enzarb/enzarb-ui", "connector": "github", "defaultBranch": "main"},
                {"url": "https://github.com/enzarb/enzarb-proto.git", "connector": "github", "defaultBranch": "main"},
                {"url": "https://github.com/enzarb/enzarb-ui/", "connector": "github", "defaultBranch": "main"},
                {"url": "https://gitlab.com/a/b", "connector": "github", "defaultBranch": "main"},
                {"url": "https://github.com/a/b", "connector": "gitlab", "defaultBranch": "main"},
                {"url": "https://github.com/a/b/c", "connector": "github", "defaultBranch": "main"}
            ]
        }))
        .unwrap();
        let got: Vec<String> = project_repos(&Project::new("p", spec))
            .iter()
            .map(|k| k.full_name())
            .collect();
        assert_eq!(got, ["enzarb/enzarb-proto", "enzarb/enzarb-ui"]);
    }

    #[test]
    fn locates_secret_from_status() {
        let p = project(Some(ProjectStatus {
            namespace: Some("enzarb-sandboxes".into()),
            sandbox: Some(SandboxStatus {
                name: "sb-enzarb".into(),
                endpoint: "ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux".into(),
                auth_secret_ref: SecretKeyRef {
                    name: "sb-enzarb-auth".into(),
                    key: "token".into(),
                },
            }),
            ..Default::default()
        }));
        let (ep, ns, name, key) = secret_location(&p).unwrap();
        assert_eq!(ep, "ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux");
        assert_eq!(
            (ns.as_str(), name.as_str(), key.as_str()),
            ("enzarb-sandboxes", "sb-enzarb-auth", "token")
        );
    }

    #[test]
    fn missing_sandbox_is_not_ready() {
        assert!(matches!(
            secret_location(&project(None)),
            Err(SandboxError::NotReady(_))
        ));
        let p = project(Some(ProjectStatus::default()));
        assert!(matches!(
            secret_location(&p),
            Err(SandboxError::NotReady(_))
        ));
    }

    #[test]
    fn missing_namespace_is_not_ready() {
        let mut p = project(Some(ProjectStatus {
            namespace: Some("enzarb-sandboxes".into()),
            sandbox: Some(SandboxStatus {
                name: "sb-enzarb".into(),
                endpoint: "ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux".into(),
                auth_secret_ref: SecretKeyRef {
                    name: "sb-enzarb-auth".into(),
                    key: "token".into(),
                },
            }),
            ..Default::default()
        }));
        p.status.as_mut().unwrap().namespace = None;
        // No derived `enzarb-p-*` / `enzarb-sandboxes` fallback.
        assert!(matches!(
            secret_location(&p),
            Err(SandboxError::NotReady(_))
        ));
    }
}
