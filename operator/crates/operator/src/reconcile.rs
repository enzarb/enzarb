//! Applies the desired objects (see `desired`) and records status (see `conditions`).

use crate::conditions::{self, Check, Observed};
use crate::desired::{self, Defaults, FIELD_MANAGER, Ownership};
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{Project, ProjectStatus};
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
    failures: Mutex<HashMap<String, u32>>,
}

impl Ctx {
    pub fn new(client: Client, defaults: Defaults) -> Self {
        Self {
            client,
            defaults,
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
        write_status(p, ctx, status, &obs).await?;
        // Nothing to retry until the user edits the spec.
        return Ok(Action::await_change());
    }

    let result = drive(p, ctx, &mut obs, &mut status).await;
    write_status(p, ctx, status, &obs).await?;
    result?;

    if obs.resources == Check::Ok && obs.sandbox == Check::Ok {
        info!(project = %name, "ready");
        Ok(Action::requeue(RESYNC))
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
        pvcs.patch(&name, &ssa(), &Patch::Apply(&pvc))
            .await
            .map_err(|e| res_err(obs, what, e))?;
    }
    obs.resources = Check::Ok;

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
    Ok(())
}

async fn write_status(
    p: &Project,
    ctx: &Ctx,
    mut status: ProjectStatus,
    obs: &Observed,
) -> Result<(), Error> {
    let now = Time(k8s_openapi::jiff::Timestamp::now());
    status.conditions = conditions::compute(
        p.metadata.generation.unwrap_or_default(),
        obs,
        &status.conditions,
        &now,
    );
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

#[cfg(test)]
mod tests {
    use super::*;

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
