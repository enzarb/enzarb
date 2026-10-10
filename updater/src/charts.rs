//! Reads and patches `HelmChart.spec.version` (helm.cattle.io/v1).

use anyhow::Result;
use async_trait::async_trait;
use kube::{
    Api, Client,
    api::{Patch, PatchParams},
    core::{ApiResource, DynamicObject, GroupVersionKind},
};

pub const FIELD_MANAGER: &str = "enzarb-updater";

#[async_trait]
pub trait Charts: Send + Sync {
    /// Current `spec.version`; `None` if the field is unset. Errors if the HelmChart is missing.
    async fn current_version(&self) -> Result<Option<String>>;
    /// Server-side apply of `spec.version` only.
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
    async fn current_version(&self) -> Result<Option<String>> {
        let o = self.api.get(&self.name).await?;
        Ok(o.data["spec"]["version"].as_str().map(String::from))
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
