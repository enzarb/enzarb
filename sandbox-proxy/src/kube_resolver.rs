//! Kubernetes-backed [`Resolver`].
//!
//! * Projects: a `kube::runtime` reflector keeps every Project in memory; lookups
//!   never hit the API server.
//! * Auth tokens: read on demand from the Secret named in
//!   `status.sandbox.authSecretRef` and cached for a short TTL. This needs only
//!   `get` on secrets, granted by a Role in the shared sandbox namespace
//!   (ADR-0013), not the cluster-wide `list`/`watch` a Secret watcher would need.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use enzarb_crds::Project;
use futures_util::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::{
    Api, Client, ResourceExt,
    runtime::{
        WatchStreamExt,
        reflector::{self, ObjectRef, Store, reflector},
        watcher,
    },
};

use tokio::sync::broadcast;

use crate::resolver::{CHANGE_CAPACITY, ProjectInfo, ResolveError, Resolver, SandboxRef};

type CacheKey = (String, String, String);

pub struct KubeResolver {
    client: Client,
    projects: Store<Project>,
    changes: broadcast::Sender<String>,
    ttl: Duration,
    cache: Mutex<HashMap<CacheKey, (Instant, String)>>,
}

impl KubeResolver {
    /// Start the Project reflector in the background and wait until the first
    /// listing has been loaded.
    pub async fn start(client: Client, secret_ttl: Duration) -> anyhow::Result<Self> {
        let api: Api<Project> = Api::all(client.clone());
        let (store, writer) = reflector::store::<Project>();
        let stream = reflector(writer, watcher(api, watcher::Config::default())).default_backoff();
        let (changes, _) = broadcast::channel(CHANGE_CAPACITY);
        let tx = changes.clone();
        tokio::spawn(async move {
            let mut stream = std::pin::pin!(stream);
            let mut names = NameTracker::default();
            while let Some(ev) = stream.next().await {
                // The reflector has already applied the event to the store.
                match ev {
                    Ok(ev) => {
                        for n in names.changed(&ev) {
                            let _ = tx.send(n);
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "project watch error"),
                }
            }
        });
        store.wait_until_ready().await?;
        Ok(Self {
            client,
            projects: store,
            changes,
            ttl: secret_ttl,
            cache: Mutex::new(HashMap::new()),
        })
    }
}

/// Maps raw watcher events to the names of Projects that changed, including
/// deletes and, after a relist, Projects that vanished while the watch was down.
#[derive(Default)]
struct NameTracker {
    known: HashSet<String>,
    relist: HashSet<String>,
}

impl NameTracker {
    fn changed(&mut self, ev: &watcher::Event<Project>) -> Vec<String> {
        use watcher::Event::*;
        match ev {
            Apply(p) => {
                self.known.insert(p.name_any());
                vec![p.name_any()]
            }
            Delete(p) => {
                self.known.remove(&p.name_any());
                vec![p.name_any()]
            }
            Init => {
                self.relist.clear();
                vec![]
            }
            InitApply(p) => {
                self.relist.insert(p.name_any());
                vec![]
            }
            InitDone => {
                // The store now holds the new listing: any old or new name may differ.
                let fresh = std::mem::take(&mut self.relist);
                let all = self.known.union(&fresh).cloned().collect();
                self.known = fresh;
                all
            }
        }
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
///
/// The sandbox is found only through `status.namespace` and `status.sandbox`
/// (ADR-0013), which the operator sets once it has verified ownership. If either
/// is missing the Project is not ready and `sandbox` is `None`; no name is derived.
pub fn project_info(p: &Project) -> ProjectInfo {
    let sandbox = p.status.as_ref().and_then(|st| {
        let namespace = st.namespace.as_ref().filter(|n| !n.is_empty())?;
        let sb = st.sandbox.as_ref()?;
        Some(SandboxRef {
            endpoint: sb.endpoint.clone(),
            secret_namespace: namespace.clone(),
            secret_name: sb.auth_secret_ref.name.clone(),
            secret_key: sb.auth_secret_ref.key.clone(),
        })
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

    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.changes.subscribe()
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
    fn tracker_reports_applies_deletes_and_relist() {
        use watcher::Event;
        let p = project(None);
        let mut t = NameTracker::default();
        assert_eq!(t.changed(&Event::Apply(p.clone())), vec!["enzarb"]);
        assert_eq!(t.changed(&Event::Delete(p.clone())), vec!["enzarb"]);

        // A Project known before a relist but absent from it is reported.
        t.changed(&Event::Apply(p.clone()));
        assert!(t.changed(&Event::Init).is_empty());
        assert_eq!(t.changed(&Event::InitDone), vec!["enzarb"]);
        assert!(t.known.is_empty());

        // Listed Projects are reported at InitDone.
        t.changed(&Event::Init);
        assert!(t.changed(&Event::InitApply(p)).is_empty());
        assert_eq!(t.changed(&Event::InitDone), vec!["enzarb"]);
    }

    #[test]
    fn maps_status() {
        let p = project(Some(ProjectStatus {
            namespace: Some("enzarb-sandboxes".into()),
            sandbox: Some(SandboxStatus {
                name: "sb-enzarb".into(),
                endpoint: "ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux".into(),
                auth_secret_ref: SecretKeyRef {
                    name: "sb-enzarb-auth".into(),
                    key: "token".into(),
                },
            }),
            ..Default::default()
        }));
        let i = project_info(&p);
        assert_eq!(i.owner, "ruckc");
        let s = i.sandbox.unwrap();
        assert_eq!(s.secret_namespace, "enzarb-sandboxes");
        assert_eq!(s.secret_name, "sb-enzarb-auth");
    }

    #[test]
    fn not_ready_without_namespace() {
        // status.sandbox alone is not enough: no derived namespace fallback.
        let p = project(Some(ProjectStatus {
            namespace: None,
            sandbox: Some(SandboxStatus {
                name: "sb-enzarb".into(),
                endpoint: "ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux".into(),
                auth_secret_ref: SecretKeyRef {
                    name: "sb-enzarb-auth".into(),
                    key: "token".into(),
                },
            }),
            ..Default::default()
        }));
        assert!(project_info(&p).sandbox.is_none());
    }

    #[test]
    fn not_ready_without_sandbox() {
        let p = project(Some(ProjectStatus {
            namespace: Some("enzarb-sandboxes".into()),
            ..Default::default()
        }));
        assert!(project_info(&p).sandbox.is_none());
    }

    #[test]
    fn no_sandbox_until_status() {
        assert!(project_info(&project(None)).sandbox.is_none());
    }
}
