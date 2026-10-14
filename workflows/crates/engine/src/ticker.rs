//! Ticker: turns due timers into `TimerFired` events.
//!
//! * `work_items.wake_at <= now()` becomes `TimerFired {}`, dedupe key
//!   `timer:<work_item_id>:<wake_at>` (RFC 3339, microseconds, UTC).
//! * A `running` job past `jobs.deadline` becomes `TimerFired {job_id}`, dedupe key
//!   `timer:<work_item_id>:job:<job_id>:<attempt>`.
//!
//! * Open enzarb-authored change requests whose base branch head moved and are confirmed
//!   unmergeable by a readiness check become `MergeConflictDetected {base_sha}` (see
//!   [`Ticker::poll_base_heads`]), dedupe key `conflict:<work_item_key>:<base_sha>`.
//!
//! Events go in through `workflows_api.ingest_event`, so a timer is emitted once per
//! (item, wake_at) however many tickers run or how often they tick.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use enzarb_connector::WorkItemKey;
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::watch;

use crate::conflict::emit_merge_conflict;
use crate::dependencies::open_dependencies;
use crate::error::Error;
use crate::executor::ConnectorRegistry;
use crate::grants;
use crate::index::ProjectIndex;

/// Minimum time between base-head polls; each pass costs one branch-head lookup per
/// distinct active (repo, base) plus a readiness call per open PR on a moved base.
pub const BASE_POLL_INTERVAL: Duration = Duration::from_secs(60);

/// Phases in which the machine acts on `MergeConflictDetected`.
const CONFLICT_PHASES: &[&str] = &["AwaitingChanges", "ReadyToMerge", "ConflictParked"];

/// (repo, base ref) of a base branch being polled.
type BaseKey = (String, String);

/// The open change requests (key, PR number) targeting one base branch.
type BaseGroup = (BaseKey, Vec<(WorkItemKey, u64)>);

/// What the base-head poll needs: connectors to ask, and which author counts as enzarb.
#[derive(Clone)]
struct BasePoll {
    connectors: ConnectorRegistry,
    bot_login: String,
    /// (repo, base ref) -> last base head whose open PRs all had a definite readiness.
    seen: Arc<Mutex<HashMap<BaseKey, String>>>,
    last_run: Arc<Mutex<Option<Instant>>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickStats {
    /// `TimerFired` events created for `wake_at`.
    pub wakes: usize,
    /// `TimerFired {job_id}` events created for job deadlines.
    pub deadlines: usize,
    /// `DependenciesChanged` events created.
    pub dependencies: usize,
}

/// Minimum time between GitHub dependency syncs; each pass lists the relationships of every
/// active work item.
pub const DEPENDENCY_SYNC_INTERVAL: Duration = Duration::from_secs(120);

/// What the dependency sync needs: connectors to ask, and the Projects that bound it.
#[derive(Clone)]
struct DepSync {
    connectors: ConnectorRegistry,
    index: Arc<dyn ProjectIndex>,
    last_run: Arc<Mutex<Option<Instant>>>,
}

#[derive(Clone)]
pub struct Ticker {
    pool: PgPool,
    interval: Duration,
    base_poll: Option<BasePoll>,
    dep_sync: Option<DepSync>,
}

const BATCH: i64 = 500;

impl Ticker {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            interval: Duration::from_secs(5),
            base_poll: None,
            dep_sync: None,
        }
    }

    /// Enable the sync of GitHub's native "blocked by" relationships into the engine.
    pub fn with_dependency_sync(
        mut self,
        connectors: ConnectorRegistry,
        index: Arc<dyn ProjectIndex>,
    ) -> Self {
        self.dep_sync = Some(DepSync {
            connectors,
            index,
            last_run: Arc::default(),
        });
        self
    }

    /// Enable the base-head conflict fan-out for change requests authored by `bot_login`.
    pub fn with_base_poll(mut self, connectors: ConnectorRegistry, bot_login: &str) -> Self {
        self.base_poll = Some(BasePoll {
            connectors,
            bot_login: bot_login.to_string(),
            seen: Arc::default(),
            last_run: Arc::default(),
        });
        self
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// One sweep.
    pub async fn tick(&self) -> Result<TickStats, Error> {
        let mut stats = TickStats::default();

        let wakes: Vec<(String, String)> = sqlx::query_as(
            "WITH due AS (
                 SELECT key, 'timer:' || id || ':' ||
                        to_char(wake_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS dk,
                        wake_at
                 FROM work_items WHERE wake_at IS NOT NULL AND wake_at <= now())
             SELECT key, dk FROM due
             WHERE NOT EXISTS (SELECT 1 FROM events e WHERE e.dedupe_key = due.dk)
             ORDER BY wake_at LIMIT $1",
        )
        .bind(BATCH)
        .fetch_all(&self.pool)
        .await?;
        for (key, dk) in wakes {
            if self.emit("TimerFired", &dk, &key, json!({})).await? {
                stats.wakes += 1;
            }
        }

        let deadlines: Vec<(String, String, i64)> = sqlx::query_as(
            "WITH due AS (
                 SELECT w.key, j.id AS job_id, j.deadline,
                        'timer:' || w.id || ':job:' || j.id || ':' || j.attempt AS dk
                 FROM jobs j JOIN work_items w ON w.id = j.work_item_id
                 WHERE j.status = 'running' AND j.deadline IS NOT NULL AND j.deadline <= now())
             SELECT key, dk, job_id FROM due
             WHERE NOT EXISTS (SELECT 1 FROM events e WHERE e.dedupe_key = due.dk)
             ORDER BY deadline LIMIT $1",
        )
        .bind(BATCH)
        .fetch_all(&self.pool)
        .await?;
        for (key, dk, job_id) in deadlines {
            if self
                .emit("TimerFired", &dk, &key, json!({ "job_id": job_id }))
                .await?
            {
                stats.deadlines += 1;
            }
        }
        stats.dependencies = self.sweep_dependencies().await?;
        Ok(stats)
    }

    /// Re-evaluate change requests whose dependencies changed since they last looked: a
    /// dependency was completed or closed, or a relationship was added or removed. Emits
    /// `DependenciesChanged` when the open set differs from the one in `state_data`, so a
    /// finished dependency continues its dependents without a human, and a missed webhook
    /// delays that by one sweep at most.
    pub async fn sweep_dependencies(&self) -> Result<usize, Error> {
        let rows: Vec<(String, serde_json::Value, i64)> = sqlx::query_as(
            "SELECT w.key, coalesce(w.state_data->'open_dependencies', '[]'::jsonb),
                    (extract(epoch FROM w.updated_at) * 1000000)::bigint
             FROM work_items w
             WHERE w.workflow = 'change_request'
               AND w.state NOT IN ('Merged', 'Closed', 'Cancelled')
               AND (jsonb_array_length(CASE WHEN jsonb_typeof(w.state_data->'open_dependencies') = 'array'
                                            THEN w.state_data->'open_dependencies' ELSE '[]'::jsonb END) > 0
                    OR EXISTS (SELECT 1 FROM workflows.effective_dependencies e
                               WHERE e.work_item_key = w.key))
             ORDER BY w.id LIMIT $1",
        )
        .bind(BATCH)
        .fetch_all(&self.pool)
        .await?;
        let mut emitted = 0;
        for (key, stored, updated) in rows {
            let Ok(item) = key.parse::<WorkItemKey>() else {
                continue;
            };
            let mut conn = self.pool.acquire().await?;
            let open = open_dependencies(&mut conn, &item).await?;
            drop(conn);
            if serde_json::to_value(&open).unwrap_or_default() == stored {
                continue;
            }
            // One event per (item, state version): a rejected one is not repeated until the
            // item changes.
            let dk = format!("deps:{key}:{updated}");
            if self
                .emit("DependenciesChanged", &dk, &key, json!({}))
                .await?
            {
                emitted += 1;
            }
        }
        Ok(emitted)
    }

    /// GitHub-native dependencies, at most once per [`DEPENDENCY_SYNC_INTERVAL`]. Returns
    /// `None` when disabled or not yet due, else how many work items changed.
    pub async fn sync_dependencies(&self) -> Result<Option<usize>, Error> {
        let Some(sync) = &self.dep_sync else {
            return Ok(None);
        };
        {
            let mut last = sync.last_run.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < DEPENDENCY_SYNC_INTERVAL) {
                return Ok(None);
            }
            *last = Some(Instant::now());
        }
        Ok(Some(
            crate::dependencies::sync_github(&self.pool, &sync.connectors, &sync.index).await?,
        ))
    }

    async fn emit(
        &self,
        event_type: &str,
        dedupe: &str,
        key: &str,
        payload: serde_json::Value,
    ) -> Result<bool, Error> {
        let id: Option<i64> =
            sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
                .bind("engine")
                .bind(dedupe)
                .bind(key)
                .bind(event_type)
                .bind(payload)
                .fetch_one(&self.pool)
                .await?;
        Ok(id.is_some())
    }

    /// Base-head fan-out, at most once per [`BASE_POLL_INTERVAL`]. Returns `None` when
    /// disabled or not yet due, else the number of `MergeConflictDetected` events created.
    ///
    /// For each distinct (repo, base ref) of open enzarb-authored change requests it looks
    /// up the base head. When the head moved since the last pass, each such PR gets a
    /// readiness check and only an explicit `mergeable == Some(false)` emits, with the
    /// looked-up head as `base_sha`. Failures are logged and skipped; a head counts as seen
    /// only once every PR on it had a definite answer, so the rest are retried next pass.
    pub async fn poll_base_heads(&self) -> Result<Option<usize>, Error> {
        let Some(bp) = &self.base_poll else {
            return Ok(None);
        };
        {
            let mut last = bp.last_run.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < BASE_POLL_INTERVAL) {
                return Ok(None);
            }
            *last = Some(Instant::now());
        }

        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT key, state_data->>'base_ref', (state_data->>'number')::bigint
             FROM work_items
             WHERE workflow = 'change_request' AND state = ANY($1)
               AND lower(state_data->>'author') = lower($2)
               AND coalesce(state_data->>'base_ref', '') <> ''
               AND state_data->>'number' ~ '^[0-9]+$'
             ORDER BY id",
        )
        .bind(CONFLICT_PHASES)
        .bind(&bp.bot_login)
        .fetch_all(&self.pool)
        .await?;

        let mut groups: Vec<BaseGroup> = Vec::new();
        for (key, base_ref, number) in rows {
            let Ok(item) = key.parse::<WorkItemKey>() else {
                continue;
            };
            let gk = (item.repo.to_string(), base_ref);
            let entry = (item, number as u64);
            match groups.iter_mut().find(|(k, _)| *k == gk) {
                Some((_, items)) => items.push(entry),
                None => groups.push((gk, vec![entry])),
            }
        }

        let mut emitted = 0;
        for (seen_key, items) in groups {
            let (repo_s, base_ref) = &seen_key;
            let repo = items[0].0.repo.clone();
            let Some(conn) = bp.connectors.get(&repo.connector) else {
                continue;
            };
            let head = match conn.branch_head(&repo, base_ref).await {
                Ok(Some(h)) if !h.is_empty() => h,
                Ok(_) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, repo = %repo_s, base = %base_ref, "base head lookup failed");
                    continue;
                }
            };
            let unchanged = bp
                .seen
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(&seen_key)
                .is_some_and(|s| *s == head);
            if unchanged {
                continue;
            }
            let mut resolved = true;
            for (item, number) in &items {
                match conn.get_merge_readiness(item, *number).await {
                    Ok(r) => match r.mergeable {
                        Some(false) => {
                            match emit_merge_conflict(&self.pool, &item.to_string(), &head).await {
                                Ok(true) => emitted += 1,
                                Ok(false) => {}
                                Err(e) => {
                                    resolved = false;
                                    tracing::warn!(error = %e, item = %item, "emitting MergeConflictDetected failed");
                                }
                            }
                        }
                        Some(true) => {}
                        None => resolved = false,
                    },
                    Err(e) => {
                        resolved = false;
                        tracing::warn!(error = %e, item = %item, "merge readiness check failed");
                    }
                }
            }
            if resolved {
                bp.seen
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(seen_key, head);
            }
        }
        Ok(Some(emitted))
    }

    /// Tick every `interval` until `shutdown` flips to true.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut warned_missing = false;
        loop {
            match grants::reconcile_realtime(&self.pool).await {
                Ok(true) => tracing::info!("granted enzarb_realtime its read access"),
                Ok(false) => {}
                Err(e) if grants::is_missing_function(&e) => {
                    if !warned_missing {
                        warned_missing = true;
                        tracing::warn!("realtime grants reconciler missing; run `migrate` first");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "realtime grants reconcile failed"),
            }
            if let Err(e) = self.tick().await {
                tracing::warn!(error = %e, "ticker sweep failed");
            }
            if let Err(e) = self.poll_base_heads().await {
                tracing::warn!(error = %e, "base head poll failed");
            }
            if let Err(e) = self.sync_dependencies().await {
                tracing::warn!(error = %e, "dependency sync failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = shutdown.changed() => {}
            }
            if *shutdown.borrow() {
                return;
            }
        }
    }
}
