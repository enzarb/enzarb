//! The `Project` CRD (`enzarb.dev/v1alpha1`, cluster-scoped). See docs/contracts.md §2.

use crate::repo::RepoKey;
use k8s_openapi::api::core::v1::ResourceRequirements;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::{CustomResource, KubeSchema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Condition types, in the order the UI boot checklist shows them.
///
/// `ResourcesReady` covers the auth Secret, both PVCs and the NetworkPolicy. An invalid spec is
/// reported there with reason [`REASON_INVALID_SPEC`].
pub const COND_RESOURCES_READY: &str = "ResourcesReady";
pub const COND_SANDBOX_READY: &str = "SandboxReady";
pub const COND_READY: &str = "Ready";
pub const CONDITION_TYPES: [&str; 3] = [COND_RESOURCES_READY, COND_SANDBOX_READY, COND_READY];
/// Terminal `False` reason on `ResourcesReady` for a spec the operator will not act on.
pub const REASON_INVALID_SPEC: &str = "InvalidSpec";
/// Non-terminal `Unknown` reason for a retryable reconcile error (e.g. a transient API failure).
pub const REASON_RETRYING: &str = "Retrying";
/// Terminal `False` reason on `ResourcesReady` when the API server refuses a PVC resize (a shrink,
/// or a StorageClass without `allowVolumeExpansion`). The `Failed` suffix makes the UI show it as
/// failed.
pub const REASON_WORKSPACE_RESIZE_FAILED: &str = "WorkspaceResizeFailed";
/// In-progress `False` reason on `ResourcesReady` while a PVC expansion is running. Deliberately
/// has no failure suffix.
pub const REASON_WORKSPACE_RESIZING: &str = "WorkspaceResizing";
/// Informational condition, `True` only while a pod-shape change (image, env, resources, ...) waits
/// for the sandbox to drain. Replaces the former `SandboxImageRoll`. Does not affect `Ready`.
pub const COND_SANDBOX_CHANGE_PENDING: &str = "SandboxChangePending";
/// Project annotation set (by workflows) to the `status.pendingChange.generation` it has drained
/// for. The operator applies the change only when it equals the pending generation.
pub const ANNOTATION_DRAINED_GENERATION: &str = "enzarb.dev/drained-generation";
/// Longest Project name: `sb-<name>-workspace` must fit a 63-character label (ADR-0013).
pub const MAX_PROJECT_NAME_LEN: usize = 50;

#[derive(CustomResource, Serialize, Deserialize, Clone, Debug, PartialEq, KubeSchema)]
#[kube(
    group = "enzarb.dev",
    version = "v1alpha1",
    kind = "Project",
    plural = "projects",
    status = "ProjectStatus",
    derive = "PartialEq",
    // ADR-0013: `sb-<name>-workspace` must fit a 63-character label, so names are DNS labels of
    // at most 50 characters. Create-only (`optionalOldSelf`): names are immutable, and root-level
    // rules also run on status writes and spec edits, which must keep working for any existing
    // Project. The operator re-checks this (`InvalidSpec`).
    validation = Rule::new("oldSelf.hasValue() || self.metadata.name.size() <= 50").optional_old_self(true).message("Project name must be at most 50 characters"),
    validation = Rule::new("oldSelf.hasValue() || self.metadata.name.matches('^[a-z0-9]([-a-z0-9]*[a-z0-9])?$')").optional_old_self(true).message("Project name must be a DNS label (lowercase letters, digits and '-')"),
    printcolumn = r#"{"name":"Owner","type":"string","jsonPath":".spec.owner"}"#,
    printcolumn = r#"{"name":"Namespace","type":"string","jsonPath":".status.namespace"}"#,
    printcolumn = r#"{"name":"Ready","type":"string","jsonPath":".status.conditions[?(@.type==\"Ready\")].status"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSpec {
    /// Owner id (GitHub login of the creator).
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
    /// Concurrent agent runs in the sandbox (1-16). Unset = the sandbox image default; the
    /// operator then injects nothing. Changing it recreates the sandbox pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 16))]
    pub max_runs: Option<u32>,
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
    /// PVC size, e.g. `20Gi`.
    #[serde(default = "default_storage")]
    pub storage: String,
    #[serde(default)]
    pub resources: ResourceRequirements,
    /// Sandbox CPU limit in cores (1-64). Unset = nothing injected (today's behaviour). The
    /// operator derives conservative requests (25% of the limit); explicit `resources` win per
    /// key. Changing it recreates the pod once the sandbox is drained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 64))]
    pub cpu: Option<u32>,
    /// Sandbox memory limit as `<int>Mi` or `<int>Gi` (512Mi-256Gi). Unset = nothing injected.
    /// Same derivation and roll semantics as `cpu`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(regex(pattern = r"^[1-9][0-9]*(Mi|Gi)$"), length(max = 12))]
    pub memory: Option<String>,
}

fn default_storage() -> String {
    "20Gi".to_string()
}

impl Default for SandboxSpec {
    fn default() -> Self {
        Self {
            image: String::new(),
            storage: default_storage(),
            resources: ResourceRequirements::default(),
            cpu: None,
            memory: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectStatus {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// The shared sandbox namespace (ADR-0013), e.g. `enzarb-sandboxes`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxStatus>,
    /// Ordered for the UI checklist.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// A pod-shape change waiting for the sandbox to drain. Absent when nothing is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_change: Option<PendingChange>,
}

/// See `ANNOTATION_DRAINED_GENERATION`. `generation` is a counter local to the pending change
/// (not `metadata.generation`); a newer spec change bumps it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingChange {
    pub generation: i64,
    /// When the first change of this pending episode was detected (drain timeout runs from here).
    pub since: Time,
    /// Human-readable summary of what differs.
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStatus {
    /// Sandbox CR name in `status.namespace`: `sb-<project>`.
    pub name: String,
    /// e.g. `ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux`.
    pub endpoint: String,
    pub auth_secret_ref: SecretKeyRef,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, JsonSchema)]
pub struct SecretKeyRef {
    pub name: String,
    pub key: String,
}

impl Project {
    fn project_name(&self) -> &str {
        self.metadata.name.as_deref().unwrap_or_default()
    }

    /// Sandbox CR and its headless Service: `sb-<name>`.
    pub fn sandbox_name(&self) -> String {
        sandbox_name_for(self.project_name())
    }

    /// Auth Secret (key `token`): `sb-<name>-auth`.
    pub fn auth_secret_name(&self) -> String {
        format!("{}-auth", self.sandbox_name())
    }

    /// Workspace PVC: `sb-<name>-workspace`.
    pub fn workspace_pvc_name(&self) -> String {
        format!("{}-workspace", self.sandbox_name())
    }

    /// Docker data PVC: `sb-<name>-docker`.
    pub fn docker_pvc_name(&self) -> String {
        format!("{}-docker", self.sandbox_name())
    }

    /// Per-Project NetworkPolicy: `sb-<name>`.
    pub fn network_policy_name(&self) -> String {
        self.sandbox_name()
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

/// `sb-<project name>`; the prefix keeps per-Project names from colliding in the shared namespace.
pub fn sandbox_name_for(project_name: &str) -> String {
    format!("sb-{project_name}")
}

/// The Project CRD rendered as YAML (what `crdgen` prints and `deploy/crds/project.yaml` holds).
pub fn project_crd_yaml() -> String {
    use kube::CustomResourceExt;
    serde_yaml::to_string(&Project::crd()).expect("CRD serializes")
}
