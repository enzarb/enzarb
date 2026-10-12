//! Applies the desired objects (see `desired`) and records status (see `conditions`).

use crate::conditions::{self, Check, Observed, RollPending};
use crate::desired::{self, Defaults, FIELD_MANAGER, Ownership};
use crate::roll::{self, DEFER_POLL};
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{
    COND_SANDBOX_IMAGE_ROLL, Project, ProjectStatus, REASON_WORKSPACE_RESIZE_FAILED,
    REASON_WORKSPACE_RESIZING,
};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Secret};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::{Api, Patch, PatchParams, PostParams};
use kube::runtime::controller::Action;
use kube::{Client, ResourceExt};
use rand::Rng;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("kubernetes API: {0}")]
    Kube(#[from] kube::Error),
}

pub struct Ctx {
    pub client: Client,
    pub defaults: Defaults,
    /// How long a deferred sandbox pod roll waits for idleness (see `roll`).
    pub roll_drain_timeout: Duration,
    failures: Mutex<HashMap<String, u32>>,
}

impl Ctx {
    pub fn new(client: Client, roll_drain_timeout: Duration, defaults: Defaults) -> Self {
        Self {
            client,
            defaults,
            roll_drain_timeout,
            failures: Mutex::default(),
        }
    }

    fn reset(&self, name: &str) {
        self.failures.lock().unwrap().remove(name);
    }

    fn fail(&self, name: &str) -> u32 {
        let mut m = self.failures.lock().unwrap();
        let n = m.entry(name.to_string()).or_default();
        *n += 1;
        *n
    }
}

/// Exponential backoff: 5s, 10s, 20s ... capped at 5 minutes.
pub fn backoff(failures: u32) -> Duration {
    let exp = failures.saturating_sub(1).min(6);
    Duration::from_secs((5u64 << exp).min(300))
}

const POLL_NOT_READY: Duration = Duration::from_secs(5);
const RESYNC: Duration = Duration::from_secs(300);

pub async fn reconcile(project: Arc<Project>, ctx: Arc<Ctx>) -> Result<Action, Error> {
    let name = project.name_any();
    if project.metadata.deletion_timestamp.is_some() {
        // No finalizer: garbage collection removes everything the Project owns.
        return Ok(Action::await_change());
    }
    let action = apply(&project, &ctx).await?;
    ctx.reset(&name);
    Ok(action)
}

pub fn error_policy(project: Arc<Project>, err: &Error, ctx: Arc<Ctx>) -> Action {
    let name = project.name_any();
    let delay = backoff(ctx.fail(&name));
    warn!(project = %name, error = %err, retry_in = ?delay, "reconcile failed");
    Action::requeue(delay)
}

fn ssa() -> PatchParams {
    PatchParams::apply(FIELD_MANAGER).force()
}

async fn apply(p: &Project, ctx: &Ctx) -> Result<Action, Error> {
    let name = p.name_any();
    let mut obs = Observed::all_skipped();
    let mut status = ProjectStatus {
        observed_generation: p.metadata.generation,
        conditions: p
            .status
            .as_ref()
            .map(|s| s.conditions.clone())
            .unwrap_or_default(),
        ..Default::default()
    };

    if let Err(e) = desired::validate(p, &ctx.defaults) {
        warn!(project = %name, error = %e, "invalid project");
        obs.resources = Check::Invalid(e.to_string());
        write_status(p, ctx, status, &obs, &RollState::Clear).await?;
        // Nothing to retry until the user edits the spec.
        return Ok(Action::await_change());
    }

    let mut roll = RollState::Clear;
    let result = drive(p, ctx, &mut obs, &mut status, &mut roll).await;
    write_status(p, ctx, status, &obs, &roll).await?;
    result?;

    if obs.resources == Check::Ok && obs.sandbox == Check::Ok {
        info!(project = %name, "ready");
        // A deferred roll is re-checked soon, so the drain timeout fires without an event.
        let wait = match &roll {
            RollState::Pending(r) => r.remaining.clamp(Duration::from_secs(1), DEFER_POLL),
            _ => RESYNC,
        };
        Ok(Action::requeue(wait))
    } else if is_not_owned(&obs.resources) || is_not_owned(&obs.sandbox) {
        Ok(Action::requeue(NOT_OWNED_RETRY))
    } else {
        Ok(Action::requeue(POLL_NOT_READY))
    }
}

/// What exists under an object name relative to this Project instance.
#[derive(Debug, PartialEq, Eq)]
enum Existing {
    Absent,
    Mine,
    /// Reported on the condition and requeued; see `blocked`.
    Blocked(Ownership),
}

async fn existing<K>(api: &Api<K>, name: &str, p: &Project) -> Result<Existing, kube::Error>
where
    K: kube::Resource + Clone + std::fmt::Debug + serde::de::DeserializeOwned,
{
    Ok(match api.get_opt(name).await? {
        None => Existing::Absent,
        Some(o) => match desired::ownership(o.meta(), p) {
            Ownership::Mine => Existing::Mine,
            other => Existing::Blocked(other),
        },
    })
}

/// Reason on the condition when an object exists but has no controller ownerReference.
pub const REASON_NOT_OWNED: &str = "NotOwnedError";
/// Requeue delay for a `NotOwnedError`: it needs an admin, so do not poll every 5s.
const NOT_OWNED_RETRY: Duration = Duration::from_secs(60);

/// The check for an object that cannot be reused.
/// - Terminating, or controlled by another Project uid (a predecessor): wait for garbage collection.
/// - No controller ownerReference: terminal failure naming the object; an admin must delete it.
fn blocked(kind: &str, name: &str, why: &Ownership) -> Check {
    match why {
        Ownership::NotOwned => Check::Failure(
            REASON_NOT_OWNED.into(),
            format!(
                "{kind} {name} already exists and is not owned by this Project; \
                 an administrator must delete it"
            ),
        ),
        _ => Check::Pending(
            "Terminating".into(),
            format!(
                "{kind} {name} belongs to a previous Project of this name or is being deleted; \
                 waiting for garbage collection"
            ),
        ),
    }
}

fn is_not_owned(c: &Check) -> bool {
    matches!(c, Check::Failure(r, _) if r == REASON_NOT_OWNED)
}

/// Applies each object in order, recording a check per step. Stops at the first API error.
/// Everything lives in the shared sandbox namespace, named per Project. An existing object is
/// reused only if this Project instance owns it (uid), so a re-created Project never picks up
/// its predecessor's Secret or volumes.
async fn drive(
    p: &Project,
    ctx: &Ctx,
    obs: &mut Observed,
    status: &mut ProjectStatus,
    roll: &mut RollState,
) -> Result<(), Error> {
    let d = &ctx.defaults;
    let ns = d.sandbox_namespace.as_str();
    let res_err = |obs: &mut Observed, what: &str, e: kube::Error| {
        obs.resources = Check::Error(format!("{what}: {e}"));
        e
    };

    // The NetworkPolicy goes first, so nothing is started unprotected.
    let policies: Api<NetworkPolicy> = Api::namespaced(ctx.client.clone(), ns);
    let np = p.network_policy_name();
    if let Existing::Blocked(why) = existing(&policies, &np, p)
        .await
        .map_err(|e| res_err(obs, "NetworkPolicy", e))?
    {
        obs.resources = blocked("NetworkPolicy", &np, &why);
        return Ok(());
    }
    policies
        .patch(&np, &ssa(), &Patch::Apply(&desired::network_policy(p, d)))
        .await
        .map_err(|e| res_err(obs, "NetworkPolicy", e))?;

    // Secret: created once, never touched again (the token must not rotate).
    let secrets: Api<Secret> = Api::namespaced(ctx.client.clone(), ns);
    let sn = p.auth_secret_name();
    match existing(&secrets, &sn, p)
        .await
        .map_err(|e| res_err(obs, "Secret", e))?
    {
        Existing::Blocked(why) => {
            obs.resources = blocked("Secret", &sn, &why);
            return Ok(());
        }
        Existing::Mine => {}
        Existing::Absent => {
            let secret = desired::auth_secret(p, d, &new_token());
            match secrets.create(&PostParams::default(), &secret).await {
                Ok(_) => info!(project = %p.name_any(), "created auth secret"),
                Err(kube::Error::Api(s)) if s.code == 409 => {}
                Err(e) => return Err(res_err(obs, "Secret", e).into()),
            }
        }
    }

    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(ctx.client.clone(), ns);
    let mut resizing = None;
    for (what, pvc) in [
        ("workspace PVC", desired::pvc(p, d)),
        ("docker PVC", desired::docker_pvc(p, d)),
    ] {
        let name = pvc.name_any();
        if let Existing::Blocked(why) = existing(&pvcs, &name, p)
            .await
            .map_err(|e| res_err(obs, what, e))?
        {
            obs.resources = blocked(what, &name, &why);
            return Ok(());
        }
        match pvcs.patch(&name, &ssa(), &Patch::Apply(&pvc)).await {
            Ok(applied) => {
                if resizing.is_none() && pvc_resizing(&applied) {
                    resizing = Some(Check::Pending(
                        REASON_WORKSPACE_RESIZING.into(),
                        format!("{what} {name} is being expanded"),
                    ));
                }
            }
            Err(e) => {
                if let Some(check) = classify_pvc_error(what, &name, &e) {
                    obs.resources = check;
                    return Ok(());
                }
                return Err(res_err(obs, what, e).into());
            }
        }
    }
    obs.resources = resizing.unwrap_or(Check::Ok);

    let sandboxes: Api<Sandbox> = Api::namespaced(ctx.client.clone(), ns);
    let bn = p.sandbox_name();
    let sb_err = |obs: &mut Observed, e: kube::Error| {
        obs.sandbox = Check::Error(e.to_string());
        e
    };
    if let Existing::Blocked(why) = existing(&sandboxes, &bn, p)
        .await
        .map_err(|e| sb_err(obs, e))?
    {
        obs.sandbox = blocked("Sandbox", &bn, &why);
        return Ok(());
    }
    let sb = sandboxes
        .patch(&bn, &ssa(), &Patch::Apply(&desired::sandbox(p, d)))
        .await
        .map_err(|e| sb_err(obs, e))?;
    // Advertise the sandbox only after every ownership check passed, so a Project re-created
    // under the same name never points consumers at its predecessor's Secret and endpoint.
    // Namespace and sandbox are set together.
    status.namespace = Some(ns.to_string());
    status.sandbox = Some(desired::sandbox_status(p, d));
    obs.sandbox = conditions::sandbox_check(Some(&sb));

    // Recreate a pod that still runs an outdated image (the Sandbox template was just updated).
    // Only the Pod is deleted; the Sandbox and PVCs stay, so the data survives.
    let now = Time(k8s_openapi::jiff::Timestamp::now());
    match roll::roll_step(
        &ctx.client,
        p,
        d,
        &sb,
        &status.conditions,
        ctx.roll_drain_timeout,
        &now,
    )
    .await
    {
        Ok(Some(pending)) => *roll = RollState::Pending(pending),
        Ok(None) => {}
        Err(e) => {
            // Keep the pending condition (and its timer) across a transient error.
            *roll = RollState::Keep;
            return Err(e.into());
        }
    }
    Ok(())
}

/// What the pod roll step concluded, for the informational `SandboxImageRoll` condition.
#[derive(Debug)]
enum RollState {
    /// No roll is pending (or it was just performed).
    Clear,
    Pending(RollPending),
    /// The step failed: leave the previous condition untouched.
    Keep,
}

async fn write_status(
    p: &Project,
    ctx: &Ctx,
    mut status: ProjectStatus,
    obs: &Observed,
    roll: &RollState,
) -> Result<(), Error> {
    let now = Time(k8s_openapi::jiff::Timestamp::now());
    let generation = p.metadata.generation.unwrap_or_default();
    let previous = std::mem::take(&mut status.conditions);
    status.conditions = conditions::compute(generation, obs, &previous, &now);
    match roll {
        RollState::Clear => {}
        RollState::Pending(r) => {
            status
                .conditions
                .push(conditions::roll_condition(r, generation, &previous, &now));
        }
        RollState::Keep => {
            if let Some(c) = previous.iter().find(|c| c.type_ == COND_SANDBOX_IMAGE_ROLL) {
                status.conditions.push(c.clone());
            }
        }
    }
    let old = p.status.clone().unwrap_or_default();
    let clear = merge_status(&old, &mut status, obs);
    if old == status {
        return Ok(());
    }
    let mut body = json!(status);
    if clear {
        // A merge patch needs explicit nulls to remove fields.
        body["namespace"] = serde_json::Value::Null;
        body["sandbox"] = serde_json::Value::Null;
    }
    let api: Api<Project> = Api::all(ctx.client.clone());
    api.patch_status(
        &p.name_any(),
        &PatchParams::default(),
        &Patch::Merge(json!({ "status": body })),
    )
    .await?;
    Ok(())
}

/// Whether a check says an existing object cannot be reused (see `blocked`).
fn is_blocked(c: &Check) -> bool {
    matches!(c, Check::Pending(r, _) if r == "Terminating") || is_not_owned(c)
}

/// Carries `namespace` and `sandbox` over from the previous status when this pass did not reach
/// them (for example after a transient API error). If an object is blocked
/// (`Terminating`/`NotOwnedError`) the previous values are NOT kept: the endpoint and Secret
/// name may belong to someone else's object, so they are removed until all checks pass again.
/// Returns whether the two fields must be cleared in the patch.
fn merge_status(old: &ProjectStatus, status: &mut ProjectStatus, obs: &Observed) -> bool {
    if is_blocked(&obs.resources) || is_blocked(&obs.sandbox) {
        status.namespace = None;
        status.sandbox = None;
        return true;
    }
    status.namespace = status.namespace.take().or(old.namespace.clone());
    status.sandbox = status.sandbox.take().or(old.sandbox.clone());
    false
}

/// 32 random bytes, hex encoded (64 characters).
fn new_token() -> String {
    use std::fmt::Write;
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// True while the PVC reports an expansion in flight.
fn pvc_resizing(pvc: &PersistentVolumeClaim) -> bool {
    pvc.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|cs| {
            cs.iter().any(|c| {
                matches!(c.type_.as_str(), "Resizing" | "FileSystemResizePending")
                    && c.status == "True"
            })
        })
}

/// Turns an API server refusal of a PVC patch (a shrink, or a StorageClass that cannot expand)
/// into a failure that needs a person. Anything else returns `None` and is retried.
fn classify_pvc_error(what: &str, name: &str, e: &kube::Error) -> Option<Check> {
    let kube::Error::Api(status) = e else {
        return None;
    };
    if status.code != 422 && status.code != 403 {
        return None;
    }
    let msg = status.message.to_lowercase();
    if !["storage", "shrink", "less than previous", "expansion"]
        .iter()
        .any(|k| msg.contains(k))
    {
        return None;
    }
    Some(Check::Failure(
        REASON_WORKSPACE_RESIZE_FAILED.into(),
        format!(
            "{what} {name}: {}; storage can only grow and the StorageClass must set allowVolumeExpansion",
            status.message
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_err(code: u16, message: &str) -> kube::Error {
        kube::Error::Api(Box::new(
            kube::core::Status::failure(message, "Invalid").with_code(code),
        ))
    }

    #[test]
    fn rejected_resize_is_a_failure_the_ui_shows_as_failed() {
        for (code, msg) in [
            (
                422,
                "spec.resources.requests.storage: Forbidden: field can not be less than previous value",
            ),
            (
                422,
                "only dynamically provisioned pvc can be resized and the storageclass that provisions the pvc must support resize",
            ),
            (
                403,
                "persistentvolumeclaims \"x\" is forbidden: expansion not allowed",
            ),
        ] {
            match classify_pvc_error("workspace PVC", "sb-x-workspace", &api_err(code, msg)) {
                Some(Check::Failure(reason, m)) => {
                    assert_eq!(reason, "WorkspaceResizeFailed");
                    assert!(reason.ends_with("Failed"));
                    assert!(m.contains("sb-x-workspace") && m.contains(msg));
                }
                other => panic!("{code} {msg}: {other:?}"),
            }
        }
    }

    #[test]
    fn other_pvc_errors_keep_retrying() {
        assert!(classify_pvc_error("w", "n", &api_err(500, "storage backend down")).is_none());
        assert!(classify_pvc_error("w", "n", &api_err(422, "name is invalid")).is_none());
        let e = kube::Error::Service("connection refused".into());
        assert!(classify_pvc_error("w", "n", &e).is_none());
    }

    #[test]
    fn resize_in_flight_is_detected_from_pvc_conditions() {
        use k8s_openapi::api::core::v1::{
            PersistentVolumeClaimCondition, PersistentVolumeClaimStatus,
        };
        let mut pvc = PersistentVolumeClaim::default();
        assert!(!pvc_resizing(&pvc));
        let cond = |t: &str, s: &str| PersistentVolumeClaimCondition {
            type_: t.into(),
            status: s.into(),
            ..Default::default()
        };
        pvc.status = Some(PersistentVolumeClaimStatus {
            conditions: Some(vec![cond("FileSystemResizePending", "True")]),
            ..Default::default()
        });
        assert!(pvc_resizing(&pvc));
        pvc.status = Some(PersistentVolumeClaimStatus {
            conditions: Some(vec![cond("Resizing", "False")]),
            ..Default::default()
        });
        assert!(!pvc_resizing(&pvc));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let secs: Vec<_> = (1..=9).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(secs, [5, 10, 20, 40, 80, 160, 300, 300, 300]);
        assert_eq!(backoff(0).as_secs(), 5);
        assert_eq!(backoff(u32::MAX).as_secs(), 300);
    }

    #[test]
    fn foreign_objects_report_terminating_on_the_right_condition() {
        use crate::conditions::{Observed, compute};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
        let obs = Observed {
            resources: blocked("Secret", "sb-x-auth", &Ownership::Terminating),
            sandbox: Check::Skipped,
        };
        let now = Time(k8s_openapi::jiff::Timestamp::from_second(1).unwrap());
        let c = compute(1, &obs, &[], &now);
        assert_eq!(c[0].type_, "ResourcesReady");
        assert_eq!(
            (c[0].status.as_str(), c[0].reason.as_str()),
            ("False", "Terminating")
        );
        assert!(c[0].message.contains("sb-x-auth"));
        assert_eq!(c[2].status, "False");
    }

    #[test]
    fn not_owned_is_a_failure_naming_the_object() {
        use crate::conditions::{Observed, compute};
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
        let check = blocked("workspace PVC", "sb-x-workspace", &Ownership::NotOwned);
        assert!(is_not_owned(&check));
        assert!(!is_not_owned(&blocked(
            "Secret",
            "s",
            &Ownership::Terminating
        )));
        let obs = Observed {
            resources: check,
            sandbox: Check::Skipped,
        };
        let now = Time(k8s_openapi::jiff::Timestamp::from_second(1).unwrap());
        let c = compute(1, &obs, &[], &now);
        assert_eq!(
            (c[0].status.as_str(), c[0].reason.as_str()),
            ("False", "NotOwnedError")
        );
        assert!(c[0].reason.ends_with("Error"));
        assert!(c[0].message.contains("sb-x-workspace") && c[0].message.contains("administrator"));
        assert!(NOT_OWNED_RETRY > POLL_NOT_READY);
    }

    fn ready_status() -> ProjectStatus {
        let mut p = crate::desired::tests::project("a", "o");
        p.metadata.uid = Some("u".into());
        let d = crate::desired::tests::defaults();
        ProjectStatus {
            namespace: Some(d.sandbox_namespace.clone()),
            sandbox: Some(desired::sandbox_status(&p, &d)),
            ..Default::default()
        }
    }

    fn obs(resources: Check, sandbox: Check) -> Observed {
        Observed { resources, sandbox }
    }

    #[test]
    fn blocked_results_never_advertise_a_sandbox() {
        for why in [Ownership::Terminating, Ownership::NotOwned] {
            let blocked_res = blocked("Secret", "sb-a-auth", &why);
            // Fresh status: nothing is set.
            let mut st = ProjectStatus::default();
            let clear = merge_status(
                &ProjectStatus::default(),
                &mut st,
                &obs(blocked_res.clone(), Check::Skipped),
            );
            assert!(
                clear && st.sandbox.is_none() && st.namespace.is_none(),
                "{why:?}"
            );
            // Previously ready: the old endpoint is dropped, not kept.
            let mut st = ProjectStatus::default();
            let clear = merge_status(
                &ready_status(),
                &mut st,
                &obs(Check::Ok, blocked("Sandbox", "sb-a", &why)),
            );
            assert!(
                clear && st.sandbox.is_none() && st.namespace.is_none(),
                "{why:?}"
            );
        }
    }

    #[test]
    fn unblocked_partial_pass_keeps_previous_values() {
        // Transient API error before the Sandbox was reached: keep what was advertised.
        let mut st = ProjectStatus::default();
        let clear = merge_status(
            &ready_status(),
            &mut st,
            &obs(Check::Error("boom".into()), Check::Skipped),
        );
        assert!(!clear);
        assert_eq!(st.sandbox, ready_status().sandbox);
        assert_eq!(st.namespace, ready_status().namespace);
        // A pass that reached the end sets new values.
        let mut st = ProjectStatus {
            namespace: Some("other".into()),
            sandbox: ready_status().sandbox,
            ..Default::default()
        };
        merge_status(
            &ProjectStatus::default(),
            &mut st,
            &obs(Check::Ok, Check::Ok),
        );
        assert_eq!(st.namespace.as_deref(), Some("other"));
    }

    #[test]
    fn token_is_32_random_bytes_hex() {
        let a = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, new_token());
    }
}
