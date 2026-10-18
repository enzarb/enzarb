//! Rollout visibility: the chart version oldie asked for (`desired`), the one the RKE2
//! helm-controller last installed (`installed`) and the latest published (`latest`), cached for
//! `/status` and logged when an upgrade is stuck. Nothing here changes the cluster.

use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::Serialize;

use crate::{
    charts::Charts,
    history::{self, History, ReleaseTimes},
    poll,
    registry::Registry,
};

/// What the cluster says about the HelmChart's rollout.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rollout {
    /// `HelmChart.spec.version`; `None` when the HelmChart is missing or sets no version.
    pub desired: Option<String>,
    /// The version of the last successful `helm-install-<name>` Job; `None` when unreadable.
    pub installed: Option<String>,
    /// When that Job completed (RFC 3339); `None` when unknown.
    pub installed_at: Option<String>,
    /// The sandbox image the installed chart configures (`operator.sandboxImage`); `None` when unknown.
    pub sandbox_image: Option<String>,
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
    /// When recent chart versions finished installing, oldest first. Additive: an older UI
    /// ignores it.
    pub releases: Vec<ReleaseTimes>,
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
            releases: Vec::new(),
        }
    }
}

/// Last computed status, served by `/status` without touching the cluster or the registry.
#[derive(Default)]
pub struct StatusCache {
    status: RwLock<Status>,
    /// The stored history was read (an unread history must not be overwritten).
    loaded: AtomicBool,
    /// The list changed and the last save failed.
    unsaved: AtomicBool,
}

impl StatusCache {
    pub fn get(&self) -> Status {
        self.status.read().expect("status lock").clone()
    }
    fn set(&self, s: Status) -> Status {
        std::mem::replace(&mut *self.status.write().expect("status lock"), s)
    }
    #[cfg(test)]
    pub(crate) fn set_for_test(&self, s: Status) {
        self.set(s);
    }
}

/// One refresh: read the rollout and the latest tag, cache them and log stuck/recovered
/// transitions. A failed read keeps the previous value of that part and is logged.
pub async fn refresh(
    registry: &dyn Registry,
    charts: &dyn Charts,
    history: &dyn History,
    cache: &StatusCache,
) {
    if !cache.loaded.load(Ordering::SeqCst) {
        match history.load().await {
            Ok(list) => {
                cache.status.write().expect("status lock").releases = list;
                cache.loaded.store(true, Ordering::SeqCst);
            }
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "cannot read release history"),
        }
    }
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
    let mut next = match &rollout {
        Some(r) => Status {
            releases: prev.releases.clone(),
            ..Status::new(r, latest)
        },
        None => Status {
            latest,
            ..prev.clone()
        },
    };
    let installed = rollout
        .as_ref()
        .and_then(|r| r.installed.as_ref().zip(Some(r)));
    if let (Some((v, r)), true) = (installed, cache.loaded.load(Ordering::SeqCst)) {
        // Without the Job's completion time the first sighting is the best estimate.
        let at = r.installed_at.clone().unwrap_or_else(now);
        let sandbox = r
            .sandbox_image
            .as_deref()
            .and_then(crate::charts::image_tag);
        if history::record(&mut next.releases, v, &at, sandbox) {
            tracing::info!(version = %v, deployed_at = %at, "chart deployed");
            cache.unsaved.store(true, Ordering::SeqCst);
        }
    }
    if cache.unsaved.load(Ordering::SeqCst) {
        match history.save(&next.releases).await {
            Ok(()) => cache.unsaved.store(false, Ordering::SeqCst),
            // Kept in memory and retried on the next refresh; /status is unaffected.
            Err(e) => tracing::warn!(error = %format!("{e:#}"), "cannot save release history"),
        }
    }
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

fn now() -> String {
    k8s_openapi::jiff::Timestamp::now().to_string()
}

/// Refresh forever every `every`, first refresh immediately.
pub async fn run(
    registry: Arc<dyn Registry>,
    charts: Arc<dyn Charts>,
    history: Arc<dyn History>,
    cache: Arc<StatusCache>,
    every: Duration,
) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        refresh(&*registry, &*charts, &*history, &cache).await;
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, anyhow};
    use async_trait::async_trait;

    use super::*;
    use crate::charts::Current;
    use std::sync::Mutex;

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
            installed_at: installed.map(|_| "2026-10-12T03:05:00Z".into()),
            sandbox_image: installed.map(|v| format!("r/sandbox:{v}")),
            job_failed,
        }
    }

    fn tags() -> Result<Vec<String>, String> {
        Ok(vec!["0.9.0".into(), "0.10.0".into(), "latest".into()])
    }

    #[derive(Default)]
    struct Hist {
        stored: Mutex<Vec<ReleaseTimes>>,
        fail_load: bool,
        fail_save: bool,
        saves: Mutex<usize>,
    }
    #[async_trait]
    impl History for Hist {
        async fn load(&self) -> Result<Vec<ReleaseTimes>> {
            if self.fail_load {
                return Err(anyhow!("load boom"));
            }
            Ok(self.stored.lock().unwrap().clone())
        }
        async fn save(&self, list: &[ReleaseTimes]) -> Result<()> {
            if self.fail_save {
                return Err(anyhow!("save boom"));
            }
            *self.saves.lock().unwrap() += 1;
            *self.stored.lock().unwrap() = list.to_vec();
            Ok(())
        }
    }

    async fn status(reg: Result<Vec<String>, String>, r: Result<Rollout, String>) -> Status {
        let cache = StatusCache::default();
        refresh(&Reg(reg), &Ch(r), &Hist::default(), &cache).await;
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
                stuck: false,
                releases: vec![ReleaseTimes {
                    version: "0.9.0".into(),
                    deployed_at: "2026-10-12T03:05:00Z".into(),
                    sandbox_image_version: Some("0.9.0".into()),
                }],
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
            &Hist::default(),
            &cache,
        )
        .await;
        refresh(
            &Reg(Err("boom".into())),
            &Ch(Err("boom".into())),
            &Hist::default(),
            &cache,
        )
        .await;
        let s = cache.get();
        assert!(s.stuck);
        assert_eq!(s.latest.as_deref(), Some("0.10.0"));
        refresh(
            &Reg(tags()),
            &Ch(Ok(ro(Some("0.10.0"), Some("0.10.0"), false))),
            &Hist::default(),
            &cache,
        )
        .await;
        assert!(!cache.get().stuck);
    }

    async fn step(h: &Hist, cache: &StatusCache, installed: &str) {
        let r = Ok(ro(Some(installed), Some(installed), false));
        refresh(&Reg(tags()), &Ch(r), h, cache).await;
    }

    #[tokio::test]
    async fn a_new_installed_version_is_recorded_once() {
        let h = Hist::default();
        let cache = StatusCache::default();
        step(&h, &cache, "0.9.0").await;
        step(&h, &cache, "0.9.0").await;
        step(&h, &cache, "0.10.0").await;
        step(&h, &cache, "0.10.0").await;
        let versions: Vec<_> = cache
            .get()
            .releases
            .into_iter()
            .map(|r| r.version)
            .collect();
        assert_eq!(versions, ["0.9.0", "0.10.0"]);
        assert_eq!(*h.saves.lock().unwrap(), 2);
        assert_eq!(h.stored.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_restart_reloads_history_without_recording_again() {
        let h = Hist::default();
        step(&h, &StatusCache::default(), "0.9.0").await;
        let restarted = StatusCache::default();
        step(&h, &restarted, "0.9.0").await;
        assert_eq!(restarted.get().releases.len(), 1);
        assert_eq!(*h.saves.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn history_errors_do_not_break_status() {
        let h = Hist {
            fail_save: true,
            ..Default::default()
        };
        let cache = StatusCache::default();
        step(&h, &cache, "0.9.0").await;
        let s = cache.get();
        assert_eq!(s.installed.as_deref(), Some("0.9.0"));
        assert_eq!(s.releases.len(), 1);

        // An unreadable history is never overwritten with a partial list.
        let h = Hist {
            fail_load: true,
            ..Default::default()
        };
        let cache = StatusCache::default();
        step(&h, &cache, "0.9.0").await;
        let s = cache.get();
        assert_eq!(s.installed.as_deref(), Some("0.9.0"));
        assert!(s.releases.is_empty());
        assert_eq!(*h.saves.lock().unwrap(), 0);
    }
}
