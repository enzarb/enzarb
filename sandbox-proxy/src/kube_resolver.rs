//! Kubernetes-backed [`Resolver`].
//!
//! * Projects: a `kube::runtime` reflector keeps every Project in memory; lookups
//!   never hit the API server.
//! * Auth tokens: read on demand from the Secret named in
//!   `status.sandbox.authSecretRef` and cached for a short TTL. This needs only
//!   `get` on secrets (restrictable with `resourceNames: [sandbox-auth]`), not the
//!   cluster-wide `list`/`watch` a Secret watcher would require.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use enzarb_crds::Project;
use futures_util::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::{
    Api, Client,
    runtime::{
        WatchStreamExt,
        reflector::{self, ObjectRef, Store, reflector},
        watcher,
    },
};

use crate::resolver::{ProjectInfo, ResolveError, Resolver, SandboxRef};

type CacheKey = (String, String, String);

pub struct KubeResolver {
    client: Client,
    projects: Store<Project>,
    ttl: Duration,
    cache: Mutex<HashMap<CacheKey, (Instant, String)>>,
}

impl KubeResolver {
    /// Start the Project reflector in the background and wait until the first
    /// listing has been loaded.
    pub async fn start(client: Client, secret_ttl: Duration) -> anyhow::Result<Self> {
        let api: Api<Project> = Api::all(client.clone());
        let (store, writer) = reflector::store::<Project>();
        let stream = reflector(writer, watcher(api, watcher::Config::default()))
            .default_backoff()
            .applied_objects();
        tokio::spawn(async move {
            let mut stream = std::pin::pin!(stream);
            while let Some(ev) = stream.next().await {
                if let Err(e) = ev {
                    tracing::warn!(error = %e, "project watch error");
                }
            }
        });
        store.wait_until_ready().await?;
        Ok(Self {
            client,
            projects: store,
            ttl: secret_ttl,
            cache: Mutex::new(HashMap::new()),
        })
    }
}

fn key(s: &SandboxRef) -> CacheKey {
    (
        s.secret_namespace.clone(),
        s.secret_name.clone(),
        s.secret_key.clone(),
    )
}

/// Pure mapping from a Project object to what the proxy needs.
pub fn project_info(p: &Project) -> ProjectInfo {
    let sandbox = p
        .status
        .as_ref()
        .and_then(|st| st.sandbox.as_ref().map(|sb| (st, sb)))
        .map(|(st, sb)| SandboxRef {
            endpoint: sb.endpoint.clone(),
            secret_namespace: st.namespace.clone().unwrap_or_else(|| p.namespace_name()),
            secret_name: sb.auth_secret_ref.name.clone(),
            secret_key: sb.auth_secret_ref.key.clone(),
        });
    ProjectInfo {
        owner: p.spec.owner.clone(),
        sandbox,
    }
}

#[async_trait]
impl Resolver for KubeResolver {
    async fn project(&self, name: &str) -> Result<ProjectInfo, ResolveError> {
        self.projects
            .get(&ObjectRef::new(name))
            .map(|p| project_info(&p))
            .ok_or(ResolveError::NotFound)
    }

    async fn token(&self, s: &SandboxRef) -> Result<String, ResolveError> {
        if let Some((at, tok)) = self.cache.lock().unwrap().get(&key(s))
            && at.elapsed() < self.ttl
        {
            return Ok(tok.clone());
        }
        let api: Api<Secret> = Api::namespaced(self.client.clone(), &s.secret_namespace);
        let secret = api.get_opt(&s.secret_name).await.map_err(|e| {
            tracing::warn!(error = %e, secret = %s.secret_name, "secret read failed");
            ResolveError::Unavailable("secret read failed".into())
        })?;
        let tok = secret
            .and_then(|sec| sec.data)
            .and_then(|mut d| d.remove(&s.secret_key))
            .and_then(|b| String::from_utf8(b.0).ok())
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .ok_or(ResolveError::NotFound)?;
        self.cache
            .lock()
            .unwrap()
            .insert(key(s), (Instant::now(), tok.clone()));
        Ok(tok)
    }

    fn invalidate_token(&self, s: &SandboxRef) {
        self.cache.lock().unwrap().remove(&key(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_crds::{ProjectSpec, ProjectStatus, SandboxStatus, SecretKeyRef};

    fn project(status: Option<ProjectStatus>) -> Project {
        let mut p = Project::new(
            "enzarb",
            ProjectSpec {
                owner: "ruckc".into(),
                display_name: String::new(),
                repositories: vec![],
                run_mode: Default::default(),
                triggers: Default::default(),
                sandbox: Default::default(),
            },
        );
        p.status = status;
        p
    }

    #[test]
    fn maps_status() {
        let p = project(Some(ProjectStatus {
            namespace: Some("enzarb-p-enzarb".into()),
            sandbox: Some(SandboxStatus {
                name: "sandbox".into(),
                endpoint: "ws://sandbox.enzarb-p-enzarb.svc:7070/v1/mux".into(),
                auth_secret_ref: SecretKeyRef {
                    name: "sandbox-auth".into(),
                    key: "token".into(),
                },
            }),
            ..Default::default()
        }));
        let i = project_info(&p);
        assert_eq!(i.owner, "ruckc");
        let s = i.sandbox.unwrap();
        assert_eq!(s.secret_namespace, "enzarb-p-enzarb");
        assert_eq!(s.secret_name, "sandbox-auth");
    }

    #[test]
    fn no_sandbox_until_status() {
        assert!(project_info(&project(None)).sandbox.is_none());
    }
}
