//! Status condition logic: observations in, ordered `Project` conditions out. Pure.

use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{
    COND_READY, COND_RESOURCES_READY, COND_SANDBOX_CHANGE_PENDING, COND_SANDBOX_READY, LastRoll,
    PendingChange, REASON_INVALID_SPEC, REASON_RETRYING,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};

/// What the reconciler learned about one step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    /// Applied, but not ready yet (reason, message).
    Pending(String, String),
    /// Applying or reading failed in a way that is retried with backoff (e.g. a transient API
    /// error). Rendered `Unknown`/`Retrying`, which is deliberately not terminal-looking.
    Error(String),
    /// Failed with a specific reason that needs a person. `*Error`/`*Failed`/`*Failure` is
    /// reserved for such states, as the UI treats those suffixes as a failed step.
    Failure(String, String),
    /// The spec cannot be acted on until it is edited (reason `InvalidSpec`).
    Invalid(String),
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
    /// Auth Secret, workspace and docker PVCs, NetworkPolicy.
    pub resources: Check,
    pub sandbox: Check,
}

impl Observed {
    pub fn all_skipped() -> Self {
        Self {
            resources: Check::Skipped,
            sandbox: Check::Skipped,
        }
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
        Check::Error(m) => ("Unknown", REASON_RETRYING.to_string(), m.clone()),
        Check::Failure(r, m) => ("False", r.clone(), m.clone()),
        Check::Invalid(m) => ("False", REASON_INVALID_SPEC.to_string(), m.clone()),
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

/// A pod-shape change waiting for the sandbox to drain (see `roll`).
#[derive(Clone, Debug, PartialEq)]
pub struct RollPending {
    pub change: PendingChange,
    /// Condition message.
    pub message: String,
    /// Time left until the drain timeout applies the change.
    pub remaining: std::time::Duration,
    /// Set when this step deleted the pod: becomes `status.lastRoll`.
    pub rolled: Option<LastRoll>,
}

/// The informational `SandboxChangePending` condition, appended after the checklist conditions
/// from `compute`; it never affects `Ready`. `lastTransitionTime` is when the first change of
/// the pending episode was seen (`pendingChange.since`).
pub fn roll_condition(pending: &RollPending, generation: i64) -> Condition {
    Condition {
        type_: COND_SANDBOX_CHANGE_PENDING.to_string(),
        status: "True".to_string(),
        reason: COND_SANDBOX_CHANGE_PENDING.to_string(),
        message: pending.message.clone(),
        observed_generation: Some(generation),
        last_transition_time: pending.change.since.clone(),
    }
}

/// Builds `ResourcesReady`, `SandboxReady`, `Ready` in UI checklist order.
/// `lastTransitionTime` is kept while a condition's status is unchanged, so an unchanged
/// result compares equal and the status write can be skipped.
pub fn compute(
    generation: i64,
    obs: &Observed,
    existing: &[Condition],
    now: &Time,
) -> Vec<Condition> {
    let mut out = vec![
        condition(
            COND_RESOURCES_READY,
            &obs.resources,
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
        let mut sb = Sandbox::new("sb-x", Default::default());
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
                Some(Sandbox::new("sb-x", Default::default())),
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

    fn statuses(c: &[Condition]) -> Vec<(&str, &str)> {
        c.iter()
            .map(|c| (c.type_.as_str(), c.status.as_str()))
            .collect()
    }

    #[test]
    fn compute_table() {
        let pend = || Check::pending("SandboxMissing", "no sandbox");
        let cases: Vec<(&str, Observed, [&str; 3])> = vec![
            (
                "all ready",
                Observed {
                    resources: Check::Ok,
                    sandbox: Check::Ok,
                },
                ["True", "True", "True"],
            ),
            (
                "missing sandbox",
                Observed {
                    resources: Check::Ok,
                    sandbox: pend(),
                },
                ["True", "False", "False"],
            ),
            (
                "resources pending",
                Observed {
                    resources: Check::pending("X", "x"),
                    sandbox: Check::Ok,
                },
                ["False", "True", "False"],
            ),
            (
                "resources error blocks sandbox",
                Observed {
                    resources: Check::Error("denied".into()),
                    sandbox: Check::Skipped,
                },
                ["Unknown", "Unknown", "False"],
            ),
            (
                "nothing attempted",
                Observed::all_skipped(),
                ["Unknown", "Unknown", "False"],
            ),
        ];
        for (name, obs, want) in cases {
            let got = compute(3, &obs, &[], &t(10));
            let types: Vec<_> = got.iter().map(|c| c.type_.as_str()).collect();
            assert_eq!(types, ["ResourcesReady", "SandboxReady", "Ready"], "{name}");
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
            resources: Check::Ok,
            sandbox: Check::pending("X", "y"),
        };
        let c = compute(1, &obs, &[], &t(0));
        assert_eq!(c[2].reason, "NotReady");
        assert_eq!(c[2].message, "waiting for SandboxReady");
        assert_eq!(
            compute(
                1,
                &Observed {
                    sandbox: Check::Error("boom".into()),
                    ..obs
                },
                &[],
                &t(0)
            )[1]
            .message,
            "boom"
        );
    }

    #[test]
    fn retryable_error_is_unknown_and_not_terminal_looking() {
        let obs = Observed {
            resources: Check::Ok,
            sandbox: Check::Error("boom".into()),
        };
        let c = compute(1, &obs, &[], &t(0));
        assert_eq!(c[1].status, "Unknown");
        assert_eq!(c[1].reason, "Retrying");
        assert_eq!(c[1].message, "boom");
        assert_eq!(c[2].status, "False");
        assert_eq!(c[2].reason, "NotReady");
        for cond in &c {
            for suffix in ["Error", "Failed", "Failure"] {
                assert!(!cond.reason.ends_with(suffix), "{}", cond.reason);
            }
        }
    }

    #[test]
    fn transition_time_kept_while_status_unchanged() {
        let ok = Observed {
            resources: Check::Ok,
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
        assert_eq!(flipped[1].last_transition_time, t(99));
        assert_eq!(flipped[2].last_transition_time, t(99));
    }

    fn pending(reason: &str) -> RollPending {
        RollPending {
            change: PendingChange {
                generation: 2,
                since: t(10),
                deadline: None,
                reason: reason.into(),
            },
            message: format!("generation 2: {reason}"),
            remaining: std::time::Duration::from_secs(60),
            rolled: None,
        }
    }

    #[test]
    fn roll_condition_is_informational_and_uses_since() {
        let ok = Observed {
            resources: Check::Ok,
            sandbox: Check::Ok,
        };
        let base = compute(1, &ok, &[], &t(1));
        assert_eq!(base[2].status, "True");
        let c = roll_condition(&pending("image/sandbox: a -> b"), 1);
        assert_eq!(c.type_, "SandboxChangePending");
        assert_eq!(c.status, "True");
        assert_eq!(c.reason, "SandboxChangePending");
        assert_eq!(c.last_transition_time, t(10));
        // Ready is computed without it and stays True; compute never emits it.
        assert!(base.iter().all(|c| c.type_ != "SandboxChangePending"));
        assert_eq!(base[2].type_, "Ready");
    }

    #[test]
    fn invalid_spec_is_reported_on_resources_ready() {
        let obs = Observed {
            resources: Check::Invalid("project name too long".into()),
            ..Observed::all_skipped()
        };
        let c = compute(2, &obs, &[], &t(5));
        assert_eq!(c[0].type_, "ResourcesReady");
        assert_eq!(c[0].status, "False");
        assert_eq!(c[0].reason, "InvalidSpec");
        assert_eq!(c[0].message, "project name too long");
        assert_eq!(c[1].status, "Unknown");
        assert_eq!(c[2].status, "False");
    }

    #[test]
    fn rejected_resize_fails_resources_and_ready_follows() {
        let obs = Observed {
            resources: Check::Failure("WorkspaceResizeFailed".into(), "cannot shrink".into()),
            ..Observed::all_skipped()
        };
        let c = compute(1, &obs, &[], &t(0));
        assert_eq!(c[0].type_, "ResourcesReady");
        assert_eq!(c[0].status, "False");
        assert_eq!(c[0].reason, "WorkspaceResizeFailed");
        assert_eq!(c[0].message, "cannot shrink");
        assert_eq!(c[2].type_, "Ready");
        assert_eq!(c[2].status, "False");
    }

    #[test]
    fn resizing_is_in_progress_not_failed() {
        let obs = Observed {
            resources: Check::Pending("WorkspaceResizing".into(), "expanding".into()),
            sandbox: Check::Ok,
        };
        let c = compute(1, &obs, &[], &t(0));
        assert_eq!(c[0].status, "False");
        assert_eq!(c[0].reason, "WorkspaceResizing");
        for suffix in ["Error", "Failed", "Failure"] {
            assert!(!c[0].reason.ends_with(suffix));
        }
    }
}
