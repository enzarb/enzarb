//! Leader election on a `coordination.k8s.io/v1` Lease, so the operator can run 2 replicas.
//!
//! Every replica starts, serves health, and then blocks in [`Elector::acquire`]. Only the
//! holder runs the controller. The holder renews every `renew_interval`; if it cannot renew
//! within `renew_deadline` (or sees another holder) [`Elector::hold`] returns and the process
//! exits so the kubelet restarts it as a standby. On clean shutdown the lease is released so
//! the standby takes over immediately instead of waiting for expiry.

use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use k8s_openapi::jiff::Timestamp;
use kube::api::{Api, PostParams};
use kube::{Client, ResourceExt};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

pub struct Config {
    pub namespace: String,
    pub name: String,
    pub identity: String,
    pub lease_duration: Duration,
    pub renew_interval: Duration,
    pub renew_deadline: Duration,
}

/// What this replica should do given the current Lease contents.
#[derive(Debug, PartialEq, Eq)]
pub enum Role {
    /// We hold it: extend it.
    Renew,
    /// Unheld or expired: take it (bumping `leaseTransitions`).
    Take,
    /// Another replica holds a live lease.
    Follow,
}

pub fn evaluate(spec: &LeaseSpec, me: &str, now: Timestamp) -> Role {
    let holder = spec.holder_identity.as_deref().unwrap_or("");
    if holder == me {
        return Role::Renew;
    }
    if holder.is_empty() {
        return Role::Take;
    }
    let last = spec
        .renew_time
        .as_ref()
        .or(spec.acquire_time.as_ref())
        .map(|t| t.0.as_second());
    let duration = i64::from(spec.lease_duration_seconds.unwrap_or(0));
    match last {
        Some(last) if last + duration >= now.as_second() => Role::Follow,
        _ => Role::Take,
    }
}

/// The spec to write for `role`, derived from the existing spec (if any).
pub fn next_spec(old: Option<&LeaseSpec>, role: &Role, cfg: &Config, now: Timestamp) -> LeaseSpec {
    let mut spec = old.cloned().unwrap_or_default();
    spec.holder_identity = Some(cfg.identity.clone());
    spec.lease_duration_seconds = Some(cfg.lease_duration.as_secs() as i32);
    spec.renew_time = Some(MicroTime(now));
    if *role == Role::Take || old.is_none() {
        spec.acquire_time = Some(MicroTime(now));
        spec.lease_transitions = Some(spec.lease_transitions.unwrap_or(-1) + 1);
    }
    spec
}

pub struct Elector {
    api: Api<Lease>,
    cfg: Config,
}

impl Elector {
    pub fn new(client: Client, cfg: Config) -> Self {
        Self {
            api: Api::namespaced(client, &cfg.namespace),
            cfg,
        }
    }

    /// One acquire-or-renew attempt. `Ok(true)` means we hold the lease afterwards.
    async fn try_once(&self) -> Result<bool, kube::Error> {
        let now = Timestamp::now();
        let cfg = &self.cfg;
        let Some(mut lease) = self.api.get_opt(&cfg.name).await? else {
            let lease = Lease {
                metadata: ObjectMeta {
                    name: Some(cfg.name.clone()),
                    namespace: Some(cfg.namespace.clone()),
                    ..Default::default()
                },
                spec: Some(next_spec(None, &Role::Take, cfg, now)),
            };
            return match self.api.create(&PostParams::default(), &lease).await {
                Ok(_) => Ok(true),
                Err(kube::Error::Api(s)) if s.code == 409 => Ok(false),
                Err(e) => Err(e),
            };
        };
        let old = lease.spec.clone().unwrap_or_default();
        let role = evaluate(&old, &cfg.identity, now);
        if role == Role::Follow {
            return Ok(false);
        }
        lease.spec = Some(next_spec(Some(&old), &role, cfg, now));
        // `replace` carries the resourceVersion from the get, so a racing replica gets a 409.
        match self
            .api
            .replace(&lease.name_any(), &PostParams::default(), &lease)
            .await
        {
            Ok(_) => Ok(true),
            Err(kube::Error::Api(s)) if s.code == 409 => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Blocks until this replica holds the lease.
    pub async fn acquire(&self) {
        loop {
            match self.try_once().await {
                Ok(true) => {
                    info!(identity = %self.cfg.identity, lease = %self.cfg.name, "acquired leadership");
                    return;
                }
                Ok(false) => debug!("lease held by another replica"),
                Err(e) => warn!(error = %e, "lease acquire failed"),
            }
            tokio::time::sleep(self.cfg.renew_interval).await;
        }
    }

    /// Renews until leadership is lost; then returns.
    pub async fn hold(&self) {
        let mut last_ok = Instant::now();
        loop {
            tokio::time::sleep(self.cfg.renew_interval).await;
            match self.try_once().await {
                Ok(true) => last_ok = Instant::now(),
                Ok(false) => {
                    warn!("lease taken by another replica");
                    return;
                }
                Err(e) => {
                    warn!(error = %e, "lease renew failed");
                    if last_ok.elapsed() >= self.cfg.renew_deadline {
                        warn!("renew deadline exceeded");
                        return;
                    }
                }
            }
        }
    }

    /// Clears the holder so a standby can take over at once. Best effort.
    pub async fn release(&self) {
        let Ok(Some(mut lease)) = self.api.get_opt(&self.cfg.name).await else {
            return;
        };
        let Some(spec) = lease.spec.as_mut() else {
            return;
        };
        if spec.holder_identity.as_deref() != Some(self.cfg.identity.as_str()) {
            return;
        }
        spec.holder_identity = None;
        spec.renew_time = None;
        match self
            .api
            .replace(&self.cfg.name, &PostParams::default(), &lease)
            .await
        {
            Ok(_) => info!("released leadership"),
            Err(e) => warn!(error = %e, "lease release failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config {
            namespace: "enzarb".into(),
            name: "enzarb-sandbox-operator".into(),
            identity: "me".into(),
            lease_duration: Duration::from_secs(15),
            renew_interval: Duration::from_secs(5),
            renew_deadline: Duration::from_secs(10),
        }
    }

    fn at(s: i64) -> Timestamp {
        Timestamp::from_second(s).unwrap()
    }

    fn held(holder: &str, renewed: i64) -> LeaseSpec {
        LeaseSpec {
            holder_identity: Some(holder.into()),
            lease_duration_seconds: Some(15),
            renew_time: Some(MicroTime(at(renewed))),
            lease_transitions: Some(3),
            ..Default::default()
        }
    }

    #[test]
    fn own_lease_is_renewed() {
        assert_eq!(evaluate(&held("me", 0), "me", at(1000)), Role::Renew);
    }

    #[test]
    fn live_foreign_lease_is_followed() {
        assert_eq!(evaluate(&held("other", 100), "me", at(110)), Role::Follow);
        assert_eq!(evaluate(&held("other", 100), "me", at(115)), Role::Follow);
    }

    #[test]
    fn expired_foreign_lease_is_taken() {
        assert_eq!(evaluate(&held("other", 100), "me", at(116)), Role::Take);
    }

    #[test]
    fn released_or_empty_lease_is_taken() {
        assert_eq!(evaluate(&LeaseSpec::default(), "me", at(1)), Role::Take);
        let mut s = held("other", 100);
        s.holder_identity = None;
        assert_eq!(evaluate(&s, "me", at(101)), Role::Take);
    }

    #[test]
    fn take_bumps_transitions_and_acquire_time() {
        let old = held("other", 100);
        let s = next_spec(Some(&old), &Role::Take, &cfg(), at(200));
        assert_eq!(s.holder_identity.as_deref(), Some("me"));
        assert_eq!(s.lease_transitions, Some(4));
        assert_eq!(s.acquire_time, Some(MicroTime(at(200))));
        assert_eq!(s.renew_time, Some(MicroTime(at(200))));
        assert_eq!(s.lease_duration_seconds, Some(15));
    }

    #[test]
    fn renew_keeps_acquire_time_and_transitions() {
        let mut old = held("me", 100);
        old.acquire_time = Some(MicroTime(at(50)));
        let s = next_spec(Some(&old), &Role::Renew, &cfg(), at(105));
        assert_eq!(s.acquire_time, Some(MicroTime(at(50))));
        assert_eq!(s.lease_transitions, Some(3));
        assert_eq!(s.renew_time, Some(MicroTime(at(105))));
    }

    #[test]
    fn first_create_starts_at_zero_transitions() {
        let s = next_spec(None, &Role::Take, &cfg(), at(1));
        assert_eq!(s.lease_transitions, Some(0));
    }
}
