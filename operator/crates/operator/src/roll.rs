//! Applying pod-shape changes to a Project's running sandbox pod, drain first.
//!
//! The Sandbox template is updated by server-side apply, but agent-sandbox does not recreate a
//! running pod for it. When the desired pod shape (image, `ENZARB_*` env, container resources,
//! runtime class, operator-defined mounts) differs from the running pod, the change is queued as
//! `status.pendingChange` and the pod is left alone. It is deleted (never the Sandbox or PVCs)
//! only when the Project annotation `enzarb.dev/drained-generation` equals the pending
//! generation, when the owner asked for it with `enzarb.dev/roll-now-generation` (the same
//! exact match), when the sandbox reports no active sessions, or when the drain timeout expires.
//! `decide` is pure; `roll_step` does the API calls.
//!
//! A new field joins the queue by adding one line to `shape_of`.

use crate::conditions::RollPending;
use crate::desired::{self, Defaults};
use crate::quantity;
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{
    ANNOTATION_DRAINED_GENERATION, ANNOTATION_ROLL_NOW_GENERATION, PendingChange, Project,
};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use k8s_openapi::jiff::Timestamp;
use kube::api::{Api, DeleteParams, Preconditions};
use kube::{Client, ResourceExt};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;
use tracing::{info, warn};

/// Pod annotation reporting the number of active agent sessions/connections. Written by the
/// sandbox runtime (not yet by anything in this repo). Absent or unparsable means unknown. Until
/// a producer exists, an idle sandbox is released by the workflows drain ack instead.
pub const ANNOTATION_ACTIVE_SESSIONS: &str = "enzarb.dev/active-sessions";
/// Upper bound on the requeue delay while a change is pending.
pub const DEFER_POLL: Duration = Duration::from_secs(30);
/// A pod younger than this is never deleted for a change, which bounds crash-loop style flapping.
pub const MIN_POD_AGE_SECS: i64 = 120;
/// Longest `pendingChange.reason`.
const MAX_REASON: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// The sandbox does not report activity.
    Unknown,
    /// Number of active sessions reported by the sandbox.
    Active(u32),
}

impl Activity {
    pub fn of_pod(pod: &Pod) -> Self {
        pod.annotations()
            .get(ANNOTATION_ACTIVE_SESSIONS)
            .and_then(|v| v.trim().parse().ok())
            .map_or(Activity::Unknown, Activity::Active)
    }
}

/// Canonical `key -> value` description of the parts of a pod spec the operator controls.
pub type PodShape = BTreeMap<String, String>;

fn canonical_quantity(v: &Value) -> String {
    match v {
        Value::String(s) => quantity::milli(s).map_or_else(|| s.clone(), |m| m.to_string()),
        other => other.to_string(),
    }
}

/// Shape of a serialized `PodSpec`. Run on both the Sandbox template and the live pod.
pub fn shape_of(spec: &Value) -> PodShape {
    let mut out = PodShape::new();
    if let Some(rc) = spec.get("runtimeClassName").and_then(Value::as_str) {
        out.insert("runtimeClassName".into(), rc.into());
    }
    let containers = spec.get("containers").and_then(Value::as_array);
    for c in containers.into_iter().flatten() {
        let Some(name) = c.get("name").and_then(Value::as_str) else {
            continue;
        };
        if let Some(img) = c.get("image").and_then(Value::as_str) {
            out.insert(format!("image/{name}"), img.into());
        }
        for e in c.get("env").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(n), Some(v)) = (
                e.get("name").and_then(Value::as_str),
                e.get("value").and_then(Value::as_str),
            ) && n.starts_with("ENZARB_")
            {
                out.insert(format!("env/{name}/{n}"), v.into());
            }
        }
        // The container identity and privileges (runtime profile): sandbox-init runs as root.
        if let Some(sc) = c.get("securityContext") {
            for f in ["runAsUser", "privileged", "allowPrivilegeEscalation"] {
                if let Some(v) = sc.get(f) {
                    out.insert(format!("security/{name}/{f}"), v.to_string());
                }
            }
            if let Some(caps) = sc.pointer("/capabilities/add").and_then(Value::as_array) {
                let mut caps: Vec<&str> = caps.iter().filter_map(Value::as_str).collect();
                caps.sort_unstable();
                out.insert(format!("security/{name}/caps"), caps.join(","));
            }
        }
        for kind in ["limits", "requests"] {
            let res = c.pointer(&format!("/resources/{kind}"));
            for (r, q) in res.and_then(Value::as_object).into_iter().flatten() {
                out.insert(
                    format!("resources/{name}/{kind}/{r}"),
                    canonical_quantity(q),
                );
            }
        }
        for m in c
            .get("volumeMounts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let (Some(n), Some(path)) = (
                m.get("name").and_then(Value::as_str),
                m.get("mountPath").and_then(Value::as_str),
            ) {
                out.insert(format!("mounts/{name}/{n}"), path.into());
            }
        }
    }
    out
}

/// The differences between the desired shape and the running pod's, sorted. Keys the desired
/// shape does not set are skipped (API-server defaults and injected fields never count), except
/// that an `ENZARB_*` env var the pod carries and the operator no longer sets does. Empty means
/// the pod is current.
pub fn diff(want: &PodShape, have: &PodShape) -> Vec<String> {
    let mut out = Vec::new();
    for (k, w) in want {
        match have.get(k) {
            Some(h) if h == w => {}
            Some(h) => out.push(format!("{k}: {h} -> {w}")),
            None => out.push(format!("{k}: <unset> -> {w}")),
        }
    }
    for (k, h) in have {
        if k.starts_with("env/") && !want.contains_key(k) {
            out.push(format!("{k}: {h} -> <unset>"));
        }
    }
    out
}

fn reason_of(diff: &[String]) -> String {
    let mut r = diff.join("; ");
    if r.len() > MAX_REASON {
        let mut end = MAX_REASON;
        while !r.is_char_boundary(end) {
            end -= 1;
        }
        r.truncate(end);
    }
    r
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PodState {
    Absent,
    Terminating,
    Running { created: Timestamp },
}

#[derive(Debug)]
pub struct Input<'a> {
    /// Output of [`diff`] for the running pod.
    pub diff: &'a [String],
    /// The Sandbox returned by the apply already carries the desired shape.
    pub template_matches: bool,
    pub pod: PodState,
    pub activity: Activity,
    pub pending: Option<&'a PendingChange>,
    /// Parsed `enzarb.dev/drained-generation`; `None` when absent or not an integer.
    pub ack: Option<i64>,
    /// Parsed `enzarb.dev/roll-now-generation`; `None` when absent or not an integer.
    pub roll_now: Option<i64>,
    pub now: Timestamp,
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Ack,
    Manual,
    Idle,
    Timeout,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Nothing to apply: clear any pending change.
    Settled,
    /// Leave status as it is and do nothing (pod absent or terminating, apply not landed, or a
    /// persistent diff after the pod was already recreated).
    Hold,
    /// Publish this pending change; do not delete.
    Defer(PendingChange),
    /// Publish this pending change and delete the pod.
    Roll(PendingChange, Why),
}

pub fn decide(i: &Input) -> Decision {
    let PodState::Running { created } = i.pod else {
        return if i.pending.is_some() {
            Decision::Hold
        } else {
            Decision::Settled
        };
    };
    if i.diff.is_empty() {
        return Decision::Settled;
    }
    if !i.template_matches {
        // The recreated pod would come back stale; wait for the apply to land.
        return Decision::Hold;
    }
    let reason = reason_of(i.diff);
    // i64::MAX cannot be exceeded by a new generation, so it is never a valid ack.
    let ack = i.ack.filter(|a| *a < i64::MAX);
    // Generations only ever exceed any ack already on the Project, so a stale ack left from an
    // earlier episode can never release a new one.
    let roll_now = i.roll_now.filter(|a| *a < i64::MAX);
    let floor = ack.max(roll_now).map_or(1, |a| a.saturating_add(1).max(1));
    let next = match i.pending {
        None => fresh(floor, i.now, i.timeout, reason),
        // The pod was recreated after this episode began: it already had its roll.
        Some(p) if created > p.since.0 => {
            if p.reason == reason {
                return Decision::Hold;
            }
            fresh(
                floor.max(p.generation.saturating_add(1)),
                i.now,
                i.timeout,
                reason,
            )
        }
        Some(p) if p.reason == reason => PendingChange {
            deadline: Some(Time(deadline(p.since.0, i.timeout))),
            ..p.clone()
        },
        // A newer change: new generation, the drain clock keeps running from the first one.
        Some(p) => PendingChange {
            generation: floor.max(p.generation.saturating_add(1)),
            since: p.since.clone(),
            deadline: Some(Time(deadline(p.since.0, i.timeout))),
            reason,
        },
    };
    let why = if ack == Some(next.generation) {
        Some(Why::Ack)
    } else if roll_now == Some(next.generation) {
        Some(Why::Manual)
    } else if i.activity == Activity::Active(0) {
        Some(Why::Idle)
    } else if i.now.as_second() >= deadline(next.since.0, i.timeout).as_second() {
        Some(Why::Timeout)
    } else {
        None
    };
    let age = i.now.as_second() - created.as_second();
    match why {
        Some(w) if age >= MIN_POD_AGE_SECS => Decision::Roll(next, w),
        _ => Decision::Defer(next),
    }
}

fn fresh(generation: i64, now: Timestamp, timeout: Duration, reason: String) -> PendingChange {
    PendingChange {
        generation,
        since: Time(now),
        deadline: Some(Time(deadline(now, timeout))),
        reason,
    }
}

pub fn deadline(since: Timestamp, timeout: Duration) -> Timestamp {
    let secs = since
        .as_second()
        .saturating_add(i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX));
    Timestamp::from_second(secs.min(Timestamp::MAX.as_second())).unwrap_or(Timestamp::MAX)
}

fn message(c: &PendingChange, activity: Activity, due: Timestamp) -> String {
    let busy = match activity {
        Activity::Active(n) if n > 0 => format!(" ({n} active sessions)"),
        _ => String::new(),
    };
    format!(
        "generation {}: {}; applied when drained (annotation {ANNOTATION_DRAINED_GENERATION}={g}), on roll now (annotation {ANNOTATION_ROLL_NOW_GENERATION}={g}) or after {due}{busy}",
        c.generation,
        c.reason,
        g = c.generation
    )
}

fn spec_value<T: serde::Serialize>(t: &T) -> Value {
    serde_json::to_value(t).unwrap_or(Value::Null)
}

/// What the step concluded for status.
#[derive(Debug)]
pub enum Outcome {
    /// Nothing pending: remove `status.pendingChange` and the condition.
    Clear,
    /// Leave both as they are.
    Keep,
    Pending(RollPending),
}

/// Compares the running pod with the desired shape and deletes the pod when released. Only ever
/// deletes the Pod.
pub async fn roll_step(
    client: &Client,
    p: &Project,
    d: &Defaults,
    sb: &Sandbox,
    pending: Option<&PendingChange>,
    timeout: Duration,
    now: &Time,
) -> Result<Outcome, kube::Error> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), &d.sandbox_namespace);
    let name = p.sandbox_name();
    // Same name as the Sandbox (agent-sandbox).
    let pod = pods.get_opt(&name).await?;
    let state = match &pod {
        None => PodState::Absent,
        Some(pod) if pod.metadata.deletion_timestamp.is_some() => PodState::Terminating,
        Some(pod) => match pod.metadata.creation_timestamp.as_ref() {
            Some(t) => PodState::Running { created: t.0 },
            // Without a creation time we cannot reason about age: do nothing.
            None => PodState::Terminating,
        },
    };
    let want = shape_of(&spec_value(&desired::sandbox(p, d).spec.pod_template.spec));
    let template = shape_of(&spec_value(&sb.spec.pod_template.spec));
    let have = pod
        .as_ref()
        .and_then(|pod| pod.spec.as_ref())
        .map(|s| shape_of(&spec_value(s)))
        .unwrap_or_default();
    let diff = diff(&want, &have);
    let activity = pod.as_ref().map_or(Activity::Unknown, Activity::of_pod);
    let ack = p
        .annotations()
        .get(ANNOTATION_DRAINED_GENERATION)
        .and_then(|v| v.trim().parse::<i64>().ok());
    let roll_now = p
        .annotations()
        .get(ANNOTATION_ROLL_NOW_GENERATION)
        .and_then(|v| v.trim().parse::<i64>().ok());
    let input = Input {
        diff: &diff,
        template_matches: self::diff(&want, &template).is_empty(),
        pod: state,
        activity,
        pending,
        ack,
        roll_now,
        now: now.0,
        timeout,
    };
    let report = |c: PendingChange| {
        let due = deadline(c.since.0, timeout);
        RollPending {
            message: message(&c, activity, due),
            remaining: Duration::from_secs(
                u64::try_from(due.as_second() - now.0.as_second()).unwrap_or(0),
            ),
            change: c,
        }
    };
    match decide(&input) {
        Decision::Settled => Ok(Outcome::Clear),
        Decision::Hold => Ok(Outcome::Keep),
        Decision::Defer(c) => Ok(Outcome::Pending(report(c))),
        Decision::Roll(c, why) => {
            let uid = pod.as_ref().and_then(|p| p.metadata.uid.clone());
            let dp = DeleteParams {
                preconditions: Some(Preconditions {
                    uid,
                    resource_version: None,
                }),
                ..Default::default()
            };
            match pods.delete(&name, &dp).await {
                Ok(_) => {
                    info!(project = %p.name_any(), generation = c.generation, ?why, reason = %c.reason, "rolled sandbox pod");
                    if why == Why::Timeout {
                        warn!(project = %p.name_any(), "drain timeout expired; applied without a drain ack");
                    }
                }
                // Already replaced or gone: nothing to roll.
                Err(kube::Error::Api(s)) if s.code == 404 || s.code == 409 => {
                    warn!(project = %p.name_any(), code = s.code, "sandbox pod changed before the roll");
                }
                Err(e) => return Err(e),
            }
            // Pending stays published until the recreated pod matches.
            Ok(Outcome::Pending(report(c)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ts(s: i64) -> Timestamp {
        Timestamp::from_second(s).unwrap()
    }

    const T: Duration = Duration::from_secs(7200);
    const D: &[&str] = &["image/sandbox: a -> b"];

    fn pc(generation: i64, since: i64, reason: &str) -> PendingChange {
        PendingChange {
            generation,
            since: Time(ts(since)),
            deadline: Some(Time(ts(since + 7200))),
            reason: reason.into(),
        }
    }

    fn base<'a>(diff: &'a [String], pending: Option<&'a PendingChange>) -> Input<'a> {
        Input {
            diff,
            template_matches: true,
            pod: PodState::Running { created: ts(0) },
            activity: Activity::Unknown,
            pending,
            ack: None,
            roll_now: None,
            now: ts(1000),
            timeout: T,
        }
    }

    fn diff_of(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn settled_without_diff_or_pod() {
        assert_eq!(decide(&base(&[], None)), Decision::Settled);
        // Pending is cleared once the pod matches.
        let p = pc(1, 5, "r");
        assert_eq!(decide(&base(&[], Some(&p))), Decision::Settled);
        let mut i = base(&[], None);
        i.pod = PodState::Absent;
        assert_eq!(decide(&i), Decision::Settled);
    }

    #[test]
    fn pod_absent_or_terminating_holds_pending() {
        let d = diff_of(D);
        let p = pc(1, 5, "image/sandbox: a -> b");
        for pod in [PodState::Absent, PodState::Terminating] {
            let mut i = base(&d, Some(&p));
            i.pod = pod;
            assert_eq!(decide(&i), Decision::Hold);
            let mut i = base(&d, None);
            i.pod = pod;
            assert_eq!(decide(&i), Decision::Settled);
        }
    }

    #[test]
    fn stale_template_holds() {
        let d = diff_of(D);
        let mut i = base(&d, None);
        i.template_matches = false;
        i.activity = Activity::Active(0);
        assert_eq!(decide(&i), Decision::Hold);
    }

    #[test]
    fn first_mismatch_opens_generation_one_and_defers() {
        let d = diff_of(D);
        assert_eq!(
            decide(&base(&d, None)),
            Decision::Defer(pc(1, 1000, "image/sandbox: a -> b"))
        );
    }

    #[test]
    fn same_target_keeps_pending_unchanged() {
        let d = diff_of(D);
        let p = pc(3, 500, "image/sandbox: a -> b");
        assert_eq!(decide(&base(&d, Some(&p))), Decision::Defer(p.clone()));
        // Idempotent.
        assert_eq!(decide(&base(&d, Some(&p))), decide(&base(&d, Some(&p))));
    }

    #[test]
    fn deadline_is_since_plus_timeout_and_survives_a_newer_change() {
        let d = diff_of(D);
        let Decision::Defer(c) = decide(&base(&d, None)) else {
            panic!("expected defer")
        };
        assert_eq!(c.deadline, Some(Time(ts(1000 + 7200))));
        let p = pc(1, 500, "image/sandbox: x -> y");
        let Decision::Defer(n) = decide(&base(&d, Some(&p))) else {
            panic!("expected defer")
        };
        assert_eq!(n.since, p.since);
        assert_eq!(n.deadline, Some(Time(ts(500 + 7200))));
        // An object from an older operator gains the deadline.
        let mut old = pc(1, 500, "image/sandbox: a -> b");
        old.deadline = None;
        assert_eq!(
            decide(&base(&d, Some(&old))),
            Decision::Defer(pc(1, 500, "image/sandbox: a -> b"))
        );
    }

    #[test]
    fn idle_sandbox_rolls_at_once_once_old_enough() {
        let d = diff_of(D);
        let mut i = base(&d, None);
        i.activity = Activity::Active(0);
        i.pod = PodState::Running {
            created: ts(1000 - MIN_POD_AGE_SECS),
        };
        assert_eq!(
            decide(&i),
            Decision::Roll(pc(1, 1000, "image/sandbox: a -> b"), Why::Idle)
        );
    }

    #[test]
    fn idle_young_pod_defers_until_min_age() {
        let d = diff_of(D);
        let mut i = base(&d, None);
        i.activity = Activity::Active(0);
        i.pod = PodState::Running {
            created: ts(1000 - MIN_POD_AGE_SECS + 1),
        };
        assert!(matches!(decide(&i), Decision::Defer(_)));
        let p = pc(1, 1000, "image/sandbox: a -> b");
        i.pending = Some(&p);
        i.now = ts(1001);
        assert!(matches!(decide(&i), Decision::Roll(_, Why::Idle)));
    }

    #[test]
    fn busy_or_unknown_waits_for_the_deadline() {
        let d = diff_of(D);
        let p = pc(1, 500, "image/sandbox: a -> b");
        for a in [Activity::Active(2), Activity::Unknown] {
            let mut i = base(&d, Some(&p));
            i.activity = a;
            assert!(matches!(decide(&i), Decision::Defer(_)));
            i.now = ts(500 + 7200);
            assert!(matches!(decide(&i), Decision::Roll(_, Why::Timeout)));
        }
    }

    #[test]
    fn busy_sessions_do_not_release() {
        let d = diff_of(D);
        let mut i = base(&d, None);
        i.activity = Activity::Active(3);
        assert!(matches!(decide(&i), Decision::Defer(_)));
    }

    #[test]
    fn ack_for_the_pending_generation_releases() {
        let d = diff_of(D);
        let p = pc(2, 500, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.ack = Some(2);
        assert_eq!(decide(&i), Decision::Roll(p.clone(), Why::Ack));
    }

    #[test]
    fn stale_future_and_garbage_acks_are_ignored() {
        let d = diff_of(D);
        let p = pc(2, 500, "image/sandbox: a -> b");
        for ack in [Some(1), Some(3), Some(0), Some(-2), Some(i64::MAX), None] {
            let mut i = base(&d, Some(&p));
            i.ack = ack;
            assert_eq!(decide(&i), Decision::Defer(p.clone()), "{ack:?}");
        }
    }

    #[test]
    fn roll_now_for_the_pending_generation_releases() {
        let d = diff_of(D);
        let p = pc(2, 500, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.roll_now = Some(2);
        assert_eq!(decide(&i), Decision::Roll(p.clone(), Why::Manual));
        // An ack wins when both match.
        i.ack = Some(2);
        assert_eq!(decide(&i), Decision::Roll(p, Why::Ack));
    }

    #[test]
    fn stale_future_and_garbage_roll_nows_are_ignored() {
        let d = diff_of(D);
        let p = pc(2, 500, "image/sandbox: a -> b");
        for v in [Some(1), Some(3), Some(0), Some(-2), Some(i64::MAX), None] {
            let mut i = base(&d, Some(&p));
            i.roll_now = v;
            assert_eq!(decide(&i), Decision::Defer(p.clone()), "{v:?}");
        }
    }

    #[test]
    fn roll_now_without_a_change_is_settled() {
        let mut i = base(&[], None);
        i.roll_now = Some(1);
        assert_eq!(decide(&i), Decision::Settled);
    }

    #[test]
    fn leftover_roll_now_never_releases_a_new_episode() {
        let d = diff_of(D);
        let mut i = base(&d, None);
        i.roll_now = Some(1);
        assert_eq!(
            decide(&i),
            Decision::Defer(pc(2, 1000, "image/sandbox: a -> b"))
        );
        i.ack = Some(4);
        i.roll_now = Some(7);
        assert_eq!(
            decide(&i),
            Decision::Defer(pc(8, 1000, "image/sandbox: a -> b"))
        );
    }

    #[test]
    fn newer_change_invalidates_old_roll_now() {
        let d = diff_of(&["image/sandbox: a -> c"]);
        let p = pc(1, 500, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.roll_now = Some(1);
        assert_eq!(
            decide(&i),
            Decision::Defer(pc(2, 500, "image/sandbox: a -> c"))
        );
    }

    #[test]
    fn young_pod_is_not_rolled_by_roll_now() {
        let d = diff_of(D);
        let p = pc(1, 1000, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.roll_now = Some(1);
        i.pod = PodState::Running { created: ts(950) };
        assert!(matches!(decide(&i), Decision::Defer(_)));
    }

    #[test]
    fn leftover_ack_never_releases_a_new_episode() {
        let d = diff_of(D);
        // A previous episode was acked with 1; a new episode must not reuse generation 1.
        let mut i = base(&d, None);
        i.ack = Some(1);
        assert_eq!(
            decide(&i),
            Decision::Defer(pc(2, 1000, "image/sandbox: a -> b"))
        );
        i.ack = Some(i64::MAX);
        assert_eq!(
            decide(&i),
            Decision::Defer(pc(1, 1000, "image/sandbox: a -> b"))
        );
    }

    #[test]
    fn newer_change_bumps_generation_keeps_since_and_invalidates_old_ack() {
        let d = diff_of(&["image/sandbox: a -> c"]);
        let p = pc(1, 500, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.ack = Some(1);
        assert_eq!(
            decide(&i),
            Decision::Defer(pc(2, 500, "image/sandbox: a -> c"))
        );
        // The stored generation 2 is what workflows reads and acks.
        let p2 = pc(2, 500, "image/sandbox: a -> c");
        let mut j = base(&d, Some(&p2));
        j.ack = Some(1);
        assert_eq!(decide(&j), Decision::Defer(p2.clone()));
        j.ack = Some(2);
        assert_eq!(decide(&j), Decision::Roll(p2, Why::Ack));
    }

    #[test]
    fn timeout_releases_even_when_busy_measured_from_first_change() {
        let d = diff_of(D);
        let p = pc(1, 1000, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.activity = Activity::Active(5);
        i.now = ts(1000 + 7199);
        assert!(matches!(decide(&i), Decision::Defer(_)));
        i.now = ts(1000 + 7200);
        assert_eq!(decide(&i), Decision::Roll(p, Why::Timeout));
        // A later change keeps the original clock.
        let newer = diff_of(&["image/sandbox: a -> c"]);
        let p = pc(1, 1000, "image/sandbox: a -> b");
        let mut i = base(&newer, Some(&p));
        i.now = ts(1000 + 7200);
        assert!(matches!(decide(&i), Decision::Roll(c, Why::Timeout) if c.generation == 2));
    }

    #[test]
    fn idle_sandbox_releases() {
        let d = diff_of(D);
        let mut i = base(&d, None);
        i.activity = Activity::Active(0);
        assert!(matches!(decide(&i), Decision::Roll(_, Why::Idle)));
    }

    #[test]
    fn young_pod_is_never_deleted() {
        let d = diff_of(D);
        let p = pc(1, 1000, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.ack = Some(1);
        i.pod = PodState::Running { created: ts(950) };
        assert!(matches!(decide(&i), Decision::Defer(_)));
        i.pod = PodState::Running { created: ts(880) };
        assert!(matches!(decide(&i), Decision::Roll(..)));
    }

    #[test]
    fn recreated_pod_with_persistent_diff_is_not_rolled_again() {
        let d = diff_of(D);
        let p = pc(1, 500, "image/sandbox: a -> b");
        let mut i = base(&d, Some(&p));
        i.ack = Some(1);
        i.pod = PodState::Running { created: ts(600) };
        assert_eq!(decide(&i), Decision::Hold);
        // A genuinely new change on that pod starts a fresh episode and clock.
        let newer = diff_of(&["image/sandbox: a -> c"]);
        let mut j = base(&newer, Some(&p));
        j.pod = PodState::Running { created: ts(600) };
        assert_eq!(
            decide(&j),
            Decision::Defer(pc(2, 1000, "image/sandbox: a -> c"))
        );
    }

    #[test]
    fn long_reason_is_truncated_on_a_char_boundary() {
        let d = vec!["é".repeat(2000)];
        let Decision::Defer(c) = decide(&base(&d, None)) else {
            panic!()
        };
        assert!(c.reason.len() <= MAX_REASON);
    }

    fn spec(image: &str, env: &[(&str, &str)], res: Value, rc: &str) -> Value {
        let env: Vec<Value> = env
            .iter()
            .map(|(n, v)| json!({"name": n, "value": v}))
            .collect();
        json!({
            "runtimeClassName": rc,
            "containers": [
                {"name": "sandbox", "image": image, "env": env, "resources": res,
                 "securityContext": {"runAsUser": 0, "privileged": false,
                                     "capabilities": {"add": ["SYS_ADMIN", "CHOWN"]}},
                 "volumeMounts": [{"name": "workspace", "mountPath": "/w"},
                                  {"name": "docker-data", "mountPath": "/var/lib/docker"}]},
            ],
        })
    }

    #[test]
    fn each_pod_shape_field_produces_a_diff() {
        let base_spec = spec("i:1", &[("ENZARB_MAX_RUNS", "2")], json!({}), "gvisor");
        let have = shape_of(&base_spec);
        let cases = [
            spec("i:2", &[("ENZARB_MAX_RUNS", "2")], json!({}), "gvisor"),
            spec("i:1", &[("ENZARB_MAX_RUNS", "3")], json!({}), "gvisor"),
            spec(
                "i:1",
                &[("ENZARB_MAX_RUNS", "2")],
                json!({"limits": {"cpu": "2"}}),
                "gvisor",
            ),
            spec("i:1", &[("ENZARB_MAX_RUNS", "2")], json!({}), "kata"),
            spec(
                "i:1",
                &[("ENZARB_MAX_RUNS", "2"), ("ENZARB_NEW", "1")],
                json!({}),
                "gvisor",
            ),
        ];
        for c in cases {
            assert!(!diff(&shape_of(&c), &have).is_empty(), "{c}");
        }
        // The container's privileges count too (runtime profile changes).
        let mut c = base_spec.clone();
        c["containers"][0]["securityContext"]["privileged"] = json!(true);
        assert_eq!(diff(&shape_of(&c), &have).len(), 1);
        let mut c = base_spec.clone();
        c["containers"][0]["securityContext"]["runAsUser"] = json!(1000);
        assert_eq!(diff(&shape_of(&c), &have).len(), 1);
        let mut c = base_spec.clone();
        c["containers"][0]["securityContext"]["capabilities"]["add"] = json!(["CHOWN"]);
        assert_eq!(diff(&shape_of(&c), &have).len(), 1);
        // Capability order is irrelevant.
        let mut c = base_spec.clone();
        c["containers"][0]["securityContext"]["capabilities"]["add"] =
            json!(["CHOWN", "SYS_ADMIN"]);
        assert!(diff(&shape_of(&c), &have).is_empty());
        let mut c = base_spec.clone();
        c["containers"][0]["volumeMounts"][0]["mountPath"] = json!("/x");
        assert_eq!(diff(&shape_of(&c), &have).len(), 1);
        assert!(diff(&shape_of(&base_spec), &have).is_empty());
    }

    /// A pod created with the old layout (dockerd sidecar + shared `docker-sock`, ADR-0012) is
    /// stale against the new desired shape, so existing sandboxes drain and roll once.
    #[test]
    fn pod_with_the_old_dockerd_sidecar_layout_is_stale() {
        let old = json!({
            "runtimeClassName": "gvisor",
            "containers": [
                {"name": "sandbox", "image": "i:1",
                 "env": [{"name": "ENZARB_MAX_RUNS", "value": "2"},
                         {"name": "DOCKER_HOST", "value": "unix:///var/run/docker/docker.sock"}],
                 "volumeMounts": [{"name": "workspace", "mountPath": "/w"},
                                  {"name": "docker-sock", "mountPath": "/var/run/docker"}]},
                {"name": "dockerd", "image": "docker:29.8.2-dind",
                 "volumeMounts": [{"name": "docker-sock", "mountPath": "/var/run/docker"},
                                  {"name": "docker-data", "mountPath": "/var/lib/docker"}]},
            ],
        });
        let new = spec("i:1", &[("ENZARB_MAX_RUNS", "2")], json!({}), "gvisor");
        let d = diff(&shape_of(&new), &shape_of(&old));
        assert!(
            d.iter()
                .any(|l| l.starts_with("mounts/sandbox/docker-data")),
            "{d:?}"
        );
        assert!(
            d.iter()
                .any(|l| l.starts_with("security/sandbox/runAsUser")),
            "{d:?}"
        );
    }

    #[test]
    fn equivalent_quantities_do_not_differ() {
        let a = spec(
            "i",
            &[],
            json!({"limits": {"memory": "1Gi", "cpu": "500m"}}),
            "g",
        );
        let b = spec(
            "i",
            &[],
            json!({"limits": {"memory": "1024Mi", "cpu": "0.5"}}),
            "g",
        );
        assert!(diff(&shape_of(&a), &shape_of(&b)).is_empty());
        let c = spec(
            "i",
            &[],
            json!({"limits": {"memory": "2Gi", "cpu": "500m"}}),
            "g",
        );
        assert_eq!(diff(&shape_of(&c), &shape_of(&b)).len(), 1);
    }

    #[test]
    fn removed_enzarb_env_is_a_diff_but_other_injected_fields_are_not() {
        let have = spec(
            "i",
            &[
                ("ENZARB_MAX_RUNS", "2"),
                ("INJECTED", "1"),
                ("ENZARB_PROJECT", "p"),
            ],
            json!({"requests": {"cpu": "1"}, "limits": {"cpu": "2"}}),
            "g",
        );
        let mut have = have;
        have["containers"][0]["volumeMounts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "kube", "mountPath": "/k"}));
        let want = spec("i", &[("ENZARB_PROJECT", "p")], json!({}), "g");
        let d = diff(&shape_of(&want), &shape_of(&have));
        assert_eq!(
            d,
            vec!["env/sandbox/ENZARB_MAX_RUNS: 2 -> <unset>".to_string()]
        );
        // Unset resources in the template never differ from defaulted ones on the pod.
        let want2 = spec(
            "i",
            &[("ENZARB_PROJECT", "p"), ("ENZARB_MAX_RUNS", "2")],
            json!({}),
            "g",
        );
        assert!(diff(&shape_of(&want2), &shape_of(&have)).is_empty());
    }

    #[test]
    fn desired_template_with_no_new_fields_matches_a_pod_built_from_it() {
        // Operator upgrade: a pod rendered from today's template, plus injected extras, is current.
        let mut p = crate::desired::tests::project("a", "o");
        p.spec.max_runs = Some(5);
        let sb = desired::sandbox(&p, &crate::desired::tests::defaults());
        let mut pod = spec_value(&sb.spec.pod_template.spec);
        pod["containers"][0]["env"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name": "INJECTED", "value": "1"}));
        pod["containers"][0]["resources"] = json!({"requests": {"cpu": "1"}});
        assert!(
            diff(
                &shape_of(&spec_value(&sb.spec.pod_template.spec)),
                &shape_of(&pod)
            )
            .is_empty()
        );
        p.spec.max_runs = None;
        let sb2 = desired::sandbox(&p, &crate::desired::tests::defaults());
        assert!(
            !diff(
                &shape_of(&spec_value(&sb2.spec.pod_template.spec)),
                &shape_of(&pod)
            )
            .is_empty()
        );
    }

    #[test]
    fn activity_from_annotation() {
        let annotate = |v: &str| {
            let mut pod = Pod::default();
            pod.metadata.annotations =
                Some([(ANNOTATION_ACTIVE_SESSIONS.to_string(), v.to_string())].into());
            pod
        };
        assert_eq!(Activity::of_pod(&Pod::default()), Activity::Unknown);
        assert_eq!(Activity::of_pod(&annotate("0")), Activity::Active(0));
        assert_eq!(Activity::of_pod(&annotate("x")), Activity::Unknown);
    }
}
