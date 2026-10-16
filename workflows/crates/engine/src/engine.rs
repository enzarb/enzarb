//! The engine: shared handles and config for the reducer, executor and ticker roles.

use std::sync::Arc;
use std::time::Duration;

use enzarb_connector::{Connector, RepoKey, WorkItemKey};
use sqlx::PgPool;

use crate::index::ProjectIndex;
use crate::library::Library;
use crate::permission::PermissionResolver;
use crate::sandbox::SandboxResolver;
use crate::secrets::SecretsKey;
use crate::steps::StepRunner;

/// Instance configuration (env, see `enzarb-workflows` config).
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// `@handle` that may prefix commands (`@enzarb /plan`).
    pub bot_handle: String,
    /// The App's bot login; its own events never start tasks.
    pub bot_login: String,
    /// Base URL of the instance UI (`ENZARB_PUBLIC_URL`), for `instance.url`.
    pub public_url: String,
    /// No session events for this long pauses the task (`ENZARB_STALL_SECS`, default 1800).
    pub stall: Duration,
    /// Concurrent sandbox steps per Project (admin capacity).
    pub max_steps_per_project: i64,
    /// Executor parallelism.
    pub executor_slots: usize,
    /// Base of the one backoff (doubles per attempt, capped at 10 minutes).
    pub backoff_base: Duration,
    /// Idle poll interval of each loop (they also wake on NOTIFY).
    pub poll: Duration,
    /// Identifies this process in job leases.
    pub worker_id: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            bot_handle: "enzarb".into(),
            bot_login: "enzarb[bot]".into(),
            public_url: String::new(),
            stall: Duration::from_secs(1800),
            max_steps_per_project: 4,
            executor_slots: 32,
            backoff_base: Duration::from_secs(5),
            poll: Duration::from_secs(2),
            worker_id: format!("workflows-{}", std::process::id()),
        }
    }
}

/// Everything the roles share. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    pub pool: PgPool,
    pub index: Arc<dyn ProjectIndex>,
    pub library: Library,
    pub permissions: Arc<dyn PermissionResolver>,
    pub runner: Arc<dyn StepRunner>,
    pub sandboxes: Arc<dyn SandboxResolver>,
    pub secrets_key: Option<SecretsKey>,
    /// The UI-captured per-user Claude token store, as a user-scope secret source.
    pub claude: Option<crate::secrets::ClaudeSource>,
    pub cfg: EngineConfig,
}

impl Engine {
    pub fn connector(&self) -> &Arc<dyn Connector> {
        self.library.connector()
    }

    /// The one backoff: `base * 2^(attempt-1)`, capped at 10 minutes.
    pub fn backoff(&self, attempt: i32) -> Duration {
        let exp = attempt.clamp(1, 16) as u32 - 1;
        (self.cfg.backoff_base * 2u32.pow(exp)).min(Duration::from_secs(600))
    }
}

/// `owner/repo` -> GitHub [`RepoKey`].
pub fn repo_key(repo: &str) -> RepoKey {
    let (o, r) = repo.split_once('/').unwrap_or((repo, ""));
    RepoKey::new("github", o, r)
}

/// Subject key `github:owner/repo#n` (lowercase repo).
pub fn subject_key(repo: &str, number: u64) -> String {
    WorkItemKey::new(repo_key(&repo.to_ascii_lowercase()), number).to_string()
}

/// Parses `github:owner/repo#n`.
pub fn parse_subject_key(key: &str) -> Option<(String, u64)> {
    let k: WorkItemKey = key.parse().ok()?;
    Some((k.repo.full_name(), k.number))
}
