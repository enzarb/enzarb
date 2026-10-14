//! Rollout visibility: the chart version oldie asked for (`desired`), the one the RKE2
//! helm-controller last installed (`installed`) and the latest published (`latest`), cached for
//! `/status` and logged when an upgrade is stuck. Nothing here changes the cluster.

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use serde::Serialize;

use crate::{charts::Charts, poll, registry::Registry};

/// What the cluster says about the HelmChart's rollout.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rollout {
    /// `HelmChart.spec.version`; `None` when the HelmChart is missing or sets no version.
    pub desired: Option<String>,
    /// The version of the last successful `helm-install-<name>` Job; `None` when unreadable.
    pub installed: Option<String>,
    /// The `helm-install-<name>` Job is failing (`status.failed > 0`, or a backoff-limit condition).
    pub job_failed: bool,
}

/// The `/status` body.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Status {
    pub installed: Option<String>,
    pub desired: Option<String>,
    pub latest: Option<String>,
    pub stuck: bool,
}

impl Status {
    pub fn new(r: &Rollout, latest: Option<String>) -> Self {
        // Only `desired != installed` with a failing Job is stuck: a Job that failed earlier and
        // then caught up leaves `installed == desired`.
        let stuck = r.job_failed && r.desired.is_some() && r.desired != r.installed;
        Self {
            installed: r.installed.clone(),
            desired: r.desired.clone(),
            latest,
            stuck,
        }
    }
}

/// Last computed status, served by `/status` without touching the cluster or the registry.
#[derive(Default)]
pub struct StatusCache(RwLock<Status>);

impl StatusCache {
    pub fn get(&self) -> Status {
        self.0.read().expect("status lock").clone()
    }
    fn set(&self, s: Status) -> Status {
        std::mem::replace(&mut *self.0.write().expect("status lock"), s)
    }
    #[cfg(test)]
    pub(crate) fn set_for_test(&self, s: Status) {
        self.set(s);
    }
}

/// One refresh: read the rollout and the latest tag, cache them and log stuck/recovered
/// transitions. A failed read keeps the previous value of that part and is logged.
pub async fn refresh(registry: &dyn Registry, charts: &dyn Charts, cache: &StatusCache) {
    let prev = cache.get();
    let rollout = match charts.rollout().await {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "cannot read HelmChart rollout");
            None
        }
    };
    let latest = match registry.list_tags().await {
        Ok(tags) => poll::highest(&tags).map(|v| v.to_string()),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "cannot list chart tags");
            prev.latest.clone()
        }
    };
    let next = match &rollout {
        Some(r) => Status::new(r, latest),
        None => Status {
            latest,
            ..prev.clone()
        },
    };
    let was_stuck = cache.set(next.clone()).stuck;
    match (was_stuck, next.stuck) {
        (_, true) => {
            tracing::error!(
                desired = next.desired.as_deref(),
                installed = next.installed.as_deref(),
                "helm upgrade failing"
            );
        }
        (true, false) => {
            tracing::info!(
                desired = next.desired.as_deref(),
                installed = next.installed.as_deref(),
                "helm upgrade recovered"
            );
        }
        (false, false) => {
            if next.installed != prev.installed || next.latest != prev.latest {
                tracing::info!(
                    installed = next.installed.as_deref(),
                    desired = next.desired.as_deref(),
                    latest = next.latest.as_deref(),
                    "chart status"
                );
            }
        }
    }
}

/// Refresh forever every `every`, first refresh immediately.
pub async fn run(
    registry: Arc<dyn Registry>,
    charts: Arc<dyn Charts>,
    cache: Arc<StatusCache>,
    every: Duration,
) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        refresh(&*registry, &*charts, &cache).await;
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;

    use super::*;
    use crate::charts::Current;

    struct Reg(Result<Vec<String>, String>);
    #[async_trait]
    impl Registry for Reg {
        async fn chart_exists(&self, _: &str) -> Result<bool> {
            Ok(true)
        }
        async fn list_tags(&self) -> Result<Vec<String>> {
            self.0.clone().map_err(|e| anyhow!(e))
        }
    }

    struct Ch(Result<Rollout, String>);
    #[async_trait]
    impl Charts for Ch {
        async fn current(&self) -> Result<Current> {
            Ok(Current::Missing)
        }
        async fn set_version(&self, _: &str) -> Result<()> {
            Ok(())
        }
        async fn rollout(&self) -> Result<Rollout> {
            self.0.clone().map_err(|e| anyhow!(e))
        }
    }

    fn ro(desired: Option<&str>, installed: Option<&str>, job_failed: bool) -> Rollout {
        Rollout {
            desired: desired.map(Into::into),
            installed: installed.map(Into::into),
            job_failed,
        }
    }

    fn tags() -> Result<Vec<String>, String> {
        Ok(vec!["0.9.0".into(), "0.10.0".into(), "latest".into()])
    }

    async fn status(reg: Result<Vec<String>, String>, r: Result<Rollout, String>) -> Status {
        let cache = StatusCache::default();
        refresh(&Reg(reg), &Ch(r), &cache).await;
        cache.get()
    }

    #[tokio::test]
    async fn healthy() {
        let s = status(tags(), Ok(ro(Some("0.9.0"), Some("0.9.0"), false))).await;
        assert_eq!(
            s,
            Status {
                installed: Some("0.9.0".into()),
                desired: Some("0.9.0".into()),
                latest: Some("0.10.0".into()),
                stuck: false
            }
        );
    }

    #[tokio::test]
    async fn stuck_when_behind_and_job_failed() {
        let s = status(tags(), Ok(ro(Some("0.10.0"), Some("0.9.0"), true))).await;
        assert!(s.stuck);
    }

    #[tokio::test]
    async fn failed_job_that_caught_up_is_not_stuck() {
        let s = status(tags(), Ok(ro(Some("0.9.0"), Some("0.9.0"), true))).await;
        assert!(!s.stuck);
    }

    #[tokio::test]
    async fn behind_without_failure_is_not_stuck() {
        let s = status(tags(), Ok(ro(Some("0.10.0"), Some("0.9.0"), false))).await;
        assert!(!s.stuck);
    }

    #[tokio::test]
    async fn unreadable_installed_is_null_and_failed_job_is_stuck() {
        let s = status(tags(), Ok(ro(Some("0.10.0"), None, true))).await;
        assert_eq!(s.installed, None);
        assert!(s.stuck);
    }

    #[tokio::test]
    async fn missing_helmchart_is_not_stuck() {
        let s = status(tags(), Ok(ro(None, None, false))).await;
        assert!(!s.stuck && s.desired.is_none());
        assert_eq!(s.latest.as_deref(), Some("0.10.0"));
    }

    #[tokio::test]
    async fn errors_keep_the_previous_values() {
        let cache = StatusCache::default();
        refresh(
            &Reg(tags()),
            &Ch(Ok(ro(Some("0.10.0"), Some("0.9.0"), true))),
            &cache,
        )
        .await;
        refresh(&Reg(Err("boom".into())), &Ch(Err("boom".into())), &cache).await;
        let s = cache.get();
        assert!(s.stuck);
        assert_eq!(s.latest.as_deref(), Some("0.10.0"));
        refresh(
            &Reg(tags()),
            &Ch(Ok(ro(Some("0.10.0"), Some("0.10.0"), false))),
            &cache,
        )
        .await;
        assert!(!cache.get().stuck);
    }
}
