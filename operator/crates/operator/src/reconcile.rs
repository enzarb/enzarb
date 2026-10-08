//! Applies the desired objects (see `desired`) and records status (see `conditions`).

use crate::conditions::{self, Check, Observed};
use crate::desired::{self, Defaults, FIELD_MANAGER, FINALIZER};
use enzarb_crds::upstream::capsule::Tenant;
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{Project, ProjectStatus};
use k8s_openapi::api::core::v1::{Namespace, PersistentVolumeClaim, Secret};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::api::{Api, DeleteParams, Patch, PatchParams, PostParams};
use kube::runtime::controller::Action;
use kube::runtime::finalizer::{Event, finalizer};
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
    #[error("{0}")]
    Finalizer(#[source] Box<dyn std::error::Error + Send + Sync>),
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
    let projects: Api<Project> = Api::all(ctx.client.clone());
    let name = project.name_any();
    let action = finalizer(&projects, FINALIZER, project, |event| async {
        match event {
            Event::Apply(p) => apply(&p, &ctx).await,
            Event::Cleanup(p) => cleanup(&p, &ctx).await,
        }
    })
    .await
    .map_err(|e| match e {
        kube::runtime::finalizer::Error::ApplyFailed(e)
        | kube::runtime::finalizer::Error::CleanupFailed(e) => e,
        other => Error::Finalizer(Box::new(other)),
    })?;
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
        obs.tenant = Check::Error(format!("InvalidSpec: {e}"));
        write_status(p, ctx, status, &obs).await?;
        // Nothing to retry until the user edits the spec.
        return Ok(Action::await_change());
    }

    let result = drive(p, ctx, &mut obs, &mut status).await;
    write_status(p, ctx, status, &obs).await?;
    result?;

    let all_ok = obs.tenant == Check::Ok && obs.namespace == Check::Ok && obs.sandbox == Check::Ok;
    if all_ok {
        info!(project = %name, "ready");
        Ok(Action::requeue(RESYNC))
    } else {
        Ok(Action::requeue(POLL_NOT_READY))
    }
}

/// Applies each object in order, recording a check per step. Stops at the first API error.
async fn drive(
    p: &Project,
    ctx: &Ctx,
    obs: &mut Observed,
    status: &mut ProjectStatus,
) -> Result<(), Error> {
    let ns_name = p.namespace_name();

    let tenants: Api<Tenant> = Api::all(ctx.client.clone());
    let t = tenants
        .patch(&p.tenant_name(), &ssa(), &Patch::Apply(&desired::tenant(p)))
        .await
        .inspect_err(|e| obs.tenant = Check::Error(e.to_string()))?;
    obs.tenant = conditions::tenant_check(&t);

    let namespaces: Api<Namespace> = Api::all(ctx.client.clone());
    let ns = namespaces
        .patch(&ns_name, &ssa(), &Patch::Apply(&desired::namespace(p)))
        .await
        .inspect_err(|e| obs.namespace = Check::Error(e.to_string()))?;
    obs.namespace = conditions::namespace_check(&ns);
    status.namespace = Some(ns_name.clone());
    if obs.namespace != Check::Ok {
        return Ok(());
    }

    // Isolation is part of the namespace being usable, and it is applied before any pod
    // exists, so a failure is reported on NamespaceReady and nothing is started unprotected.
    let policies: Api<NetworkPolicy> = Api::namespaced(ctx.client.clone(), &ns_name);
    policies
        .patch(
            desired::NETPOL_NAME,
            &ssa(),
            &Patch::Apply(&desired::network_policy(p, &ctx.defaults.netpol)),
        )
        .await
        .inspect_err(|e| obs.namespace = Check::Error(format!("NetworkPolicy: {e}")))?;

    let sandbox_err = |obs: &mut Observed, e: kube::Error| {
        obs.sandbox = Check::Error(e.to_string());
        e
    };

    // Secret: created once, never touched again (the token must not rotate).
    let secrets: Api<Secret> = Api::namespaced(ctx.client.clone(), &ns_name);
    if secrets
        .get_opt(desired::AUTH_SECRET_NAME)
        .await
        .map_err(|e| sandbox_err(obs, e))?
        .is_none()
    {
        let secret = desired::auth_secret(p, &new_token());
        match secrets.create(&PostParams::default(), &secret).await {
            Ok(_) => info!(project = %p.name_any(), "created sandbox-auth secret"),
            Err(kube::Error::Api(s)) if s.code == 409 => {}
            Err(e) => return Err(sandbox_err(obs, e).into()),
        }
    }

    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(ctx.client.clone(), &ns_name);
    pvcs.patch(
        desired::PVC_NAME,
        &ssa(),
        &Patch::Apply(&desired::pvc(p, &ctx.defaults)),
    )
    .await
    .map_err(|e| sandbox_err(obs, e))?;
    pvcs.patch(
        crate::docker::PVC_NAME,
        &ssa(),
        &Patch::Apply(&desired::docker_pvc(p, &ctx.defaults)),
    )
    .await
    .map_err(|e| sandbox_err(obs, e))?;

    let sandboxes: Api<Sandbox> = Api::namespaced(ctx.client.clone(), &ns_name);
    let sb = sandboxes
        .patch(
            desired::SANDBOX_NAME,
            &ssa(),
            &Patch::Apply(&desired::sandbox(p, &ctx.defaults)),
        )
        .await
        .map_err(|e| sandbox_err(obs, e))?;
    status.sandbox = Some(desired::sandbox_status(p));
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
    // Fields not reached this pass keep their previous value.
    let old = p.status.clone().unwrap_or_default();
    status.namespace = status.namespace.or(old.namespace.clone());
    status.sandbox = status.sandbox.or(old.sandbox.clone());
    if old == status {
        return Ok(());
    }
    let api: Api<Project> = Api::all(ctx.client.clone());
    api.patch_status(
        &p.name_any(),
        &PatchParams::default(),
        &Patch::Merge(json!({ "status": status })),
    )
    .await?;
    Ok(())
}

/// Deletes the project namespace (only if we created it). The Tenant is shared by all of
/// the owner's projects and may hold namespaces we did not create, so it is never deleted.
async fn cleanup(p: &Project, ctx: &Ctx) -> Result<Action, Error> {
    let name = p.name_any();
    let namespaces: Api<Namespace> = Api::all(ctx.client.clone());
    if let Some(ns) = namespaces.get_opt(&p.namespace_name()).await? {
        if desired::namespace_belongs_to(&ns, &name) {
            match namespaces
                .delete(&p.namespace_name(), &DeleteParams::default())
                .await
            {
                Ok(_) => {
                    info!(project = %name, namespace = %p.namespace_name(), "deleted namespace")
                }
                Err(kube::Error::Api(s)) if s.code == 404 => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            warn!(project = %name, "namespace not created by this operator; leaving it");
        }
    }
    Ok(Action::await_change())
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
    fn token_is_32_random_bytes_hex() {
        let a = new_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, new_token());
    }
}
