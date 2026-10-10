//! Where a Project's sandbox-init lives and how to authenticate to it.

use std::collections::HashMap;
use std::sync::RwLock;

use async_trait::async_trait;
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
}

/// Fixed map for tests and local runs.
#[derive(Default)]
pub struct StaticSandboxResolver {
    map: RwLock<HashMap<String, SandboxTarget>>,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_crds::{ProjectSpec, ProjectStatus, SandboxStatus, SecretKeyRef};

    fn project(status: Option<ProjectStatus>) -> Project {
        let spec: ProjectSpec =
            serde_json::from_value(serde_json::json!({"owner": "ruckc"})).unwrap();
        let mut p = Project::new("enzarb", spec);
        p.status = status;
        p
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
