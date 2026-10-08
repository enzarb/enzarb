//! Status condition logic: observations in, ordered `Project` conditions out. Pure.

use enzarb_crds::upstream::capsule::{Tenant, TenantStatusState};
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{COND_NAMESPACE_READY, COND_READY, COND_SANDBOX_READY, COND_TENANT_READY};
use k8s_openapi::api::core::v1::Namespace;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};

/// What the reconciler learned about one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    /// Applied, but not ready yet (reason, message).
    Pending(String, String),
    /// Applying or reading failed.
    Error(String),
    /// An earlier step prevented this one from being attempted.
    Skipped,
}

impl Check {
    fn pending(reason: &str, message: impl Into<String>) -> Self {
        Check::Pending(reason.into(), message.into())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub tenant: Check,
    pub namespace: Check,
    pub sandbox: Check,
}

impl Observed {
    pub fn all_skipped() -> Self {
        Self {
            tenant: Check::Skipped,
            namespace: Check::Skipped,
            sandbox: Check::Skipped,
        }
    }
}

/// The Tenant is ready once Capsule reports it `Active`.
pub fn tenant_check(t: &Tenant) -> Check {
    match t.status.as_ref().map(|s| &s.state) {
        Some(TenantStatusState::Active) => Check::Ok,
        Some(TenantStatusState::Cordoned) => Check::pending("TenantCordoned", "tenant is cordoned"),
        Some(TenantStatusState::Terminating) => {
            Check::pending("TenantTerminating", "tenant is terminating")
        }
        None => Check::pending(
            "TenantPending",
            "Capsule has not reported tenant status yet",
        ),
    }
}

pub fn namespace_check(ns: &Namespace) -> Check {
    match ns.status.as_ref().and_then(|s| s.phase.as_deref()) {
        Some("Active") => Check::Ok,
        Some(phase) => Check::pending("NamespaceNotActive", format!("namespace phase is {phase}")),
        // The API server defaults the phase to Active; a missing one means we just created it.
        None => Check::Ok,
    }
}

/// Maps the Sandbox's own `Ready` condition (pod ready and Service present) to a check.
pub fn sandbox_check(sb: Option<&Sandbox>) -> Check {
    let Some(sb) = sb else {
        return Check::pending("SandboxMissing", "Sandbox object does not exist yet");
    };
    let ready = sb
        .status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .and_then(|cs| cs.iter().find(|c| c.type_ == "Ready"));
    let Some(ready) = ready else {
        return Check::pending("SandboxPending", "Sandbox has not reported status yet");
    };
    if let (Some(seen), Some(want)) = (ready.observed_generation, sb.metadata.generation)
        && seen < want
    {
        return Check::pending(
            "SandboxOutdated",
            "Sandbox status is for an older generation",
        );
    }
    if ready.status == "True" {
        Check::Ok
    } else {
        let reason = if ready.reason.is_empty() {
            "SandboxNotReady"
        } else {
            ready.reason.as_str()
        };
        Check::pending(reason, ready.message.clone())
    }
}

fn condition(
    type_: &str,
    check: &Check,
    generation: i64,
    existing: &[Condition],
    now: &Time,
) -> Condition {
    let (status, reason, message) = match check {
        Check::Ok => ("True", "Ready".to_string(), String::new()),
        Check::Pending(r, m) => ("False", r.clone(), m.clone()),
        Check::Error(m) => ("False", "ReconcileError".to_string(), m.clone()),
        Check::Skipped => (
            "Unknown",
            "Blocked".to_string(),
            "waiting for an earlier step".to_string(),
        ),
    };
    let last_transition_time = existing
        .iter()
        .find(|c| c.type_ == type_ && c.status == status)
        .map_or_else(|| now.clone(), |c| c.last_transition_time.clone());
    Condition {
        type_: type_.to_string(),
        status: status.to_string(),
        reason,
        message,
        observed_generation: Some(generation),
        last_transition_time,
    }
}

/// Builds `TenantReady`, `NamespaceReady`, `SandboxReady`, `Ready` in UI checklist order.
/// `lastTransitionTime` is kept while a condition's status is unchanged, so an unchanged
/// result compares equal and the status write can be skipped.
pub fn compute(
    generation: i64,
    obs: &Observed,
    existing: &[Condition],
    now: &Time,
) -> Vec<Condition> {
    let mut out = vec![
        condition(COND_TENANT_READY, &obs.tenant, generation, existing, now),
        condition(
            COND_NAMESPACE_READY,
            &obs.namespace,
            generation,
            existing,
            now,
        ),
        condition(COND_SANDBOX_READY, &obs.sandbox, generation, existing, now),
    ];
    let not_ready: Vec<&str> = out
        .iter()
        .filter(|c| c.status != "True")
        .map(|c| c.type_.as_str())
        .collect();
    let ready = if not_ready.is_empty() {
        Check::Ok
    } else {
        Check::pending("NotReady", format!("waiting for {}", not_ready.join(", ")))
    };
    out.push(condition(COND_READY, &ready, generation, existing, now));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_crds::upstream::sandbox::SandboxStatus;
    use k8s_openapi::jiff::Timestamp;

    fn t(secs: i64) -> Time {
        Time(Timestamp::from_second(secs).unwrap())
    }

    fn sandbox_with(cond: Option<(&str, &str, &str)>, generation: i64, seen: i64) -> Sandbox {
        let mut sb = Sandbox::new("sandbox", Default::default());
        sb.metadata.generation = Some(generation);
        sb.status = Some(SandboxStatus {
            conditions: cond.map(|(status, reason, message)| {
                vec![Condition {
                    type_: "Ready".into(),
                    status: status.into(),
                    reason: reason.into(),
                    message: message.into(),
                    observed_generation: Some(seen),
                    last_transition_time: t(1),
                }]
            }),
            ..Default::default()
        });
        sb
    }

    #[test]
    fn sandbox_check_table() {
        let cases: Vec<(&str, Option<Sandbox>, Check)> = vec![
            (
                "missing",
                None,
                Check::pending("SandboxMissing", "Sandbox object does not exist yet"),
            ),
            (
                "no status",
                Some(Sandbox::new("sandbox", Default::default())),
                Check::pending("SandboxPending", "Sandbox has not reported status yet"),
            ),
            (
                "not ready",
                Some(sandbox_with(
                    Some(("False", "DependenciesNotReady", "Pod does not exist")),
                    1,
                    1,
                )),
                Check::pending("DependenciesNotReady", "Pod does not exist"),
            ),
            (
                "ready",
                Some(sandbox_with(
                    Some(("True", "DependenciesReady", "Pod is Ready")),
                    1,
                    1,
                )),
                Check::Ok,
            ),
            (
                "stale ready",
                Some(sandbox_with(Some(("True", "DependenciesReady", "")), 2, 1)),
                Check::pending(
                    "SandboxOutdated",
                    "Sandbox status is for an older generation",
                ),
            ),
            (
                "empty reason",
                Some(sandbox_with(Some(("False", "", "x")), 1, 1)),
                Check::pending("SandboxNotReady", "x"),
            ),
        ];
        for (name, sb, want) in cases {
            assert_eq!(sandbox_check(sb.as_ref()), want, "{name}");
        }
    }

    #[test]
    fn tenant_and_namespace_checks() {
        use enzarb_crds::upstream::capsule::TenantStatus;
        let mut tn = Tenant::new("enzarb-ruckc", Default::default());
        assert!(matches!(tenant_check(&tn), Check::Pending(r, _) if r == "TenantPending"));
        tn.status = Some(TenantStatus {
            conditions: vec![],
            size: 0,
            state: TenantStatusState::Active,
            classes: None,
            namespaces: None,
            observed_generation: None,
            owners: None,
            promotions: None,
            spaces: None,
        });
        assert_eq!(tenant_check(&tn), Check::Ok);
        tn.status.as_mut().unwrap().state = TenantStatusState::Cordoned;
        assert!(matches!(tenant_check(&tn), Check::Pending(r, _) if r == "TenantCordoned"));

        let mut ns = Namespace::default();
        assert_eq!(namespace_check(&ns), Check::Ok);
        ns.status = Some(k8s_openapi::api::core::v1::NamespaceStatus {
            phase: Some("Terminating".into()),
            ..Default::default()
        });
        assert!(matches!(namespace_check(&ns), Check::Pending(r, _) if r == "NamespaceNotActive"));
    }

    fn statuses(c: &[Condition]) -> Vec<(&str, &str)> {
        c.iter()
            .map(|c| (c.type_.as_str(), c.status.as_str()))
            .collect()
    }

    #[test]
    fn compute_table() {
        let pend = || Check::pending("SandboxMissing", "no sandbox");
        let cases: Vec<(&str, Observed, [&str; 4])> = vec![
            (
                "all ready",
                Observed {
                    tenant: Check::Ok,
                    namespace: Check::Ok,
                    sandbox: Check::Ok,
                },
                ["True", "True", "True", "True"],
            ),
            (
                "missing sandbox",
                Observed {
                    tenant: Check::Ok,
                    namespace: Check::Ok,
                    sandbox: pend(),
                },
                ["True", "True", "False", "False"],
            ),
            (
                "tenant pending",
                Observed {
                    tenant: Check::pending("TenantPending", "x"),
                    namespace: Check::Ok,
                    sandbox: Check::Ok,
                },
                ["False", "True", "True", "False"],
            ),
            (
                "namespace error blocks sandbox",
                Observed {
                    tenant: Check::Ok,
                    namespace: Check::Error("denied".into()),
                    sandbox: Check::Skipped,
                },
                ["True", "False", "Unknown", "False"],
            ),
            (
                "nothing attempted",
                Observed::all_skipped(),
                ["Unknown", "Unknown", "Unknown", "False"],
            ),
        ];
        for (name, obs, want) in cases {
            let got = compute(3, &obs, &[], &t(10));
            let types: Vec<_> = got.iter().map(|c| c.type_.as_str()).collect();
            assert_eq!(
                types,
                ["TenantReady", "NamespaceReady", "SandboxReady", "Ready"],
                "{name}"
            );
            let got_status: Vec<_> = statuses(&got).into_iter().map(|(_, s)| s).collect();
            assert_eq!(got_status, want, "{name}");
            assert!(
                got.iter().all(|c| c.observed_generation == Some(3)),
                "{name}"
            );
        }
    }

    #[test]
    fn ready_message_lists_blockers() {
        let obs = Observed {
            tenant: Check::Ok,
            namespace: Check::Ok,
            sandbox: Check::pending("X", "y"),
        };
        let c = compute(1, &obs, &[], &t(0));
        assert_eq!(c[3].reason, "NotReady");
        assert_eq!(c[3].message, "waiting for SandboxReady");
        assert_eq!(
            compute(
                1,
                &Observed {
                    sandbox: Check::Error("boom".into()),
                    ..obs
                },
                &[],
                &t(0)
            )[2]
            .message,
            "boom"
        );
    }

    #[test]
    fn transition_time_kept_while_status_unchanged() {
        let ok = Observed {
            tenant: Check::Ok,
            namespace: Check::Ok,
            sandbox: Check::Ok,
        };
        let first = compute(1, &ok, &[], &t(10));
        let again = compute(1, &ok, &first, &t(99));
        assert_eq!(first, again);
        let down = Observed {
            sandbox: Check::pending("X", "y"),
            ..ok
        };
        let flipped = compute(1, &down, &first, &t(99));
        assert_eq!(flipped[0].last_transition_time, t(10));
        assert_eq!(flipped[2].last_transition_time, t(99));
        assert_eq!(flipped[3].last_transition_time, t(99));
    }
}
