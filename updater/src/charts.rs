//! Reads and patches `HelmChart.spec.version` (helm.cattle.io/v1).

use crate::rollout::Rollout;
use anyhow::Result;
use async_trait::async_trait;
use k8s_openapi::api::{apps::v1::Deployment, batch::v1::Job};
use kube::{
    Api, Client,
    api::{Patch, PatchParams},
    core::{ApiResource, DynamicObject, GroupVersionKind},
};

pub const FIELD_MANAGER: &str = "enzarb-updater";

#[derive(Debug, Clone, PartialEq)]
pub enum Current {
    Missing,
    /// The HelmChart exists; its `spec.version` if set.
    Present(Option<String>),
}

#[async_trait]
pub trait Charts: Send + Sync {
    /// The HelmChart's state. A missing HelmChart is `Current::Missing`, never an error: the
    /// cluster bootstrap owns its creation.
    async fn current(&self) -> Result<Current>;
    /// Server-side apply of `spec.version` only. The RBAC grants no `create`, so this cannot
    /// create a HelmChart that was deleted since `current`.
    async fn set_version(&self, version: &str) -> Result<()>;
    /// Desired vs installed version and whether the helm-install Job is failing. Read-only.
    async fn rollout(&self) -> Result<Rollout>;
}

pub struct KubeCharts {
    api: Api<DynamicObject>,
    jobs: Api<Job>,
    deployments: Api<Deployment>,
    name: String,
}

impl KubeCharts {
    pub fn new(client: Client, namespace: &str, release_namespace: &str, name: &str) -> Self {
        let ar = ApiResource::from_gvk(&GroupVersionKind::gvk("helm.cattle.io", "v1", "HelmChart"));
        Self {
            api: Api::namespaced_with(client.clone(), namespace, &ar),
            jobs: Api::namespaced(client.clone(), namespace),
            deployments: Api::namespaced(client, release_namespace),
            name: name.into(),
        }
    }
}

/// The RKE2 helm-controller runs `helm-install-<name>` with the HelmChart's `spec.version` in
/// the `VERSION` env var of its container.
fn job_version(job: &Job) -> Option<String> {
    job.spec
        .as_ref()?
        .template
        .spec
        .as_ref()?
        .containers
        .iter()
        .flat_map(|c| c.env.iter().flatten())
        .find(|e| e.name == "VERSION")
        .and_then(|e| e.value.clone())
        .filter(|v| !v.is_empty())
}

pub const OPERATOR_DEPLOYMENT: &str = "enzarb-sandbox-operator";

/// The `ENZARB_SANDBOX_IMAGE` env of the operator Deployment: the chart's `operator.sandboxImage`.
pub fn deployment_sandbox_image(d: &Deployment) -> Option<String> {
    d.spec
        .as_ref()?
        .template
        .spec
        .as_ref()?
        .containers
        .iter()
        .flat_map(|c| c.env.iter().flatten())
        .find(|e| e.name == "ENZARB_SANDBOX_IMAGE")
        .and_then(|e| e.value.clone())
        .filter(|v| !v.is_empty())
}

/// The tag of an image reference (`registry/name:1.2.3@sha256:...`) without a leading `v`.
pub fn image_tag(image: &str) -> Option<String> {
    let r = image.split('@').next()?;
    let colon = r.rfind(':')?;
    if colon < r.rfind('/').map_or(0, |s| s + 1) {
        return None;
    }
    let tag = r[colon + 1..].trim_start_matches('v');
    (!tag.is_empty()).then(|| tag.to_string())
}

/// `installed` and `job_failed` from the helm-install Job. A succeeded Job means its version is
/// installed; a Job that never succeeded tells us nothing about what is installed.
pub fn job_state(job: &Job) -> (Option<String>, bool) {
    let st = job.status.as_ref();
    let succeeded = st.and_then(|s| s.succeeded).unwrap_or(0) > 0;
    let failed = st.and_then(|s| s.failed).unwrap_or(0) > 0
        || st.is_some_and(|s| {
            s.conditions
                .iter()
                .flatten()
                .any(|c| c.type_ == "Failed" && c.status == "True")
        });
    let installed = if succeeded { job_version(job) } else { None };
    (installed, failed && !succeeded)
}

/// When the succeeded helm-install Job completed (RFC 3339, UTC): the chart was deployed then.
pub fn job_completed_at(job: &Job) -> Option<String> {
    let st = job.status.as_ref()?;
    if st.succeeded.unwrap_or(0) == 0 {
        return None;
    }
    st.completion_time.as_ref().map(|t| t.0.to_string())
}

#[async_trait]
impl Charts for KubeCharts {
    async fn current(&self) -> Result<Current> {
        match self.api.get_opt(&self.name).await? {
            None => Ok(Current::Missing),
            Some(o) => Ok(Current::Present(
                o.data["spec"]["version"].as_str().map(String::from),
            )),
        }
    }

    async fn rollout(&self) -> Result<Rollout> {
        let Some(chart) = self.api.get_opt(&self.name).await? else {
            return Ok(Rollout::default());
        };
        let desired = chart.data["spec"]["version"].as_str().map(String::from);
        let job = self
            .jobs
            .get_opt(&format!("helm-install-{}", self.name))
            .await?;
        let (installed, job_failed) = job.as_ref().map(job_state).unwrap_or((None, false));
        let installed_at = installed
            .as_ref()
            .and(job.as_ref())
            .and_then(job_completed_at);
        // The chart's `operator.sandboxImage` reaches the operator as ENZARB_SANDBOX_IMAGE. Only
        // read when a version is installed; a missing Deployment or RBAC just leaves it unknown.
        let sandbox_image = if installed.is_some() {
            match self.deployments.get_opt(OPERATOR_DEPLOYMENT).await {
                Ok(d) => d.as_ref().and_then(deployment_sandbox_image),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot read the operator Deployment");
                    None
                }
            }
        } else {
            None
        };
        Ok(Rollout {
            desired,
            installed,
            installed_at,
            sandbox_image,
            job_failed,
        })
    }

    async fn set_version(&self, version: &str) -> Result<()> {
        let body = serde_json::json!({
            "apiVersion": "helm.cattle.io/v1",
            "kind": "HelmChart",
            "metadata": { "name": self.name },
            "spec": { "version": version },
        });
        // force: we deliberately take ownership of spec.version from whoever set it before.
        self.api
            .patch(
                &self.name,
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(&body),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(version: &str, succeeded: i32, failed: i32) -> Job {
        serde_json::from_value(serde_json::json!({
            "spec": {"template": {"spec": {"containers": [
                {"name": "helm", "env": [{"name": "VERSION", "value": version}]}
            ]}}},
            "status": {"succeeded": succeeded, "failed": failed},
        }))
        .unwrap()
    }

    #[test]
    fn succeeded_job_reports_its_version() {
        assert_eq!(
            job_state(&job("0.9.0", 1, 0)),
            (Some("0.9.0".into()), false)
        );
        // earlier failed pods do not matter once the Job succeeded
        assert_eq!(
            job_state(&job("0.9.0", 1, 2)),
            (Some("0.9.0".into()), false)
        );
    }

    #[test]
    fn image_tags_are_extracted() {
        assert_eq!(
            image_tag("ghcr.io/enzarb/enzarb-sandbox:0.34.0").as_deref(),
            Some("0.34.0")
        );
        assert_eq!(image_tag("h:5000/x:v1.2@sha256:ab").as_deref(), Some("1.2"));
        assert_eq!(image_tag("h:5000/x"), None);
        assert_eq!(image_tag("x"), None);
    }

    #[test]
    fn deployment_image_is_read_from_env() {
        let d: Deployment = serde_json::from_value(serde_json::json!({
            "spec": {"selector": {}, "template": {"spec": {"containers": [
                {"name": "operator", "env": [{"name": "ENZARB_SANDBOX_IMAGE", "value": "r/s:0.34.0"}]}
            ]}}}
        }))
        .unwrap();
        assert_eq!(deployment_sandbox_image(&d).as_deref(), Some("r/s:0.34.0"));
    }

    #[test]
    fn completion_time_is_reported_for_a_succeeded_job() {
        let mut j = job("0.9.0", 1, 0);
        assert_eq!(job_completed_at(&j), None);
        j.status.as_mut().unwrap().completion_time =
            serde_json::from_value(serde_json::json!("2026-10-12T03:05:00Z")).unwrap();
        assert_eq!(
            job_completed_at(&j).as_deref(),
            Some("2026-10-12T03:05:00Z")
        );
        j.status.as_mut().unwrap().succeeded = Some(0);
        assert_eq!(job_completed_at(&j), None);
    }

    #[test]
    fn failing_job_is_flagged_and_installed_unknown() {
        assert_eq!(job_state(&job("0.10.0", 0, 1)), (None, true));
    }

    #[test]
    fn backoff_limit_condition_is_a_failure() {
        let j: Job = serde_json::from_value(serde_json::json!({
            "spec": {"template": {"spec": {"containers": []}}},
            "status": {"conditions": [{"type": "Failed", "status": "True", "reason": "BackoffLimitExceeded"}]},
        }))
        .unwrap();
        assert_eq!(job_state(&j), (None, true));
    }

    #[test]
    fn running_job_is_neither() {
        assert_eq!(job_state(&job("0.10.0", 0, 0)), (None, false));
    }
}
