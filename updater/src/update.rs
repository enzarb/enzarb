//! The one place that decides and applies a version bump, shared by the webhook and the poller.

use semver::Version;

use crate::{
    charts::{Charts, Current},
    filter::{self, Upgrade},
};

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Patched,
    /// Already at, or above, the version (never downgrades).
    Noop,
    /// No HelmChart: logged, nothing created (the cluster bootstrap owns it).
    Missing,
    /// `spec.version` is not plain semver; never overwritten.
    Unparsable(String),
    /// Reading the HelmChart failed.
    ReadFailed,
    /// Patching the HelmChart failed.
    PatchFailed,
}

/// Read-compare-apply under `lock`, so concurrent callers cannot apply out of order.
pub async fn apply(
    charts: &dyn Charts,
    lock: &tokio::sync::Mutex<()>,
    version: &Version,
    source: &str,
) -> Outcome {
    let v = version.to_string();
    let _guard = lock.lock().await;
    let current = match charts.current().await {
        Ok(Current::Missing) => {
            tracing::warn!(decision = "noop", source, version = %v, reason = "HelmChart does not exist; not creating it (cluster bootstrap owns it)");
            return Outcome::Missing;
        }
        Ok(Current::Present(c)) => c,
        Err(e) => {
            tracing::error!(decision = "error", source, version = %v, error = %format!("{e:#}"), reason = "read HelmChart");
            return Outcome::ReadFailed;
        }
    };
    match filter::compare(version, current.as_deref()) {
        Err(e) => {
            tracing::error!(decision = "error", source, version = %v, error = %e);
            Outcome::Unparsable(e)
        }
        Ok(Upgrade::Equal) => {
            tracing::info!(decision = "noop", source, version = %v, current = current.as_deref(), reason = "already at version");
            Outcome::Noop
        }
        Ok(Upgrade::Older) => {
            tracing::info!(decision = "noop", source, version = %v, current = current.as_deref(), reason = "older than current, never downgrade");
            Outcome::Noop
        }
        Ok(Upgrade::Patch) => match charts.set_version(&v).await {
            Ok(()) => {
                tracing::info!(decision = "patched", source, version = %v, from = current.as_deref());
                Outcome::Patched
            }
            Err(e) => {
                tracing::error!(decision = "error", source, version = %v, error = %format!("{e:#}"), reason = "patch HelmChart");
                Outcome::PatchFailed
            }
        },
    }
}
