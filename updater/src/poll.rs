//! Polling mode: list the chart's tags, take the highest plain semver, apply it. Needs no
//! webhook, so nothing inbound.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, bail};
use semver::Version;

use crate::{
    charts::Charts,
    registry::Registry,
    update::{self, Outcome},
};

/// Whether the last poll succeeded; backs `/readyz` when polling is on.
#[derive(Default)]
pub struct PollHealth(AtomicBool);

impl PollHealth {
    pub fn set(&self, ok: bool) {
        self.0.store(ok, Ordering::Relaxed);
    }
    pub fn ok(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Highest strictly plain `X.Y.Z` tag (a leading `v` is not a chart tag; prerelease and build
/// metadata are skipped, as is anything else).
pub fn highest(tags: &[String]) -> Option<Version> {
    tags.iter()
        .filter_map(|t| Version::parse(t).ok())
        .filter(|v| v.pre.is_empty() && v.build.is_empty())
        .max()
}

/// One poll. `Err` means the poll failed (registry unreachable, HelmChart unreadable or
/// unpatchable, `spec.version` unparsable); a missing HelmChart or no tags is success.
pub async fn poll_once(
    registry: &dyn Registry,
    charts: &dyn Charts,
    lock: &tokio::sync::Mutex<()>,
) -> Result<()> {
    let tags = registry.list_tags().await?;
    let Some(v) = highest(&tags) else {
        tracing::info!(
            decision = "noop",
            source = "poll",
            tags = tags.len(),
            reason = "no plain semver tag"
        );
        return Ok(());
    };
    match update::apply(charts, lock, &v, "poll").await {
        Outcome::Patched | Outcome::Noop | Outcome::Missing => Ok(()),
        Outcome::Unparsable(e) => bail!(e),
        Outcome::ReadFailed => bail!("cannot read HelmChart"),
        Outcome::PatchFailed => bail!("patch failed"),
    }
}

/// Poll forever every `every`, first poll immediately.
pub async fn run(
    registry: Arc<dyn Registry>,
    charts: Arc<dyn Charts>,
    lock: Arc<tokio::sync::Mutex<()>>,
    health: Arc<PollHealth>,
    every: Duration,
) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        match poll_once(&*registry, &*charts, &lock).await {
            Ok(()) => health.set(true),
            Err(e) => {
                health.set(false);
                tracing::error!(decision = "error", source = "poll", error = %format!("{e:#}"), reason = "poll failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anyhow::anyhow;
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

    struct FakeHelmCharts {
        cur: Current,
        set: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Charts for FakeHelmCharts {
        async fn current(&self) -> Result<Current> {
            Ok(self.cur.clone())
        }
        async fn set_version(&self, v: &str) -> Result<()> {
            self.set.lock().unwrap().push(v.into());
            Ok(())
        }
    }

    fn tags(t: &[&str]) -> Result<Vec<String>, String> {
        Ok(t.iter().map(|s| s.to_string()).collect())
    }

    async fn poll(reg: Result<Vec<String>, String>, cur: Current) -> (Result<()>, Vec<String>) {
        let c = FakeHelmCharts {
            cur,
            set: Mutex::new(vec![]),
        };
        let r = poll_once(&Reg(reg), &c, &Default::default()).await;
        (r, c.set.lock().unwrap().clone())
    }

    fn present(v: &str) -> Current {
        Current::Present(Some(v.into()))
    }

    #[test]
    fn highest_is_numeric_plain_semver() {
        let t = |l: &[&str]| highest(&l.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            t(&[
                "0.9.0",
                "0.10.0",
                "latest",
                "v9.9.9",
                "1.0.0-rc.1",
                "0.10.0+b"
            ]),
            Some(Version::new(0, 10, 0))
        );
        assert_eq!(t(&["latest", "sha-abc"]), None);
        assert_eq!(t(&[]), None);
    }

    #[tokio::test]
    async fn patches_to_highest_tag() {
        let (r, set) = poll(tags(&["0.1.0", "0.3.0", "0.2.0"]), present("0.2.0")).await;
        assert!(r.is_ok());
        assert_eq!(set, vec!["0.3.0"]);
    }

    #[tokio::test]
    async fn never_downgrades_and_noops_when_equal() {
        for cur in ["0.3.0", "0.4.0"] {
            let (r, set) = poll(tags(&["0.1.0", "0.3.0"]), present(cur)).await;
            assert!(r.is_ok());
            assert!(set.is_empty());
        }
    }

    #[tokio::test]
    async fn missing_helmchart_is_logged_and_not_created() {
        let (r, set) = poll(tags(&["0.3.0"]), Current::Missing).await;
        assert!(r.is_ok());
        assert!(set.is_empty());
    }

    #[tokio::test]
    async fn unset_version_is_patched() {
        let (_, set) = poll(tags(&["0.3.0"]), Current::Present(None)).await;
        assert_eq!(set, vec!["0.3.0"]);
    }

    #[tokio::test]
    async fn registry_error_and_unparsable_current_fail_the_poll() {
        let (r, set) = poll(Err("boom".into()), present("0.1.0")).await;
        assert!(r.is_err() && set.is_empty());
        let (r, set) = poll(tags(&["0.3.0"]), present("latest")).await;
        assert!(r.is_err() && set.is_empty());
    }

    #[tokio::test]
    async fn no_semver_tags_is_ok_and_does_nothing() {
        let (r, set) = poll(tags(&["latest"]), present("0.1.0")).await;
        assert!(r.is_ok() && set.is_empty());
    }
}
