//! Reads and patches `HelmChart.spec.version` (helm.cattle.io/v1).

use anyhow::Result;
use async_trait::async_trait;
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
}

pub struct KubeCharts {
    api: Api<DynamicObject>,
    name: String,
}

impl KubeCharts {
    pub fn new(client: Client, namespace: &str, name: &str) -> Self {
        let ar = ApiResource::from_gvk(&GroupVersionKind::gvk("helm.cattle.io", "v1", "HelmChart"));
        Self {
            api: Api::namespaced_with(client, namespace, &ar),
            name: name.into(),
        }
    }
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
