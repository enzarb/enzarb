//! Persisted deploy history: when each chart version finished installing. The updater has no
//! database, so the last `MAX_RELEASES` entries live in one ConfigMap and survive restarts.

use anyhow::{Context, Result};
use async_trait::async_trait;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::{
    Api, Client,
    api::{Patch, PatchParams},
};
use serde::{Deserialize, Serialize};

use crate::charts::FIELD_MANAGER;

pub const CONFIGMAP: &str = "enzarb-updater-releases";
const KEY: &str = "releases";
pub const MAX_RELEASES: usize = 20;

/// A chart version and when the helm-controller finished installing it (RFC 3339, UTC).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseTimes {
    pub version: String,
    pub deployed_at: String,
}

/// Appends `version` unless it is already the newest entry, then keeps the last `MAX_RELEASES`.
/// Returns whether the list changed. The list is ordered oldest first.
pub fn record(list: &mut Vec<ReleaseTimes>, version: &str, deployed_at: &str) -> bool {
    if list.last().is_some_and(|r| r.version == version) {
        return false;
    }
    list.push(ReleaseTimes {
        version: version.into(),
        deployed_at: deployed_at.into(),
    });
    let extra = list.len().saturating_sub(MAX_RELEASES);
    list.drain(..extra);
    true
}

#[async_trait]
pub trait History: Send + Sync {
    /// The stored list; empty when nothing was stored yet.
    async fn load(&self) -> Result<Vec<ReleaseTimes>>;
    async fn save(&self, list: &[ReleaseTimes]) -> Result<()>;
}

pub struct KubeHistory {
    api: Api<ConfigMap>,
}

impl KubeHistory {
    pub fn new(client: Client, namespace: &str) -> Self {
        Self {
            api: Api::namespaced(client, namespace),
        }
    }
}

#[async_trait]
impl History for KubeHistory {
    async fn load(&self) -> Result<Vec<ReleaseTimes>> {
        let Some(cm) = self.api.get_opt(CONFIGMAP).await? else {
            return Ok(Vec::new());
        };
        let Some(raw) = cm.data.as_ref().and_then(|d| d.get(KEY)) else {
            return Ok(Vec::new());
        };
        serde_json::from_str(raw).with_context(|| format!("parse ConfigMap {CONFIGMAP}"))
    }

    async fn save(&self, list: &[ReleaseTimes]) -> Result<()> {
        let body = serde_json::json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": { "name": CONFIGMAP },
            "data": { KEY: serde_json::to_string(list)? },
        });
        self.api
            .patch(
                CONFIGMAP,
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

    #[test]
    fn records_a_version_once() {
        let mut l = Vec::new();
        assert!(record(&mut l, "0.9.0", "2026-10-01T00:00:00Z"));
        assert!(!record(&mut l, "0.9.0", "2026-10-02T00:00:00Z"));
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].deployed_at, "2026-10-01T00:00:00Z");
    }

    #[test]
    fn is_bounded_to_the_newest_entries() {
        let mut l = Vec::new();
        for i in 0..(MAX_RELEASES + 5) {
            record(&mut l, &format!("0.{i}.0"), "2026-10-01T00:00:00Z");
        }
        assert_eq!(l.len(), MAX_RELEASES);
        assert_eq!(l[0].version, "0.5.0");
        assert_eq!(
            l.last().unwrap().version,
            format!("0.{}.0", MAX_RELEASES + 4)
        );
    }

    #[test]
    fn serialises_camel_case() {
        let v = serde_json::to_value(ReleaseTimes {
            version: "1".into(),
            deployed_at: "t".into(),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"version": "1", "deployedAt": "t"}));
    }
}
