//! Actor permission lookup, the one piece of I/O a transition depends on.
//!
//! The reducer resolves permissions *before* it opens the event's transaction, through
//! a [`PermissionResolver`]. Production wires [`ConnectorPermissions`] (one
//! `Connector::check_permission` per repo key's connector) inside a
//! [`CachedPermissions`] (5 minutes per repo and actor). Tests use [`StaticPermissions`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use enzarb_connector::{Actor, Connector, Permission, RepoKey};

/// Time a resolved permission stays cached.
pub const PERMISSION_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("permission lookup failed: {0}")]
pub struct PermissionError(pub String);

#[async_trait]
pub trait PermissionResolver: Send + Sync {
    async fn permission(
        &self,
        repo: &RepoKey,
        actor: &Actor,
    ) -> Result<Permission, PermissionError>;
}

/// Routes to the `Connector` named by the repo key (`github:...` -> `"github"`).
#[derive(Default)]
pub struct ConnectorPermissions {
    connectors: HashMap<String, Arc<dyn Connector>>,
}

impl ConnectorPermissions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, connector_name: &str, c: Arc<dyn Connector>) -> Self {
        self.connectors.insert(connector_name.to_string(), c);
        self
    }
}

#[async_trait]
impl PermissionResolver for ConnectorPermissions {
    async fn permission(
        &self,
        repo: &RepoKey,
        actor: &Actor,
    ) -> Result<Permission, PermissionError> {
        let c = self
            .connectors
            .get(&repo.connector)
            .ok_or_else(|| PermissionError(format!("no connector {:?}", repo.connector)))?;
        c.check_permission(repo, actor)
            .await
            .map_err(|e| PermissionError(e.to_string()))
    }
}

/// Caches successful lookups per (repo, actor login), both case-insensitive. Errors are not cached.
pub struct CachedPermissions<R> {
    inner: R,
    ttl: Duration,
    cache: Mutex<HashMap<(String, String), (Instant, Permission)>>,
}

impl<R: PermissionResolver> CachedPermissions<R> {
    pub fn new(inner: R) -> Self {
        Self::with_ttl(inner, PERMISSION_TTL)
    }

    pub fn with_ttl(inner: R, ttl: Duration) -> Self {
        Self {
            inner,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl<R: PermissionResolver> PermissionResolver for CachedPermissions<R> {
    async fn permission(
        &self,
        repo: &RepoKey,
        actor: &Actor,
    ) -> Result<Permission, PermissionError> {
        let key = (
            repo.to_string().to_ascii_lowercase(),
            actor.login.to_ascii_lowercase(),
        );
        if let Some((at, p)) = self.cache.lock().expect("cache lock").get(&key)
            && at.elapsed() < self.ttl
        {
            return Ok(*p);
        }
        let p = self.inner.permission(repo, actor).await?;
        self.cache
            .lock()
            .expect("cache lock")
            .insert(key, (Instant::now(), p));
        Ok(p)
    }
}

/// Fake: fixed permissions by actor login (case-insensitive), with a default, and
/// a call counter.
pub struct StaticPermissions {
    by_login: RwLock<HashMap<String, Permission>>,
    default: RwLock<Permission>,
    fail: RwLock<bool>,
    calls: Mutex<u64>,
}

impl StaticPermissions {
    pub fn new(default: Permission) -> Self {
        Self {
            by_login: RwLock::new(HashMap::new()),
            default: RwLock::new(default),
            fail: RwLock::new(false),
            calls: Mutex::new(0),
        }
    }

    pub fn set(&self, login: &str, p: Permission) {
        self.by_login
            .write()
            .expect("lock")
            .insert(login.to_ascii_lowercase(), p);
    }

    /// Make every lookup fail (simulates a connector outage).
    pub fn set_failing(&self, fail: bool) {
        *self.fail.write().expect("lock") = fail;
    }

    pub fn calls(&self) -> u64 {
        *self.calls.lock().expect("lock")
    }
}

#[async_trait]
impl PermissionResolver for StaticPermissions {
    async fn permission(
        &self,
        _repo: &RepoKey,
        actor: &Actor,
    ) -> Result<Permission, PermissionError> {
        *self.calls.lock().expect("lock") += 1;
        if *self.fail.read().expect("lock") {
            return Err(PermissionError("simulated outage".into()));
        }
        Ok(self
            .by_login
            .read()
            .expect("lock")
            .get(&actor.login.to_ascii_lowercase())
            .copied()
            .unwrap_or(*self.default.read().expect("lock")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzarb_connector::fake::FakeConnector;

    fn actor(login: &str) -> Actor {
        Actor {
            login: login.into(),
            id: 1,
            is_bot: false,
        }
    }

    #[tokio::test]
    async fn cache_hits_per_repo_and_actor_case_insensitively() {
        let inner = Arc::new(StaticPermissions::new(Permission::Read));
        inner.set("alice", Permission::Write);
        struct Shared(Arc<StaticPermissions>);
        #[async_trait]
        impl PermissionResolver for Shared {
            async fn permission(
                &self,
                r: &RepoKey,
                a: &Actor,
            ) -> Result<Permission, PermissionError> {
                self.0.permission(r, a).await
            }
        }
        let cached = CachedPermissions::new(Shared(inner.clone()));
        let repo: RepoKey = "github:o/r".parse().unwrap();
        let upper: RepoKey = "github:O/R".parse().unwrap();
        assert_eq!(
            cached.permission(&repo, &actor("alice")).await,
            Ok(Permission::Write)
        );
        assert_eq!(
            cached.permission(&upper, &actor("ALICE")).await,
            Ok(Permission::Write)
        );
        assert_eq!(inner.calls(), 1);
        assert_eq!(
            cached.permission(&repo, &actor("bob")).await,
            Ok(Permission::Read)
        );
        assert_eq!(inner.calls(), 2);
        let other: RepoKey = "github:o/other".parse().unwrap();
        cached.permission(&other, &actor("alice")).await.unwrap();
        assert_eq!(inner.calls(), 3);
    }

    #[tokio::test]
    async fn expired_entries_refresh_and_errors_are_not_cached() {
        let inner = Arc::new(StaticPermissions::new(Permission::Read));
        struct Shared(Arc<StaticPermissions>);
        #[async_trait]
        impl PermissionResolver for Shared {
            async fn permission(
                &self,
                r: &RepoKey,
                a: &Actor,
            ) -> Result<Permission, PermissionError> {
                self.0.permission(r, a).await
            }
        }
        let cached = CachedPermissions::with_ttl(Shared(inner.clone()), Duration::from_millis(30));
        let repo: RepoKey = "github:o/r".parse().unwrap();
        inner.set_failing(true);
        assert!(cached.permission(&repo, &actor("a")).await.is_err());
        inner.set_failing(false);
        assert!(cached.permission(&repo, &actor("a")).await.is_ok());
        assert_eq!(inner.calls(), 2);
        tokio::time::sleep(Duration::from_millis(50)).await;
        cached.permission(&repo, &actor("a")).await.unwrap();
        assert_eq!(inner.calls(), 3);
    }

    #[tokio::test]
    async fn connector_resolver_uses_check_permission() {
        let fake = Arc::new(FakeConnector::new());
        let repo: RepoKey = "github:o/r".parse().unwrap();
        fake.set_permission(repo.clone(), "alice", Permission::Maintain);
        let r = ConnectorPermissions::new().with("github", fake);
        assert_eq!(
            r.permission(&repo, &actor("alice")).await,
            Ok(Permission::Maintain)
        );
        let gl: RepoKey = "gitlab:o/r".parse().unwrap();
        assert!(r.permission(&gl, &actor("alice")).await.is_err());
    }
}
