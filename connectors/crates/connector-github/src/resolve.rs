//! Fills `actor.can_write` on mapped comment events from the connector's
//! permission check, behind a small TTL cache. [`crate::catalog::map`] stays pure.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use enzarb_connector::{Actor, Connector, Permission, RepoKey};
use enzarb_flow::events::Payload;

use crate::catalog::Mapped;

/// How long a successful lookup is reused.
pub const PERMISSION_TTL: Duration = Duration::from_secs(300);

/// Resolves the permission of comment authors. Failed lookups are logged, yield
/// `can_write: false` and are never cached.
pub struct CommentPermissions<C> {
    connector: C,
    ttl: Duration,
    cache: Mutex<HashMap<(String, String), (Instant, Permission)>>,
}

impl<C: Connector> CommentPermissions<C> {
    pub fn new(connector: C) -> Self {
        Self::with_ttl(connector, PERMISSION_TTL)
    }

    pub fn with_ttl(connector: C, ttl: Duration) -> Self {
        Self {
            connector,
            ttl,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Sets `actor.can_write` on every comment event (issue and pull request
    /// comments, review comments). Other events are left alone.
    pub async fn resolve(&self, mapped: &mut [Mapped]) {
        for (_, ev) in mapped.iter_mut() {
            if !matches!(ev.payload, Payload::Comment(_)) {
                continue;
            }
            let (Some(repo), Some(actor)) = (ev.repo.as_deref(), ev.actor.as_mut()) else {
                continue;
            };
            if actor.bot || actor.login.is_empty() {
                continue;
            }
            actor.can_write = self.can_write(repo, &actor.login).await;
        }
    }

    async fn can_write(&self, repo: &str, login: &str) -> bool {
        let Some((owner, name)) = repo.split_once('/') else {
            return false;
        };
        let key = (repo.to_ascii_lowercase(), login.to_ascii_lowercase());
        if let Some((at, p)) = self.cache.lock().expect("cache lock").get(&key)
            && at.elapsed() < self.ttl
        {
            return p.can_write();
        }
        let actor = Actor {
            login: login.to_string(),
            id: 0,
            is_bot: false,
        };
        match self
            .connector
            .check_permission(&RepoKey::new("github", owner, name), &actor)
            .await
        {
            Ok(p) => {
                self.cache
                    .lock()
                    .expect("cache lock")
                    .insert(key, (Instant::now(), p));
                p.can_write()
            }
            Err(e) => {
                tracing::warn!(repo, login, "permission lookup failed: {e}");
                false
            }
        }
    }
}
