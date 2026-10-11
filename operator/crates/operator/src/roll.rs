//! Rolling a Project's sandbox pod when its image no longer matches the desired image.
//!
//! The Sandbox template is updated by server-side apply, but agent-sandbox does not recreate a
//! running pod for it. The operator deletes the pod (never the Sandbox or the PVCs) so the
//! Sandbox controller recreates it from the current template. `decide` is pure; `roll_step`
//! does the API calls.

use crate::conditions::RollPending;
use crate::desired::{self, Defaults};
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{COND_SANDBOX_IMAGE_ROLL, Project};
use k8s_openapi::api::core::v1::{Container, EnvVar, Pod};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use k8s_openapi::jiff::Timestamp;
use kube::api::{Api, DeleteParams, Preconditions};
use kube::{Client, ResourceExt};
use std::time::Duration;
use tracing::{info, warn};

/// Pod annotation reporting the number of active agent sessions/connections. Written by the
/// sandbox runtime (not yet by anything in this repo). Absent or unparsable means unknown.
pub const ANNOTATION_ACTIVE_SESSIONS: &str = "enzarb.dev/active-sessions";
/// Name of the main container in the Sandbox pod template (see `desired::sandbox`).
const MAIN_CONTAINER: &str = "sandbox";
/// Upper bound on the requeue delay while a roll is deferred.
pub const DEFER_POLL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// The sandbox does not report activity: only the drain timeout releases the roll.
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

#[derive(Debug)]
pub struct Input<'a> {
    pub desired: &'a str,
    /// Image in the Sandbox object returned by the apply.
    pub template: Option<&'a str>,
    /// Image of the running pod's main container; `None` when there is no pod.
    pub pod: Option<&'a str>,
    pub activity: Activity,
    /// When this mismatch was first observed, if before now.
    pub since: Option<Timestamp>,
    pub now: Timestamp,
    pub timeout: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Noop,
    Defer { since: Timestamp },
    Roll { forced: bool },
}

pub fn decide(i: &Input) -> Decision {
    let Some(pod) = i.pod else {
        // Created from the current template.
        return Decision::Noop;
    };
    if pod == i.desired {
        return Decision::Noop;
    }
    if i.template != Some(i.desired) {
        // The recreated pod would come back stale; wait for the apply to land.
        return Decision::Noop;
    }
    if i.activity == Activity::Active(0) {
        return Decision::Roll { forced: false };
    }
    let since = i.since.unwrap_or(i.now);
    if i.now.as_second() >= deadline(since, i.timeout).as_second() {
        Decision::Roll { forced: true }
    } else {
        Decision::Defer { since }
    }
}

pub fn deadline(since: Timestamp, timeout: Duration) -> Timestamp {
    let secs = since
        .as_second()
        .saturating_add(i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX));
    Timestamp::from_second(secs.min(Timestamp::MAX.as_second())).unwrap_or(Timestamp::MAX)
}

/// What a running pod must match to be current: the main container image plus the env the
/// operator sets conditionally (`ENZARB_MAX_RUNS`). Other env entries are not compared, since
/// the live pod may carry entries injected by others. Rendered as `image` when the env var is
/// absent and `image [ENZARB_MAX_RUNS=n]` otherwise.
pub fn revision(image: &str, env: &[EnvVar]) -> String {
    match env.iter().find(|e| e.name == desired::ENV_MAX_RUNS) {
        Some(e) => format!(
            "{image} [{}={}]",
            desired::ENV_MAX_RUNS,
            e.value.as_deref().unwrap_or_default()
        ),
        None => image.to_string(),
    }
}

fn container_revision(c: &Container) -> Option<String> {
    Some(revision(
        c.image.as_deref()?,
        c.env.as_deref().unwrap_or_default(),
    ))
}

/// Revision of the main container of a pod.
pub fn pod_revision(pod: &Pod) -> Option<String> {
    container_revision(
        pod.spec
            .as_ref()?
            .containers
            .iter()
            .find(|c| c.name == MAIN_CONTAINER)?,
    )
}

/// Revision of the main container in the Sandbox pod template.
pub fn template_revision(sb: &Sandbox) -> Option<String> {
    let c = sb
        .spec
        .pod_template
        .spec
        .containers
        .iter()
        .find(|c| c.name == MAIN_CONTAINER)?;
    let env: Vec<EnvVar> = c
        .env
        .iter()
        .flatten()
        .map(|e| EnvVar {
            name: e.name.clone(),
            value: e.value.clone(),
            ..Default::default()
        })
        .collect();
    Some(revision(c.image.as_deref()?, &env))
}

/// The message of the roll condition. It starts with the revision transition, which identifies
/// the mismatch: a changed pod image or env starts a new drain period.
pub fn message_prefix(old: &str, new: &str) -> String {
    format!("revision {old} -> {new}; ")
}

pub fn message(old: &str, new: &str, deadline: Timestamp) -> String {
    format!(
        "{}the pod is recreated when the sandbox is idle or after {deadline}",
        message_prefix(old, new)
    )
}

/// When the roll for this image transition was first seen pending, from the previous conditions.
fn pending_since(existing: &[Condition], old: &str, new: &str) -> Option<Timestamp> {
    existing
        .iter()
        .find(|c| {
            c.type_ == COND_SANDBOX_IMAGE_ROLL
                && c.status == "True"
                && c.message.starts_with(&message_prefix(old, new))
        })
        .map(|c| c.last_transition_time.0)
}

/// Compares the running pod with the desired image and deletes it when it is time. Returns the
/// pending state to report while the roll is waiting. Only ever deletes the Pod.
pub async fn roll_step(
    client: &Client,
    p: &Project,
    d: &Defaults,
    sb: &Sandbox,
    existing: &[Condition],
    timeout: Duration,
    now: &Time,
) -> Result<Option<RollPending>, kube::Error> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), &d.sandbox_namespace);
    let name = p.sandbox_name();
    // Same name as the Sandbox (agent-sandbox).
    let Some(pod) = pods.get_opt(&name).await? else {
        return Ok(None);
    };
    if pod.metadata.deletion_timestamp.is_some() {
        return Ok(None);
    }
    let Some(want) = template_revision(&desired::sandbox(p, d)) else {
        return Ok(None);
    };
    let template = template_revision(sb);
    let Some(old) = pod_revision(&pod) else {
        return Ok(None);
    };
    let old = old.as_str();
    let since = pending_since(existing, old, &want);
    let input = Input {
        desired: &want,
        template: template.as_deref(),
        pod: Some(old),
        activity: Activity::of_pod(&pod),
        since,
        now: now.0,
        timeout,
    };
    match decide(&input) {
        Decision::Noop => Ok(None),
        Decision::Defer { since } => {
            let due = deadline(since, timeout);
            Ok(Some(RollPending {
                message: message(old, &want, due),
                remaining: Duration::from_secs(
                    u64::try_from(due.as_second() - now.0.as_second()).unwrap_or(0),
                ),
            }))
        }
        Decision::Roll { forced } => {
            let dp = DeleteParams {
                preconditions: Some(Preconditions {
                    uid: pod.metadata.uid.clone(),
                    resource_version: None,
                }),
                ..Default::default()
            };
            match pods.delete(&name, &dp).await {
                Ok(_) => {
                    info!(project = %p.name_any(), from = old, to = %want, forced, "rolled sandbox pod");
                    Ok(None)
                }
                // Already replaced or gone: nothing to roll.
                Err(kube::Error::Api(s)) if s.code == 404 || s.code == 409 => {
                    warn!(project = %p.name_any(), code = s.code, "sandbox pod changed before the roll");
                    Ok(None)
                }
                Err(e) => Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: i64) -> Timestamp {
        Timestamp::from_second(s).unwrap()
    }

    const T: Duration = Duration::from_secs(600);

    fn input(
        pod: Option<&'static str>,
        activity: Activity,
        since: Option<i64>,
        now: i64,
    ) -> Input<'static> {
        Input {
            desired: "img:2",
            template: Some("img:2"),
            pod,
            activity,
            since: since.map(ts),
            now: ts(now),
            timeout: T,
        }
    }

    #[test]
    fn noop_when_images_equal() {
        let i = input(Some("img:2"), Activity::Active(0), None, 1000);
        assert_eq!(decide(&i), Decision::Noop);
    }

    #[test]
    fn noop_without_pod() {
        let i = input(None, Activity::Active(0), None, 1000);
        assert_eq!(decide(&i), Decision::Noop);
    }

    #[test]
    fn noop_while_template_is_stale() {
        let mut i = input(Some("img:1"), Activity::Active(0), None, 1000);
        i.template = Some("img:1");
        assert_eq!(decide(&i), Decision::Noop);
        i.template = None;
        assert_eq!(decide(&i), Decision::Noop);
    }

    #[test]
    fn defers_while_busy() {
        let i = input(Some("img:1"), Activity::Active(2), None, 1000);
        assert_eq!(decide(&i), Decision::Defer { since: ts(1000) });
    }

    #[test]
    fn defers_when_activity_unknown() {
        let i = input(Some("img:1"), Activity::Unknown, Some(1000), 1500);
        assert_eq!(decide(&i), Decision::Defer { since: ts(1000) });
    }

    #[test]
    fn rolls_when_idle() {
        let i = input(Some("img:1"), Activity::Active(0), None, 1000);
        assert_eq!(decide(&i), Decision::Roll { forced: false });
    }

    #[test]
    fn forced_after_timeout_even_when_busy() {
        let busy = |now| input(Some("img:1"), Activity::Active(3), Some(1000), now);
        assert_eq!(decide(&busy(1599)), Decision::Defer { since: ts(1000) });
        assert_eq!(decide(&busy(1600)), Decision::Roll { forced: true });
        let unknown = input(Some("img:1"), Activity::Unknown, Some(1000), 5000);
        assert_eq!(decide(&unknown), Decision::Roll { forced: true });
    }

    #[test]
    fn timer_survives_restart() {
        // After a restart the start comes from the persisted condition, not from `now`.
        let t0 = ts(1000);
        let cond = Condition {
            type_: COND_SANDBOX_IMAGE_ROLL.into(),
            status: "True".into(),
            reason: enzarb_crds::REASON_SANDBOX_IMAGE_ROLL_PENDING.into(),
            message: message("img:1", "img:2", deadline(t0, T)),
            observed_generation: Some(1),
            last_transition_time: Time(t0),
        };
        let since = pending_since(std::slice::from_ref(&cond), "img:1", "img:2");
        assert_eq!(since, Some(t0));
        let mut i = input(Some("img:1"), Activity::Unknown, None, 1700);
        i.since = since;
        assert_eq!(decide(&i), Decision::Roll { forced: true });
        // A different pod image is a new mismatch.
        assert_eq!(pending_since(&[cond], "img:0", "img:2"), None);
    }

    fn env(name: &str, value: &str) -> EnvVar {
        EnvVar {
            name: name.into(),
            value: Some(value.into()),
            ..Default::default()
        }
    }

    #[test]
    fn revision_covers_max_runs_only() {
        let base = vec![env("ENZARB_PROJECT", "a"), env("DOCKER_HOST", "unix:///x")];
        let with = |n: &str| [base.clone(), vec![env("ENZARB_MAX_RUNS", n)]].concat();
        assert_eq!(revision("img:1", &base), "img:1");
        assert_eq!(revision("img:1", &with("3")), "img:1 [ENZARB_MAX_RUNS=3]");
        assert_ne!(revision("img:1", &with("3")), revision("img:1", &with("4")));
        assert_ne!(revision("img:1", &base), revision("img:1", &with("3")));
        // Unrelated entries injected by others do not matter.
        let mut other = base.clone();
        other.push(env("INJECTED", "1"));
        assert_eq!(revision("img:1", &other), revision("img:1", &base));
    }

    #[test]
    fn env_only_change_rolls() {
        let env_of = |n: Option<&str>| {
            n.map(|n| vec![env("ENZARB_MAX_RUNS", n)])
                .unwrap_or_default()
        };
        let cases = [(None, Some("3")), (Some("3"), None), (Some("3"), Some("4"))];
        for (old, new) in cases {
            let (old, new) = (
                revision("img:2", &env_of(old)),
                revision("img:2", &env_of(new)),
            );
            let mk = |activity, since, now| Input {
                desired: &new,
                template: Some(&new),
                pod: Some(&old),
                activity,
                since,
                now: ts(now),
                timeout: T,
            };
            assert_eq!(
                decide(&mk(Activity::Active(0), None, 1000)),
                Decision::Roll { forced: false }
            );
            assert_eq!(
                decide(&mk(Activity::Active(1), None, 1000)),
                Decision::Defer { since: ts(1000) }
            );
            assert_eq!(
                decide(&mk(Activity::Unknown, Some(ts(1000)), 1600)),
                Decision::Roll { forced: true }
            );
        }
        let same = revision("img:2", &env_of(Some("3")));
        let i = Input {
            desired: &same,
            template: Some(&same),
            pod: Some(&same),
            activity: Activity::Active(0),
            since: None,
            now: ts(1000),
            timeout: T,
        };
        assert_eq!(decide(&i), Decision::Noop);
    }

    #[test]
    fn pod_and_template_revisions_match_for_the_same_env() {
        let mut p = crate::desired::tests::project("a", "o");
        p.spec.max_runs = Some(5);
        let sb = desired::sandbox(&p, &crate::desired::tests::defaults());
        let pod = Pod {
            spec: Some(k8s_openapi::api::core::v1::PodSpec {
                containers: serde_json::from_value(
                    serde_json::to_value(&sb.spec.pod_template.spec.containers).unwrap(),
                )
                .unwrap(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(pod_revision(&pod), template_revision(&sb));
        assert!(pod_revision(&pod).unwrap().ends_with("[ENZARB_MAX_RUNS=5]"));
        p.spec.max_runs = None;
        let sb2 = desired::sandbox(&p, &crate::desired::tests::defaults());
        assert_ne!(pod_revision(&pod), template_revision(&sb2));
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
