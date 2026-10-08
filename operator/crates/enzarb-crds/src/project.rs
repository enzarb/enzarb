//! The `Project` CRD (`enzarb.dev/v1alpha1`, cluster-scoped). See docs/contracts.md §2.

use crate::repo::RepoKey;
use k8s_openapi::api::core::v1::ResourceRequirements;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Condition types, in the order the UI boot checklist shows them.
pub const COND_TENANT_READY: &str = "TenantReady";
pub const COND_NAMESPACE_READY: &str = "NamespaceReady";
pub const COND_SANDBOX_READY: &str = "SandboxReady";
pub const COND_READY: &str = "Ready";
pub const CONDITION_TYPES: [&str; 4] = [
    COND_TENANT_READY,
    COND_NAMESPACE_READY,
    COND_SANDBOX_READY,
    COND_READY,
];

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[kube(
    group = "enzarb.dev",
    version = "v1alpha1",
    kind = "Project",
    plural = "projects",
    status = "ProjectStatus",
    derive = "PartialEq",
    printcolumn = r#"{"name":"Owner","type":"string","jsonPath":".spec.owner"}"#,
    printcolumn = r#"{"name":"Namespace","type":"string","jsonPath":".status.namespace"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSpec {
    /// Owner id; the Capsule Tenant is named `enzarb-<owner>`.
    pub owner: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub repositories: Vec<Repository>,
    #[serde(default)]
    pub run_mode: RunMode,
    #[serde(default)]
    pub triggers: Triggers,
    #[serde(default)]
    pub sandbox: SandboxSpec,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    /// Canonical https URL, e.g. `https://github.com/enzarb/enzarb-ui`.
    pub url: String,
    /// Connector name, e.g. `github`.
    pub connector: String,
    pub default_branch: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    #[default]
    Worktree,
    /// Post-MVP.
    Fresh,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Triggers {
    #[serde(default = "yes")]
    pub issues: bool,
    #[serde(default = "yes")]
    pub change_requests: bool,
    #[serde(default = "yes")]
    pub mentions: bool,
}

fn yes() -> bool {
    true
}

impl Default for Triggers {
    fn default() -> Self {
        Self {
            issues: true,
            change_requests: true,
            mentions: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSpec {
    /// Empty = operator default (`--sandbox-image`).
    #[serde(default)]
    pub image: String,
    /// Empty = none.
    #[serde(default)]
    pub runtime_class_name: String,
    /// PVC size, e.g. `20Gi`.
    #[serde(default = "default_storage")]
    pub storage: String,
    #[serde(default)]
    pub resources: ResourceRequirements,
}

fn default_storage() -> String {
    "20Gi".to_string()
}

impl Default for SandboxSpec {
    fn default() -> Self {
        Self {
            image: String::new(),
            runtime_class_name: String::new(),
            storage: default_storage(),
            resources: ResourceRequirements::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// `enzarb-p-<project name>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxStatus>,
    /// Ordered for the UI checklist.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStatus {
    /// Sandbox CR name in `status.namespace`.
    pub name: String,
    /// e.g. `ws://sandbox.enzarb-p-enzarb.svc:7070/v1/mux`.
    pub endpoint: String,
    pub auth_secret_ref: SecretKeyRef,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SecretKeyRef {
    pub name: String,
    pub key: String,
}

impl Project {
    /// Project namespace: `enzarb-p-<project name>`.
    pub fn namespace_name(&self) -> String {
        namespace_for(self.metadata.name.as_deref().unwrap_or_default())
    }

    /// Capsule Tenant name: `enzarb-<owner>`.
    pub fn tenant_name(&self) -> String {
        format!("enzarb-{}", self.spec.owner)
    }

    /// Whether any linked repository has this repo key (case-insensitive).
    /// Accepts keys such as `github:enzarb/enzarb-ui`; malformed keys never match.
    pub fn matches_repo(&self, key: &str) -> bool {
        let Ok(want) = RepoKey::parse(key) else {
            return false;
        };
        self.spec
            .repositories
            .iter()
            .filter_map(|r| RepoKey::from_connector_url(&r.connector, &r.url).ok())
            .any(|k| k == want)
    }
}

pub fn namespace_for(project_name: &str) -> String {
    format!("enzarb-p-{project_name}")
}

/// The Project CRD rendered as YAML (what `crdgen` prints and `deploy/crds/project.yaml` holds).
pub fn project_crd_yaml() -> String {
    use kube::CustomResourceExt;
    serde_yaml::to_string(&Project::crd()).expect("CRD serializes")
}
