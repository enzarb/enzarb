//! The reducer: one transaction per event (workflows.md "Reducer").
//!
//! Per event:
//! 1. **Before the transaction:** parse the event, find its Project through the
//!    [`ProjectIndex`], and resolve the actor's permission through the
//!    [`PermissionResolver`] (the only I/O a decision needs).
//! 2. **Transaction:** claim the event (`FOR UPDATE SKIP LOCKED`), take a per-work-item
//!    advisory lock (so events of one item are serialized even before its row exists, and
//!    never overtake an earlier unprocessed event of the same item), lock the work item row,
//!    run [`machine::transition`], then persist state, effects, a `transitions` row, mark
//!    the event processed and `NOTIFY work_item_changed`.
//!
//! Rules:
//! * Repo keys are lowercased before they are matched or stored (`work_items.key` is the
//!   lowercase work item key).
//! * An event for a repository in no Project is delivered to an existing work item by its key
//!   (`Ctx.orphaned`: only close/merge, owner cancel and repo removal are accepted).
//! * An event with no Project and no work item, an unparseable event, or one that cannot start a workflow
//!   (nothing routes it, or the first transition rejects it) is marked processed with a
//!   `to_state = 'ignored'` transition that has no work item. No work item is created.
//! * The work item row is inserted only after [`machine::route`] returned a workflow and the
//!   first transition accepted the event.
//! * Effects apply in order: `CancelJobs` cancels queued/running jobs of the item, `Job`
//!   inserts (`ON CONFLICT (idempotency_key) DO NOTHING`), `SetWakeAt`, and `StartWorkflow`
//!   inserts the handoff event (`handoff:<event.id>`) in the same transaction.
//! * A rejected event gets a transition row with `note`; an accepted no-op (same state,
//!   same data, no effects) gets none.
//! * Infrastructure errors (database, permission lookup, unreadable stored state) roll the
//!   transaction back and leave the event unprocessed to be retried; later events of that
//!   work item wait behind it.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use enzarb_connector::epic::parse_epic_marker;
use enzarb_connector::{Actor, EventType, ItemKind, Permission, RepoKey, WorkItemKey};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::{Notify, watch};

use crate::db::{Event as EventRow, WorkItem};
use crate::definitions::DefinitionSource;
use crate::error::Error;
use crate::index::{ProjectIndex, ProjectInfo};
use crate::machine::graph::GraphSource;
use crate::machine::{
    self, CrPhase, Ctx, Effect, EngineEvent, EpicParent, EventPayload, Triggers, WORKFLOW_VERSION,
    WorkflowState,
};
use crate::permission::PermissionResolver;

/// How long the reducer waits for a graph workflow definition before it falls back to the
/// compiled machine.
pub const DEFINITION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct ReducerConfig {
    /// `ENZARB_BOT_LOGIN`.
    pub bot_login: String,
    /// `ENZARB_BOT_HANDLE`.
    pub bot_handle: String,
    pub machine: machine::Config,
    /// Concurrent workers in [`Reducer::run`].
    pub workers: usize,
    /// Fallback poll when no `workflow_events` notification arrives.
    pub poll_interval: Duration,
    /// Unprocessed events looked at per pass.
    pub batch: i64,
}

impl Default for ReducerConfig {
    fn default() -> Self {
        Self {
            bot_login: "enzarb[bot]".into(),
            bot_handle: "enzarb".into(),
            machine: machine::Config::default(),
            workers: 4,
            poll_interval: Duration::from_secs(2),
            batch: 32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Processed,
    /// Claimed elsewhere, or waiting behind an earlier event of the same item.
    Skipped,
}

pub struct Reducer {
    pool: PgPool,
    index: Arc<dyn ProjectIndex>,
    permissions: Arc<dyn PermissionResolver>,
    cfg: ReducerConfig,
    /// Reads graph workflow definitions; `None` keeps every repository on the compiled
    /// machines.
    definitions: Option<Arc<dyn DefinitionSource>>,
}

/// Everything resolved before the transaction opens.
struct Prepared {
    key: Result<WorkItemKey, String>,
    event: Result<EngineEvent, String>,
    project: Option<ProjectInfo>,
    permission: Permission,
    /// The issue graph on the default branch, for an issue-opening event (ADR-0020).
    graph: Option<GraphSource>,
}

/// Lowercase every part of a work item key (GitHub names are case-insensitive).
pub fn normalize_key(key: &WorkItemKey) -> WorkItemKey {
    WorkItemKey::new(
        RepoKey::new(
            key.repo.connector.to_ascii_lowercase(),
            key.repo.owner.to_ascii_lowercase(),
            key.repo.repo.to_ascii_lowercase(),
        ),
        key.number,
    )
}

fn actor_of(ev: &EngineEvent) -> Option<&Actor> {
    match &ev.payload {
        EventPayload::Connector(e) => Some(&e.data.actor),
        EventPayload::Ui(u) => Some(&u.actor),
        _ => None,
    }
}

impl Reducer {
    pub fn new(
        pool: PgPool,
        index: Arc<dyn ProjectIndex>,
        permissions: Arc<dyn PermissionResolver>,
        cfg: ReducerConfig,
    ) -> Self {
        Self {
            pool,
            index,
            permissions,
            cfg,
            definitions: None,
        }
    }

    /// Read graph workflow definitions (`.enzarb/workflows/issue.yaml`) through `source`.
    pub fn with_definitions(mut self, source: Arc<dyn DefinitionSource>) -> Self {
        self.definitions = Some(source);
        self
    }

    /// The issue graph of the event's repository, for an issue-opening event in a Project
    /// with issue triggers. Any error or a slow provider ([`DEFINITION_TIMEOUT`]) is logged
    /// and the repository keeps the compiled machine: a provider outage never stalls events.
    async fn graph_definition(
        &self,
        key: &WorkItemKey,
        ev: &EngineEvent,
        project: &ProjectInfo,
    ) -> Option<GraphSource> {
        let source = self.definitions.as_ref()?;
        let EventPayload::Connector(e) = &ev.payload else {
            return None;
        };
        if e.event_type != EventType::WorkItemOpened
            || e.data.item.kind != ItemKind::Issue
            || !project.triggers.issues
        {
            return None;
        }
        let fetch = source.fetch(&key.repo, &project.default_branch, "issue");
        match tokio::time::timeout(DEFINITION_TIMEOUT, fetch).await {
            Ok(Ok(g)) => g,
            Ok(Err(e)) => {
                tracing::warn!(item = %key, error = %e,
                    "reading the workflow definition failed; using the built-in workflow");
                None
            }
            Err(_) => {
                tracing::warn!(item = %key,
                    "reading the workflow definition timed out; using the built-in workflow");
                None
            }
        }
    }

    /// One pass over the oldest unprocessed events, in id order. Returns how many were
    /// processed. Errors on single events are logged and leave that event for a retry.
    pub async fn pass(&self) -> usize {
        let rows: Vec<EventRow> = match sqlx::query_as(
            "SELECT id, source, dedupe_key, work_item_key, type, payload, received_at, processed_at
             FROM events WHERE processed_at IS NULL ORDER BY id LIMIT $1",
        )
        .bind(self.cfg.batch)
        .fetch_all(&self.pool)
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(error = %e, "reducer: listing events failed");
                return 0;
            }
        };
        let mut done = 0;
        for row in rows {
            let id = row.id;
            match self.process(row).await {
                Ok(Outcome::Processed) => done += 1,
                Ok(Outcome::Skipped) => {}
                Err(e) => {
                    tracing::warn!(event_id = id, error = %e, "reducer: event failed; will retry")
                }
            }
        }
        done
    }

    /// Run passes until nothing more can be processed. Returns the total processed.
    pub async fn drain(&self) -> usize {
        let mut total = 0;
        loop {
            let n = self.pass().await;
            if n == 0 {
                return total;
            }
            total += n;
        }
    }

    /// Run `cfg.workers` workers until `shutdown` flips to true. Workers wake on
    /// `NOTIFY workflow_events` and otherwise poll every `cfg.poll_interval`.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) -> Result<(), Error> {
        let wake = Arc::new(Notify::new());
        let mut listener = sqlx::postgres::PgListener::connect_with(&self.pool).await?;
        listener.listen("workflow_events").await?;

        let mut tasks = tokio::task::JoinSet::new();
        {
            let wake = wake.clone();
            let mut shutdown = shutdown.clone();
            tasks.spawn(async move {
                loop {
                    tokio::select! {
                        r = listener.recv() => match r {
                            Ok(_) => wake.notify_waiters(),
                            Err(e) => {
                                tracing::warn!(error = %e, "reducer: listener error");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        },
                        _ = shutdown.changed() => {}
                    }
                    if *shutdown.borrow() {
                        return;
                    }
                }
            });
        }
        for _ in 0..self.cfg.workers.max(1) {
            let me = self.clone();
            let wake = wake.clone();
            let mut shutdown = shutdown.clone();
            tasks.spawn(async move {
                while !*shutdown.borrow() {
                    if me.pass().await > 0 {
                        continue;
                    }
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = tokio::time::sleep(me.cfg.poll_interval) => {}
                        _ = shutdown.changed() => {}
                    }
                }
            });
        }
        while tasks.join_next().await.is_some() {}
        Ok(())
    }

    async fn prepare(&self, row: &EventRow) -> Result<Prepared, Error> {
        let key = row
            .work_item_key
            .parse::<WorkItemKey>()
            .map(|k| normalize_key(&k))
            .map_err(|e| e.to_string());
        let event = EngineEvent::from_row(row.id.to_string(), &row.r#type, &row.payload)
            .map_err(|e| e.to_string());
        let project = key
            .as_ref()
            .ok()
            .and_then(|k| self.index.lookup(&k.repo.to_string()));
        let mut permission = Permission::None;
        if let (Ok(k), Ok(ev), Some(_)) = (&key, &event, &project)
            && let Some(actor) = actor_of(ev)
            && !actor.is_bot
        {
            permission = self.permissions.permission(&k.repo, actor).await?;
        }
        let mut graph = None;
        if let (Ok(k), Ok(ev), Some(p)) = (&key, &event, &project) {
            graph = self.graph_definition(k, ev, p).await;
        }
        Ok(Prepared {
            key,
            event,
            project,
            permission,
            graph,
        })
    }

    async fn process(&self, row: EventRow) -> Result<Outcome, Error> {
        let prep = self.prepare(&row).await?;
        let mut tx = self.pool.begin().await?;

        let claimed: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM events WHERE id = $1 AND processed_at IS NULL FOR UPDATE SKIP LOCKED",
        )
        .bind(row.id)
        .fetch_optional(&mut *tx)
        .await?;
        if claimed.is_none() {
            return Ok(Outcome::Skipped);
        }

        let (key, ev) = match (&prep.key, &prep.event) {
            (Ok(k), Ok(e)) => (k.clone(), e),
            (k, e) => {
                let why = k
                    .as_ref()
                    .err()
                    .or(e.as_ref().err())
                    .cloned()
                    .unwrap_or_default();
                ignore(&mut tx, &row, None, &format!("unparseable event: {why}")).await?;
                tx.commit().await?;
                return Ok(Outcome::Processed);
            }
        };
        let key_s = key.to_string();

        // Serialize per work item, whether or not its row exists yet.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(&key_s)
            .execute(&mut *tx)
            .await?;
        let earlier: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM events
                            WHERE id < $1 AND processed_at IS NULL AND lower(work_item_key) = $2)",
        )
        .bind(row.id)
        .bind(&key_s)
        .fetch_one(&mut *tx)
        .await?;
        if earlier {
            return Ok(Outcome::Skipped);
        }

        let existing: Option<WorkItem> = sqlx::query_as(
            "SELECT id, key, project_id, workflow, workflow_version, state, state_data, wake_at,
                    created_at, updated_at FROM work_items WHERE key = $1 FOR UPDATE",
        )
        .bind(&key_s)
        .fetch_optional(&mut *tx)
        .await?;

        // An existing item whose repo left every Project still gets close/cancel events,
        // routed by its key; everything else about it is gated by `Ctx.orphaned`.
        let orphaned = prep.project.is_none();
        let project = match (prep.project.clone(), &existing) {
            (Some(p), _) => p,
            (None, Some(w)) => ProjectInfo {
                name: w.project_id.clone().unwrap_or_default(),
                triggers: Triggers {
                    issues: false,
                    change_requests: false,
                    mentions: false,
                },
                default_branch: "main".into(),
            },
            (None, None) => {
                ignore(&mut tx, &row, None, "no project for repository").await?;
                tx.commit().await?;
                return Ok(Outcome::Processed);
            }
        };
        let orphan_owner = orphaned
            && matches!(&ev.payload, EventPayload::Ui(u)
                if self.index.owner(&project.name)
                    .is_some_and(|o| o.eq_ignore_ascii_case(&u.actor.login)));

        // Dependencies named in the event's body or comment, inside this Project only.
        let changed_deps = crate::dependencies::record_text(
            &mut tx,
            &key,
            ev,
            prep.permission.can_write(),
            |dep| {
                self.index
                    .lookup(&dep.repo.to_string())
                    .is_some_and(|p| p.name == project.name)
            },
        )
        .await?;
        if changed_deps {
            tracing::debug!(item = %key, "text dependencies changed");
        }

        // The change request this item is (or could be) stacked on. An item already stacked
        // keeps its parent; a new or issue item may pick its single open parent.
        let stacked_on: Option<String> = existing
            .as_ref()
            .and_then(|w| w.state_data.get("stacked_on"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let stack_parent =
            if stacked_on.is_some() || existing.as_ref().is_none_or(|w| w.workflow == "issue") {
                crate::dependencies::stack_parent(&mut tx, &key, stacked_on.as_deref()).await?
            } else {
                None
            };

        // The stored epic that a bot-authored issue claims as its parent. Only an opening
        // event of an unknown work item can use it; the machine decides whether to trust it.
        let epic_parent = if existing.is_none() {
            epic_parent(&mut tx, ev, &project.name).await?
        } else {
            None
        };

        let ctx = Ctx {
            epic_parent,
            stack_parent,
            orphaned,
            orphan_owner,
            work_item: key.clone(),
            now: Utc::now(),
            bot_login: self.cfg.bot_login.clone(),
            bot_handle: self.cfg.bot_handle.clone(),
            actor_permission: prep.permission,
            config: self.cfg.machine.clone(),
            triggers: project.triggers,
            default_branch: project.default_branch.clone(),
            open_dependencies: crate::dependencies::open_dependencies(&mut tx, &key).await?,
            graph: prep.graph.clone(),
        };

        let state = match &existing {
            Some(w) => {
                WorkflowState::from_row(&w.workflow, w.workflow_version, &w.state, &w.state_data)
                    .map_err(|e| Error::State(format!("work item {}: {e}", w.key)))?
            }
            None => match machine::route(&ctx, ev) {
                Some(kind) => WorkflowState::New(kind),
                None => {
                    ignore(&mut tx, &row, None, "event starts no workflow").await?;
                    tx.commit().await?;
                    return Ok(Outcome::Processed);
                }
            },
        };

        let t = machine::transition(&ctx, &state, ev);

        let work_item_id = match &existing {
            Some(w) => w.id,
            None if !t.accepted => {
                ignore(&mut tx, &row, None, &t.note).await?;
                tx.commit().await?;
                return Ok(Outcome::Processed);
            }
            None => {
                sqlx::query_scalar(
                    "INSERT INTO work_items (key, project_id, workflow, workflow_version, state, state_data)
                     VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
                )
                .bind(&key_s)
                .bind(&project.name)
                .bind(t.next.workflow_name())
                .bind(WORKFLOW_VERSION)
                .bind(t.next.state_name())
                .bind(t.next.state_data())
                .fetch_one(&mut *tx)
                .await?
            }
        };

        let from_state = existing.as_ref().map(|w| w.state.clone());
        let unchanged = t.next == state && t.effects.is_empty() && existing.is_some();
        let state_changed = existing.is_some() && t.next != state;
        if t.accepted && state_changed {
            sqlx::query(
                "UPDATE work_items SET state = $2, state_data = $3, workflow_version = $4,
                        updated_at = now() WHERE id = $1",
            )
            .bind(work_item_id)
            .bind(t.next.state_name())
            .bind(t.next.state_data())
            .bind(WORKFLOW_VERSION)
            .execute(&mut *tx)
            .await?;
        }

        if t.accepted {
            for effect in &t.effects {
                apply_effect(&mut tx, work_item_id, &project.name, effect).await?;
            }
        }

        if !(t.accepted && unchanged) {
            let effects = if t.accepted {
                serde_json::to_value(&t.effects).unwrap_or_default()
            } else {
                serde_json::json!([])
            };
            sqlx::query(
                "INSERT INTO transitions (work_item_id, from_state, to_state, event_id, event_type, effects, note)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(work_item_id)
            .bind(&from_state)
            .bind(if t.accepted { t.next.state_name() } else { state.state_name() })
            .bind(row.id)
            .bind(&row.r#type)
            .bind(effects)
            .bind(if t.note.is_empty() {
                None
            } else if t.accepted {
                Some(t.note.clone())
            } else {
                Some(format!("rejected: {}", t.note))
            })
            .execute(&mut *tx)
            .await?;
        }

        // A parent change request that merged or closed: its stacked children re-evaluate now
        // (a merge retargets them) instead of at the next sweep.
        if t.accepted
            && state_changed
            && matches!(&t.next, WorkflowState::ChangeRequest(c)
                if matches!(c.phase, CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled))
        {
            let n =
                crate::dependencies::notify_stacked_children(&mut tx, &key, &t.next.state_name())
                    .await?;
            if n > 0 {
                tracing::info!(item = %key, children = n, "stacked change requests notified");
            }
        }

        sqlx::query("UPDATE events SET processed_at = now() WHERE id = $1")
            .bind(row.id)
            .execute(&mut *tx)
            .await?;
        if t.accepted && !unchanged {
            sqlx::query("SELECT pg_notify('work_item_changed', $1)")
                .bind(work_item_id.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(Outcome::Processed)
    }
}

/// The epic named by the marker in the body of an issue-opening event, if that epic is a
/// work item of `project` that is creating or waiting for its sub-issues. Everything else is
/// `None`: no marker, an unknown parent, another Project, another phase.
async fn epic_parent(
    tx: &mut Transaction<'_, Postgres>,
    ev: &EngineEvent,
    project: &str,
) -> Result<Option<EpicParent>, Error> {
    let EventPayload::Connector(e) = &ev.payload else {
        return Ok(None);
    };
    if e.event_type != EventType::WorkItemOpened || e.data.item.kind != ItemKind::Issue {
        return Ok(None);
    }
    let Some((parent, _)) = parse_epic_marker(&e.data.item.body) else {
        return Ok(None);
    };
    let parent = normalize_key(&parent);
    let row: Option<(String, serde_json::Value, Option<String>)> = sqlx::query_as(
        "SELECT state, state_data, project_id FROM work_items WHERE key = $1 AND workflow = 'issue'",
    )
    .bind(parent.to_string())
    .fetch_optional(&mut **tx)
    .await?;
    let Some((state, data, project_id)) = row else {
        return Ok(None);
    };
    if project_id.as_deref() != Some(project)
        || !matches!(state.as_str(), "Decomposing" | "AwaitingChildren")
        || data.get("epic").and_then(|v| v.as_bool()) != Some(true)
    {
        return Ok(None);
    }
    let sub_keys = data
        .get("sub_issues")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.get("key").and_then(|k| k.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(EpicParent {
        key: parent,
        depth: data.get("epic_depth").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
        sub_keys,
    }))
}

/// Mark the event processed with an `ignored` transition (no state change).
async fn ignore(
    tx: &mut Transaction<'_, Postgres>,
    row: &EventRow,
    work_item_id: Option<i64>,
    note: &str,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO transitions (work_item_id, from_state, to_state, event_id, event_type, note)
         VALUES ($1, NULL, 'ignored', $2, $3, $4)",
    )
    .bind(work_item_id)
    .bind(row.id)
    .bind(&row.r#type)
    .bind(note)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE events SET processed_at = now() WHERE id = $1")
        .bind(row.id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn apply_effect(
    tx: &mut Transaction<'_, Postgres>,
    work_item_id: i64,
    project: &str,
    effect: &Effect,
) -> Result<(), Error> {
    match effect {
        Effect::Job {
            kind,
            priority,
            idempotency_key,
            input,
        } => {
            sqlx::query(
                "INSERT INTO jobs (work_item_id, project_id, kind, priority, idempotency_key, input)
                 VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (idempotency_key) DO NOTHING",
            )
            .bind(work_item_id)
            .bind(project)
            .bind(kind.as_str())
            .bind(priority)
            .bind(idempotency_key)
            .bind(input)
            .execute(&mut **tx)
            .await?;
        }
        Effect::CancelJobs => {
            sqlx::query(
                "UPDATE jobs SET status = 'cancelled', lease_owner = NULL, lease_until = NULL,
                        updated_at = now()
                 WHERE work_item_id = $1 AND status IN ('queued', 'running')",
            )
            .bind(work_item_id)
            .execute(&mut **tx)
            .await?;
        }
        Effect::SetWakeAt { at } => {
            sqlx::query("UPDATE work_items SET wake_at = $2 WHERE id = $1")
                .bind(work_item_id)
                .bind(at)
                .execute(&mut **tx)
                .await?;
        }
        Effect::StartWorkflow { key, event, .. } => {
            let key = normalize_key(key).to_string();
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)",
            )
            .bind("engine")
            .bind(format!("handoff:{}", event.id))
            .bind(key)
            .bind(event.event_type.as_str())
            .bind(serde_json::to_value(event.as_ref()).unwrap_or_default())
            .fetch_one(&mut **tx)
            .await?;
        }
    }
    Ok(())
}
