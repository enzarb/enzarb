//! Reads and patches `HelmChart.spec.version` (helm.cattle.io/v1).

use crate::rollout::Rollout;
use anyhow::Result;
use async_trait::async_trait;
use k8s_openapi::api::batch::v1::Job;
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
    name: String,
}

impl KubeCharts {
    pub fn new(client: Client, namespace: &str, name: &str) -> Self {
        let ar = ApiResource::from_gvk(&GroupVersionKind::gvk("helm.cattle.io", "v1", "HelmChart"));
        Self {
            api: Api::namespaced_with(client.clone(), namespace, &ar),
            jobs: Api::namespaced(client, namespace),
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
        Ok(Rollout {
            desired,
            installed,
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
