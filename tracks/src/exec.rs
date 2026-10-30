//! Executes `pending` and `running` runs (ADR-0029 section 5, rollout steps 4 and 5): `agent`
//! and `command` nodes in the Project's sandbox, `action` nodes (one GitHub call, see
//! [`crate::actions`]), `wait` nodes (the run parks) and `end` nodes.
//!
//! * **Claiming.** `SELECT ... FOR UPDATE SKIP LOCKED` picks a run whose lease is free or expired
//!   and takes a lease (`lease_owner`, `lease_until`). The driver renews the lease while it works;
//!   a crashed replica's lease expires and another replica resumes the run. Every write to the
//!   run checks `lease_owner`, so a replica that lost its lease cannot overwrite the new holder.
//! * **Steps.** The `node_runs` row (with the sandbox `step_run_id`) is written before
//!   `step_start` is sent. After a restart the driver finds the row and re-attaches with
//!   `step_status`; it only sends `step_start` again when the sandbox does not know the step.
//! * **Actions.** The `node_runs` row with the call's `intent` is written before the GitHub call;
//!   a row found on a resume makes the call a replay that first looks for its own effect.
//! * **Waits.** Reaching a `wait` parks the run (`status = waiting`, no lease, `wait_node_run_id`,
//!   `wait_until`). The router (a matching event) or the timeout sweep ([`Executor::wake_waiting`])
//!   sets `wait_result` and flips the run back to `running`; the driver then feeds the result to
//!   `advance`. Both flips are `UPDATE ... WHERE status = 'waiting'`, so only one wins.
//! * **Transitions.** The node's outcome, the `ez-workflow` `advance` and the run row are written
//!   in one transaction.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use enzarb_proto::{
    EzSpec, Mode, Runtime, Secret, StepExec, StepFinished, StepPhase, StepStart, StepWorkspace,
};
use ez_workflow::expr::shell_quote;
use ez_workflow::schema::EndStatus;
use ez_workflow::{Engine, Rendered, RunState, RunStatus, WorkflowsFile};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use tokio::sync::{Semaphore, watch};

use crate::actions::{ActionError, Actions, Call, NoActions, marker};
use crate::github::{MODES_PATH, RepoSource, SourceError, WORKFLOWS_PATH};
use crate::projects::ProjectLookup;
use crate::sandbox::{ClaudeTokens, Link, LinkError, Sandboxes};

/// Files ez folds into the prompt (`ez-core` prompt.rs `INSTRUCTION_FILES`).
pub const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".ez/INSTRUCTIONS.md"];

#[derive(Debug, Clone)]
pub struct ExecConfig {
    pub concurrency: usize,
    pub poll: Duration,
    pub lease: Duration,
    /// How long a run waits before it is looked at again after a transient problem.
    pub retry_backoff: Duration,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            poll: Duration::from_secs(2),
            lease: Duration::from_secs(30),
            retry_backoff: Duration::from_secs(15),
        }
    }
}

/// Why a drive ended without finishing the run.
#[derive(Debug)]
enum Stop {
    /// Try again later; the run keeps its state.
    Retry(String),
    /// Another replica holds the run now.
    LostLease,
}

impl From<sqlx::Error> for Stop {
    fn from(e: sqlx::Error) -> Self {
        Stop::Retry(format!("database: {e}"))
    }
}
impl From<LinkError> for Stop {
    fn from(e: LinkError) -> Self {
        Stop::Retry(e.to_string())
    }
}
impl From<SourceError> for Stop {
    fn from(e: SourceError) -> Self {
        Stop::Retry(e.to_string())
    }
}

pub struct Executor {
    pool: PgPool,
    source: Arc<dyn RepoSource>,
    projects: Arc<dyn ProjectLookup>,
    sandboxes: Arc<dyn Sandboxes>,
    tokens: Arc<dyn ClaudeTokens>,
    actions: Arc<dyn Actions>,
    engine: Engine,
    cfg: ExecConfig,
    me: String,
}

/// The files of the run's pinned commit.
struct Pinned {
    file: WorkflowsFile,
    modes_toml: Option<String>,
    instructions: BTreeMap<String, String>,
}

struct RunRow {
    id: String,
    workflow: String,
    project: String,
    repo: String,
    sha: String,
    status: String,
    state: Value,
    stop_requested: bool,
    has_deadline: bool,
    expired: bool,
    wait_result: Option<Value>,
}

impl Executor {
    pub fn new(
        pool: PgPool,
        source: Arc<dyn RepoSource>,
        projects: Arc<dyn ProjectLookup>,
        sandboxes: Arc<dyn Sandboxes>,
        tokens: Arc<dyn ClaudeTokens>,
        cfg: ExecConfig,
    ) -> Self {
        Self {
            pool,
            source,
            projects,
            sandboxes,
            tokens,
            actions: Arc::new(NoActions),
            engine: Engine::new(),
            cfg,
            me: format!("tracks-{}", std::process::id()) + "-" + &uuid_like(),
        }
    }

    /// Executes `action` nodes through `actions` (default: they fail, no GitHub App).
    pub fn with_actions(mut self, actions: Arc<dyn Actions>) -> Self {
        self.actions = actions;
        self
    }

    /// Claims and drives runs until `stop` flips. Several replicas may run this.
    pub async fn run(self: Arc<Self>, mut stop: watch::Receiver<bool>) {
        let slots = Arc::new(Semaphore::new(self.cfg.concurrency));
        tracing::info!(replica = %self.me, concurrency = self.cfg.concurrency, "run executor on");
        loop {
            if *stop.borrow() {
                break;
            }
            let permit = tokio::select! {
                p = slots.clone().acquire_owned() => p.expect("semaphore"),
                _ = stop.changed() => continue,
            };
            if let Err(e) = self.wake_waiting().await {
                tracing::warn!(error = %e, "waking waiting runs failed");
            }
            match self.claim().await {
                Ok(Some(id)) => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        me.drive(&id).await;
                        drop(permit);
                    });
                }
                Ok(None) => {
                    drop(permit);
                    tokio::select! {
                        _ = tokio::time::sleep(self.cfg.poll) => {},
                        _ = stop.changed() => {},
                    }
                }
                Err(e) => {
                    drop(permit);
                    tracing::warn!(error = %e, "claiming a run failed");
                    tokio::time::sleep(self.cfg.poll).await;
                }
            }
        }
        // Wait for in-flight drives to notice the stop (they only end at a step boundary or
        // when their lease is lost); leases of unfinished runs expire and others resume them.
        let _ = tokio::time::timeout(
            Duration::from_secs(20),
            slots.acquire_many(self.cfg.concurrency as u32),
        )
        .await;
    }

    /// Wakes parked runs whose wait timed out (outputs `timed_out = true`), and ones that were
    /// stopped or passed their run deadline (the driver then ends them). Returns how many.
    pub async fn wake_waiting(&self) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(
            "UPDATE tracks.runs SET status = 'running', updated_at = now(), wait_until = NULL,
                    wait_result = CASE
                        WHEN stop_requested OR (deadline IS NOT NULL AND deadline < now()) THEN NULL
                        ELSE jsonb_build_object(
                            'outputs', jsonb_build_object('event', 'null'::jsonb, 'timed_out', true),
                            'event_id', 'null'::jsonb) END
              WHERE status = 'waiting' AND finished_at IS NULL
                AND (stop_requested OR (wait_until IS NOT NULL AND wait_until < now())
                     OR (deadline IS NOT NULL AND deadline < now()))",
        )
        .execute(&self.pool)
        .await?;
        Ok(r.rows_affected())
    }

    /// Takes the lease of the oldest claimable run.
    pub async fn claim(&self) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar(
            "UPDATE tracks.runs SET lease_owner = $1, lease_until = now() + make_interval(secs => $2)
              WHERE id = (SELECT id FROM tracks.runs
                           WHERE finished_at IS NULL AND status IN ('pending', 'running')
                             AND (lease_until IS NULL OR lease_until < now())
                           ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED)
          RETURNING id::text",
        )
        .bind(&self.me)
        .bind(self.cfg.lease.as_secs_f64())
        .fetch_optional(&self.pool)
        .await
    }

    async fn release(&self, id: &str, backoff: Duration) {
        let _ = sqlx::query(
            "UPDATE tracks.runs SET lease_until = now() + make_interval(secs => $3)
              WHERE id = $1::uuid AND lease_owner = $2",
        )
        .bind(id)
        .bind(&self.me)
        .bind(backoff.as_secs_f64())
        .execute(&self.pool)
        .await;
    }

    /// Drives a claimed run until it ends, waits for retry, or the lease is lost.
    pub async fn drive(&self, id: &str) {
        let (lost_tx, lost_rx) = watch::channel(false);
        let hb = {
            let (pool, me, id, lease) = (
                self.pool.clone(),
                self.me.clone(),
                id.to_string(),
                self.cfg.lease,
            );
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(lease / 3).await;
                    let r = sqlx::query(
                        "UPDATE tracks.runs SET lease_until = now() + make_interval(secs => $3)
                          WHERE id = $1::uuid AND lease_owner = $2 AND finished_at IS NULL",
                    )
                    .bind(&id)
                    .bind(&me)
                    .bind(lease.as_secs_f64())
                    .execute(&pool)
                    .await;
                    match r {
                        Ok(r) if r.rows_affected() == 0 => {
                            let _ = lost_tx.send(true);
                            return;
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!(run = %id, error = %e, "lease renewal failed"),
                    }
                }
            })
        };
        let res = self.drive_inner(id, lost_rx).await;
        hb.abort();
        match res {
            Ok(()) => {}
            Err(Stop::LostLease) => tracing::warn!(run = id, "lease lost; another replica has it"),
            Err(Stop::Retry(why)) => {
                tracing::warn!(run = id, reason = %why, "run paused; retrying later");
                self.release(id, self.cfg.retry_backoff).await;
            }
        }
    }

    async fn drive_inner(&self, id: &str, lost: watch::Receiver<bool>) -> Result<(), Stop> {
        let mut pinned: Option<Pinned> = None;
        loop {
            if *lost.borrow() {
                return Err(Stop::LostLease);
            }
            let run = self.load(id).await?;
            let mut state: RunState = match serde_json::from_value(run.state.clone()) {
                Ok(s) => s,
                Err(e) => {
                    return self
                        .end_run(
                            &run,
                            None,
                            EndStatus::Failed,
                            format!("corrupt run state: {e}"),
                        )
                        .await;
                }
            };
            if state.is_ended() {
                return self.write_state(&run, &state).await;
            }
            if run.stop_requested {
                return self
                    .end_run(&run, Some(state), EndStatus::Cancelled, "stopped".into())
                    .await;
            }
            if run.expired {
                return self
                    .end_run(&run, Some(state), EndStatus::Failed, "timeout".into())
                    .await;
            }
            if pinned.is_none() {
                match self.pin(&run).await {
                    Ok(p) => pinned = Some(p),
                    Err(PinError::Retry(s)) => return Err(Stop::Retry(s)),
                    Err(PinError::Fatal(m)) => {
                        return self.end_run(&run, Some(state), EndStatus::Failed, m).await;
                    }
                }
            }
            let pin = pinned.as_ref().expect("pinned");
            if !run.has_deadline {
                let secs = pin
                    .file
                    .workflows
                    .get(&run.workflow)
                    .map(|w| w.run_timeout().as_secs_f64())
                    .unwrap_or(14.0 * 86400.0);
                sqlx::query(
                    "UPDATE tracks.runs SET deadline = created_at + make_interval(secs => $2)
                      WHERE id = $1::uuid AND deadline IS NULL",
                )
                .bind(id)
                .bind(secs)
                .execute(&self.pool)
                .await?;
            }
            if run.status == "pending" && !self.start_allowed(&run, pin).await? {
                tracing::info!(run = id, workflow = %run.workflow, "max_runs reached; run stays pending");
                self.release(id, self.cfg.retry_backoff).await;
                return Ok(());
            }
            let rendered = match state.render(&self.engine, &pin.file) {
                Ok(Ok(r)) => r,
                Ok(Err(msg)) => {
                    return self
                        .end_run(&run, Some(state), EndStatus::Failed, msg)
                        .await;
                }
                Err(e) => {
                    return self
                        .end_run(&run, Some(state), EndStatus::Failed, e.to_string())
                        .await;
                }
            };
            let node = state.current().unwrap_or_default().to_string();
            match rendered {
                Rendered::Agent { .. } | Rendered::Command { .. } => {
                    let outcome = self
                        .run_step(&run, &mut state, &node, &rendered, pin, lost.clone())
                        .await?;
                    match outcome {
                        StepOutcome::Advanced => {}
                        StepOutcome::Ended => return Ok(()),
                    }
                }
                Rendered::Action { .. } => {
                    match self
                        .action_step(&run, &mut state, &node, &rendered, pin)
                        .await?
                    {
                        StepOutcome::Advanced => {}
                        StepOutcome::Ended => return Ok(()),
                    }
                }
                Rendered::Wait { .. } => {
                    match self
                        .wait_step(&run, &mut state, &node, &rendered, pin)
                        .await?
                    {
                        StepOutcome::Advanced => {}
                        StepOutcome::Ended => return Ok(()),
                    }
                }
                Rendered::End { status } => {
                    return self.end_run(&run, Some(state), status, String::new()).await;
                }
            }
        }
    }

    async fn load(&self, id: &str) -> Result<RunRow, Stop> {
        let r = sqlx::query(
            "SELECT id::text AS id, workflow, project, repo, sha, status, state, stop_requested,
                    deadline IS NOT NULL AS has_deadline, (deadline IS NOT NULL AND deadline < now()) AS expired, wait_result
               FROM tracks.runs WHERE id = $1::uuid",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;
        Ok(RunRow {
            id: r.get("id"),
            workflow: r.get("workflow"),
            project: r.get("project"),
            repo: r.get("repo"),
            sha: r.get("sha"),
            status: r.get("status"),
            state: r.get("state"),
            stop_requested: r.get("stop_requested"),
            has_deadline: r.get("has_deadline"),
            expired: r.get("expired"),
            wait_result: r.get("wait_result"),
        })
    }

    async fn pin(&self, run: &RunRow) -> Result<Pinned, PinError> {
        let src = |e: SourceError| match e {
            SourceError::Transient(m) => PinError::Retry(m),
            SourceError::Permanent(m) => PinError::Fatal(m),
        };
        let text = self
            .source
            .file_at(&run.repo, &run.sha, WORKFLOWS_PATH)
            .await
            .map_err(src)?
            .ok_or_else(|| PinError::Fatal(format!("{WORKFLOWS_PATH} is gone at {}", run.sha)))?;
        let file = ez_workflow::parse(&text)
            .map_err(|e| PinError::Fatal(format!("invalid {WORKFLOWS_PATH}: {e}")))?;
        let modes_toml = self
            .source
            .file_at(&run.repo, &run.sha, MODES_PATH)
            .await
            .map_err(src)?;
        let mut instructions = BTreeMap::new();
        for path in INSTRUCTION_FILES {
            if let Some(c) = self
                .source
                .file_at(&run.repo, &run.sha, path)
                .await
                .map_err(src)?
            {
                instructions.insert(path.to_string(), c);
            }
        }
        Ok(Pinned {
            file,
            modes_toml,
            instructions,
        })
    }

    /// Flips a `pending` run to `running` unless the workflow's `max_runs` is reached for the
    /// Project. The advisory lock makes the check and the flip atomic across replicas.
    async fn start_allowed(&self, run: &RunRow, pin: &Pinned) -> Result<bool, sqlx::Error> {
        let max = pin
            .file
            .workflows
            .get(&run.workflow)
            .map(|w| w.max_runs())
            .unwrap_or(3) as i64;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 1))")
            .bind(format!("{}/{}", run.project, run.workflow))
            .execute(&mut *tx)
            .await?;
        let live: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tracks.runs WHERE project = $1 AND workflow = $2
                AND status = 'running' AND finished_at IS NULL AND NOT test AND id <> $3::uuid",
        )
        .bind(&run.project)
        .bind(&run.workflow)
        .bind(&run.id)
        .fetch_one(&mut *tx)
        .await?;
        let ok = live < max;
        if ok {
            sqlx::query(
                "UPDATE tracks.runs SET status = 'running', updated_at = now()
                  WHERE id = $1::uuid AND lease_owner = $2",
            )
            .bind(&run.id)
            .bind(&self.me)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(ok)
    }

    /// Writes the end of the run (`status`, `reason`) and, when there is one, its state.
    async fn end_run(
        &self,
        run: &RunRow,
        state: Option<RunState>,
        status: EndStatus,
        reason: String,
    ) -> Result<(), Stop> {
        let mut state = state;
        if let Some(s) = state.as_mut() {
            match status {
                EndStatus::Cancelled => s.cancel(reason.clone()),
                EndStatus::Failed => s.fail(reason.clone()),
                EndStatus::Done => {}
            }
        }
        let reason_col = (!reason.is_empty()).then_some(reason.clone());
        let state_json = state.as_ref().and_then(|s| serde_json::to_value(s).ok());
        let r = sqlx::query(
            "UPDATE tracks.runs SET status = $3, reason = $4, finished_at = now(), updated_at = now(),
                    state = COALESCE($5, state), lease_until = NULL
              WHERE id = $1::uuid AND lease_owner = $2 AND finished_at IS NULL",
        )
        .bind(&run.id)
        .bind(&self.me)
        .bind(status.as_str())
        .bind(&reason_col)
        .bind(state_json)
        .execute(&self.pool)
        .await?;
        if r.rows_affected() == 0 {
            return Err(Stop::LostLease);
        }
        tracing::info!(run = %run.id, workflow = %run.workflow, status = status.as_str(), reason = %reason, "run ended");
        Ok(())
    }

    /// Persists the state of a run `advance` moved to its end node.
    async fn write_state(&self, run: &RunRow, state: &RunState) -> Result<(), Stop> {
        match &state.status {
            RunStatus::Ended { status, reason, .. } => {
                self.end_run(
                    run,
                    Some(state.clone()),
                    *status,
                    reason.clone().unwrap_or_default(),
                )
                .await
            }
            RunStatus::At { .. } => Ok(()),
        }
    }

    // ----- actions and waits -----

    /// Runs an `action` node: intent first, then the GitHub call (a replay looks for its effect
    /// first), then the outputs and the transition in one transaction.
    async fn action_step(
        &self,
        run: &RunRow,
        state: &mut RunState,
        node: &str,
        rendered: &Rendered,
        pin: &Pinned,
    ) -> Result<StepOutcome, Stop> {
        let Rendered::Action {
            action,
            title,
            body,
            labels,
            draft,
        } = rendered
        else {
            return Err(Stop::Retry("not an action".into()));
        };
        let visit = *state.visits.get(node).unwrap_or(&1) as i32;
        let mark = marker(&run.id, node, visit);
        let test = state.run.test;
        let branch = if test {
            format!("ez-test/{}", run.id)
        } else {
            format!("ez/{}/{}", run.workflow, run.id)
        };
        let base = match self.projects.owner(&run.repo).await {
            Ok(Some(p)) => p.default_branch,
            Ok(None) => {
                let m = format!("no Project owns {}", run.repo);
                return self.fail_action(run, state, node, visit, rendered, m).await;
            }
            Err(e) => return Err(Stop::Retry(e)),
        };
        let intent = json!({
            "action": action.as_str(), "repo": run.repo, "branch": branch, "base": base,
            "subject": state.subject, "marker": mark, "title": title, "body": body,
            "labels": labels, "draft": draft, "test": test,
        });
        // The intent is durable before the call: a row found later means the call may have run.
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO tracks.node_runs (run_id, node, visit, status, inputs, rendered, intent, started_at)
             VALUES ($1::uuid, $2, $3, 'running', $4, $4, $5, now())
             ON CONFLICT (run_id, node, visit) DO NOTHING RETURNING id",
        )
        .bind(&run.id)
        .bind(node)
        .bind(visit)
        .bind(serde_json::to_value(rendered).unwrap_or(Value::Null))
        .bind(&intent)
        .fetch_optional(&self.pool)
        .await?;
        let replay = inserted.is_none();
        let node_run_id: i64 = match inserted {
            Some(id) => id,
            None => {
                sqlx::query_scalar(
                    "SELECT id FROM tracks.node_runs WHERE run_id = $1::uuid AND node = $2 AND visit = $3",
                )
                .bind(&run.id)
                .bind(node)
                .bind(visit)
                .fetch_one(&self.pool)
                .await?
            }
        };

        let outputs = if test {
            // Test runs change nothing outside the sandbox: record the call (ADR-0029 section 6).
            json!({
                "number": 9_000_000 + visit as u64,
                "url": format!("https://github.com/{}/test-run/{}/{node}", run.repo, run.id),
                "test": true,
                "call": intent,
            })
        } else {
            let call = Call {
                repo: &run.repo,
                base: &base,
                branch: &branch,
                subject: state.subject.as_ref(),
                action: *action,
                title: title.as_deref(),
                body: body.as_deref(),
                labels,
                draft: *draft,
                marker: &mark,
                replay,
            };
            match self.actions.perform(&call).await {
                Ok(o) => o,
                Err(ActionError::Transient(m)) => return Err(Stop::Retry(m)),
                Err(ActionError::Permanent(m)) => {
                    return self.fail_action(run, state, node, visit, rendered, m).await;
                }
            }
        };

        let mut next = state.clone();
        if let Err(e) = next.advance(&self.engine, &pin.file, outputs.clone()) {
            return self
                .fail_action(run, state, node, visit, rendered, e.to_string())
                .await;
        }
        tracing::info!(run = %run.id, node, action = action.as_str(), replay, "action done");
        self.finish_local(run, node_run_id, "done", &outputs, None, None, Some(&next))
            .await?;
        *state = next;
        Ok(if state.is_ended() {
            StepOutcome::Ended
        } else {
            StepOutcome::Advanced
        })
    }

    async fn fail_action(
        &self,
        run: &RunRow,
        state: &RunState,
        node: &str,
        visit: i32,
        rendered: &Rendered,
        msg: String,
    ) -> Result<StepOutcome, Stop> {
        self.record_failed_node(run, node, visit, rendered, &msg)
            .await?;
        self.end_run(run, Some(state.clone()), EndStatus::Failed, msg)
            .await?;
        Ok(StepOutcome::Ended)
    }

    /// A `wait` node. Without a `wait_result` the run parks; with one (set by the router or the
    /// timeout sweep) the node finishes and the run advances.
    async fn wait_step(
        &self,
        run: &RunRow,
        state: &mut RunState,
        node: &str,
        rendered: &Rendered,
        pin: &Pinned,
    ) -> Result<StepOutcome, Stop> {
        let Rendered::Wait {
            on,
            condition,
            timeout_secs,
        } = rendered
        else {
            return Err(Stop::Retry("not a wait".into()));
        };
        let visit = *state.visits.get(node).unwrap_or(&1) as i32;
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM tracks.node_runs WHERE run_id = $1::uuid AND node = $2 AND visit = $3",
        )
        .bind(&run.id)
        .bind(node)
        .bind(visit)
        .fetch_optional(&self.pool)
        .await?;
        if let (Some(res), Some(nid)) = (&run.wait_result, existing) {
            let outputs = res.get("outputs").cloned().unwrap_or(Value::Null);
            let timed_out = outputs.get("timed_out").and_then(Value::as_bool) == Some(true);
            let wait = json!({ "resumed_by_event": res.get("event_id"), "timed_out": timed_out });
            let mut next = state.clone();
            if let Err(e) = next.advance(&self.engine, &pin.file, outputs.clone()) {
                let why = e.to_string();
                self.finish_local(run, nid, "failed", &outputs, Some(&why), Some(&wait), None)
                    .await?;
                self.end_run(run, Some(state.clone()), EndStatus::Failed, why)
                    .await?;
                return Ok(StepOutcome::Ended);
            }
            tracing::info!(run = %run.id, node, timed_out, "wait finished");
            self.finish_local(run, nid, "done", &outputs, None, Some(&wait), Some(&next))
                .await?;
            *state = next;
            return Ok(if state.is_ended() {
                StepOutcome::Ended
            } else {
                StepOutcome::Advanced
            });
        }

        // Park: the node row, the wait's deadline and the status in one transaction.
        let mut tx = self.pool.begin().await?;
        let nid: i64 = sqlx::query_scalar(
            "INSERT INTO tracks.node_runs (run_id, node, visit, status, inputs, rendered, started_at)
             VALUES ($1::uuid, $2, $3, 'waiting', $4, $4, now())
             ON CONFLICT (run_id, node, visit) DO UPDATE SET status = 'waiting'
             RETURNING id",
        )
        .bind(&run.id)
        .bind(node)
        .bind(visit)
        .bind(serde_json::to_value(rendered).unwrap_or(Value::Null))
        .fetch_one(&mut *tx)
        .await?;
        let parked: Option<Option<String>> = sqlx::query_scalar(
            "UPDATE tracks.runs SET status = 'waiting', current_node = $3, wait_node_run_id = $4,
                    wait_result = NULL, updated_at = now(), lease_owner = NULL, lease_until = NULL,
                    wait_until = CASE WHEN $5::float8 IS NULL THEN NULL
                                      ELSE now() + make_interval(secs => $5::float8) END
              WHERE id = $1::uuid AND lease_owner = $2 AND finished_at IS NULL
          RETURNING to_char(wait_until AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.MS\"Z\"')",
        )
        .bind(&run.id)
        .bind(&self.me)
        .bind(node)
        .bind(nid)
        .bind(timeout_secs.map(|s| s as f64))
        .fetch_optional(&mut *tx)
        .await?;
        let Some(until) = parked else {
            tx.rollback().await?;
            return Err(Stop::LostLease);
        };
        sqlx::query("UPDATE tracks.node_runs SET wait = $2 WHERE id = $1")
            .bind(nid)
            .bind(
                json!({ "on": on, "if": condition, "timeout_secs": timeout_secs, "until": until }),
            )
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(run = %run.id, node, "run waits");
        Ok(StepOutcome::Ended)
    }

    /// Finishes a node that ran in tracks and, with `next`, writes the advanced run state, in
    /// one transaction. Also clears any wait bookkeeping of the run.
    #[allow(clippy::too_many_arguments)]
    async fn finish_local(
        &self,
        run: &RunRow,
        node_run_id: i64,
        status: &str,
        outputs: &Value,
        error: Option<&str>,
        wait: Option<&Value>,
        next: Option<&RunState>,
    ) -> Result<(), Stop> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE tracks.node_runs SET status = $2, outputs = $3, error = $4,
                    wait = CASE WHEN $5::jsonb IS NULL THEN wait ELSE COALESCE(wait, '{}'::jsonb) || $5::jsonb END,
                    finished_at = now()
              WHERE id = $1",
        )
        .bind(node_run_id)
        .bind(status)
        .bind(outputs)
        .bind(error)
        .bind(wait)
        .execute(&mut *tx)
        .await?;
        if let Some(next) = next {
            let (st, finished, reason) = match &next.status {
                RunStatus::At { .. } => ("running", false, None),
                RunStatus::Ended { status, reason, .. } => (status.as_str(), true, reason.clone()),
            };
            let r = sqlx::query(
                "UPDATE tracks.runs SET state = $3, visits = $4, current_node = $5, status = $6, reason = $7,
                        wait_node_run_id = NULL, wait_result = NULL, wait_until = NULL,
                        updated_at = now(), finished_at = CASE WHEN $8 THEN now() END,
                        lease_until = CASE WHEN $8 THEN NULL ELSE lease_until END
                  WHERE id = $1::uuid AND lease_owner = $2 AND finished_at IS NULL",
            )
            .bind(&run.id)
            .bind(&self.me)
            .bind(serde_json::to_value(next).unwrap_or(Value::Null))
            .bind(serde_json::to_value(&next.visits).unwrap_or(json!({})))
            .bind(next.current())
            .bind(st)
            .bind(&reason)
            .bind(finished)
            .execute(&mut *tx)
            .await?;
            if r.rows_affected() == 0 {
                tx.rollback().await?;
                return Err(Stop::LostLease);
            }
        }
        tx.commit().await?;
        Ok(())
    }

    // ----- steps -----

    async fn run_step(
        &self,
        run: &RunRow,
        state: &mut RunState,
        node: &str,
        rendered: &Rendered,
        pin: &Pinned,
        lost: watch::Receiver<bool>,
    ) -> Result<StepOutcome, Stop> {
        let visit = *state.visits.get(node).unwrap_or(&1) as i32;
        let existing = sqlx::query(
            "SELECT id, step_run_id, status FROM tracks.node_runs
              WHERE run_id = $1::uuid AND node = $2 AND visit = $3",
        )
        .bind(&run.id)
        .bind(node)
        .bind(visit)
        .fetch_optional(&self.pool)
        .await?;

        let (node_run_id, prev_step, resumed) = match existing {
            Some(r) => {
                let nid: i64 = r.get("id");
                let sid: Option<String> = r.get("step_run_id");
                (nid, sid.unwrap_or_else(|| step_id(&run.id, nid)), true)
            }
            None => (0, String::new(), false),
        };

        // Build the step (also needed to start it again when the sandbox forgot it).
        let build = match self
            .build_step(run, state, rendered, pin, node, visit)
            .await
        {
            Ok(b) => b,
            Err(BuildError::NoCredential) => {
                self.record_failed_node(run, node, visit, rendered, "no_credential")
                    .await?;
                self.end_run(
                    run,
                    Some(state.clone()),
                    EndStatus::Failed,
                    "no_credential".into(),
                )
                .await?;
                return Ok(StepOutcome::Ended);
            }
            Err(BuildError::Fatal(m)) => {
                self.record_failed_node(run, node, visit, rendered, &m)
                    .await?;
                self.end_run(run, Some(state.clone()), EndStatus::Failed, m)
                    .await?;
                return Ok(StepOutcome::Ended);
            }
            Err(BuildError::Retry(m)) => return Err(Stop::Retry(m)),
        };

        let (node_run_id, step_ident) = if resumed {
            (node_run_id, prev_step)
        } else {
            let nid: i64 = sqlx::query_scalar(
                "INSERT INTO tracks.node_runs (run_id, node, visit, status, inputs, rendered, mode, started_at)
                 VALUES ($1::uuid, $2, $3, 'running', $4, $4, $5, now())
                 ON CONFLICT (run_id, node, visit) DO UPDATE SET status = tracks.node_runs.status
                 RETURNING id",
            )
            .bind(&run.id)
            .bind(node)
            .bind(visit)
            .bind(serde_json::to_value(rendered).unwrap_or(Value::Null))
            .bind(match rendered {
                Rendered::Agent { mode, .. } => Some(mode.clone()),
                _ => None,
            })
            .fetch_one(&self.pool)
            .await?;
            let sid = step_id(&run.id, nid);
            sqlx::query("UPDATE tracks.node_runs SET step_run_id = $2 WHERE id = $1")
                .bind(nid)
                .bind(&sid)
                .execute(&self.pool)
                .await?;
            (nid, sid)
        };
        let mut step = build;
        step.run_id = step_ident.clone();

        let link = self
            .sandboxes
            .connect(&run.project)
            .await
            .map_err(|e| match e {
                LinkError::Permanent(m) => Stop::Retry(m),
                e => e.into(),
            })?;
        let fin = self
            .attach_and_wait(run, &*link, step, resumed, lost)
            .await?;
        let Some(fin) = fin else {
            // Stopped or timed out while the step ran: the run was ended by the callee.
            return Ok(StepOutcome::Ended);
        };

        // The outcome and the transition in one transaction.
        let outputs = outputs_of(rendered, &fin);
        let mut next = state.clone();
        let step_failed = fin.exit != enzarb_proto::step::exit::SUCCESS;
        if fin.exit == enzarb_proto::step::exit::INTERRUPTED {
            let why = fin
                .error
                .clone()
                .unwrap_or_else(|| "the step was interrupted".into());
            self.finish_node(run, node_run_id, &fin, &outputs, None)
                .await?;
            if self.load(&run.id).await?.stop_requested {
                self.end_run(run, Some(next), EndStatus::Cancelled, "stopped".into())
                    .await?;
            } else {
                self.end_run(run, Some(next), EndStatus::Failed, why)
                    .await?;
            }
            return Ok(StepOutcome::Ended);
        }
        let tr = next.advance(&self.engine, &pin.file, outputs.clone());
        if let Err(e) = tr {
            self.finish_node(run, node_run_id, &fin, &outputs, None)
                .await?;
            self.end_run(run, Some(state.clone()), EndStatus::Failed, e.to_string())
                .await?;
            return Ok(StepOutcome::Ended);
        }
        tracing::info!(run = %run.id, node, exit = %fin.exit, failed = step_failed, "node finished");
        self.finish_node(run, node_run_id, &fin, &outputs, Some(&next))
            .await?;
        *state = next;
        if state.is_ended() {
            return Ok(StepOutcome::Ended);
        }
        Ok(StepOutcome::Advanced)
    }

    async fn record_failed_node(
        &self,
        run: &RunRow,
        node: &str,
        visit: i32,
        rendered: &Rendered,
        error: &str,
    ) -> Result<(), Stop> {
        sqlx::query(
            "INSERT INTO tracks.node_runs (run_id, node, visit, status, inputs, rendered, error, started_at, finished_at)
             VALUES ($1::uuid, $2, $3, 'failed', $4, $4, $5, now(), now())
             ON CONFLICT (run_id, node, visit) DO UPDATE SET status = 'failed', error = $5, finished_at = now()",
        )
        .bind(&run.id)
        .bind(node)
        .bind(visit)
        .bind(serde_json::to_value(rendered).unwrap_or(Value::Null))
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Writes the node's result and, with `next`, the advanced run state, atomically.
    async fn finish_node(
        &self,
        run: &RunRow,
        node_run_id: i64,
        fin: &StepFinished,
        outputs: &Value,
        next: Option<&RunState>,
    ) -> Result<(), Stop> {
        let mut tx = self.pool.begin().await?;
        let ok = fin.exit == enzarb_proto::step::exit::SUCCESS;
        sqlx::query(
            "UPDATE tracks.node_runs SET status = $2, outputs = $3, usage = $4, error = $5, git = $6, finished_at = now()
              WHERE id = $1",
        )
        .bind(node_run_id)
        .bind(if ok { "done" } else { "failed" })
        .bind(outputs)
        .bind(fin.usage.as_ref().and_then(|u| serde_json::to_value(u).ok()))
        .bind(&fin.error)
        .bind(serde_json::to_value(&fin.git).ok())
        .execute(&mut *tx)
        .await?;
        if let Some(next) = next {
            let (status, finished, reason) = match &next.status {
                RunStatus::At { .. } => ("running", false, None),
                RunStatus::Ended { status, reason, .. } => (status.as_str(), true, reason.clone()),
            };
            let r = sqlx::query(
                "UPDATE tracks.runs SET state = $3, visits = $4, current_node = $5, status = $6, reason = $7,
                        updated_at = now(), finished_at = CASE WHEN $8 THEN now() END,
                        lease_until = CASE WHEN $8 THEN NULL ELSE lease_until END
                  WHERE id = $1::uuid AND lease_owner = $2 AND finished_at IS NULL",
            )
            .bind(&run.id)
            .bind(&self.me)
            .bind(serde_json::to_value(next).unwrap_or(Value::Null))
            .bind(serde_json::to_value(&next.visits).unwrap_or(json!({})))
            .bind(next.current())
            .bind(status)
            .bind(&reason)
            .bind(finished)
            .execute(&mut *tx)
            .await?;
            if r.rows_affected() == 0 {
                tx.rollback().await?;
                return Err(Stop::LostLease);
            }
        }
        tx.commit().await?;
        Ok(())
    }

    /// Starts (or re-attaches to) the step and waits for `step_finished`. `Ok(None)`: the run
    /// was stopped or timed out and has been ended.
    async fn attach_and_wait(
        &self,
        run: &RunRow,
        link: &dyn Link,
        step: StepStart,
        resumed: bool,
        mut lost: watch::Receiver<bool>,
    ) -> Result<Option<StepFinished>, Stop> {
        let sid = step.run_id.clone();
        if resumed {
            let (phase, result) = link.status(&sid).await?;
            tracing::info!(run = %run.id, step = %sid, ?phase, "re-attached to the step");
            match (phase, result) {
                (_, Some(f)) => return Ok(Some(f)),
                (StepPhase::Running, None) => {}
                (StepPhase::Interrupted, None) => link.resume(&sid),
                (StepPhase::Unknown, None) | (StepPhase::Finished, None) => {
                    link.start(step).await?;
                }
            }
        } else {
            link.start(step).await?;
        }
        let mut stopping = false;
        let mut stop_at = None;
        loop {
            tokio::select! {
                fin = link.finished(&sid) => return Ok(Some(fin?)),
                _ = lost.changed() => return Err(Stop::LostLease),
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
            if let Some(at) = stop_at {
                if tokio::time::Instant::now() > at {
                    let reason = if stopping { "stopped" } else { "timeout" };
                    let st = self.load(&run.id).await?;
                    let state: Option<RunState> = serde_json::from_value(st.state.clone()).ok();
                    let status = if reason == "stopped" {
                        EndStatus::Cancelled
                    } else {
                        EndStatus::Failed
                    };
                    self.end_run(&st, state, status, reason.into()).await?;
                    return Ok(None);
                }
                continue;
            }
            let cur = self.load(&run.id).await?;
            if cur.stop_requested || cur.expired {
                stopping = cur.stop_requested;
                tracing::info!(run = %run.id, step = %sid, stopping, "stopping the step");
                link.stop(&sid);
                stop_at = Some(tokio::time::Instant::now() + Duration::from_secs(60));
            }
        }
    }

    async fn build_step(
        &self,
        run: &RunRow,
        state: &RunState,
        rendered: &Rendered,
        pin: &Pinned,
        node: &str,
        visit: i32,
    ) -> Result<StepStart, BuildError> {
        let test = state.run.test;
        let base = match self.projects.owner(&run.repo).await {
            Ok(Some(p)) => p.default_branch,
            Ok(None) => return Err(BuildError::Fatal(format!("no Project owns {}", run.repo))),
            Err(e) => return Err(BuildError::Retry(e)),
        };
        let push = matches!(rendered, Rendered::Agent { push: true, .. }) && !test;
        let token = self
            .source
            .clone_token(&run.repo, push)
            .await
            .map_err(|e| match e {
                SourceError::Transient(m) => BuildError::Retry(m),
                SourceError::Permanent(m) => BuildError::Fatal(m),
            })?;
        let branch = if test {
            format!("ez-test/{}", run.id)
        } else {
            format!("ez/{}/{}", run.workflow, run.id)
        };
        let ws = StepWorkspace {
            repo: run.repo.clone(),
            url: format!("https://github.com/{}.git", run.repo),
            mode: Mode::Write,
            branch,
            base,
            push,
            token: Secret::new(token),
        };
        let mut env = BTreeMap::new();
        let exec = match rendered {
            Rendered::Agent {
                mode,
                instruction,
                result_schema,
                ..
            } => {
                let owner = match self.tokens.owner_of(&run.project).await {
                    Ok(Some(o)) => o,
                    Ok(None) => {
                        return Err(BuildError::Fatal(format!("no Project {}", run.project)));
                    }
                    Err(e) => return Err(BuildError::Retry(e)),
                };
                match self.tokens.token_for(&owner).await {
                    Ok(Some(t)) => {
                        env.insert("CLAUDE_CODE_OAUTH_TOKEN".to_string(), t);
                    }
                    Ok(None) => return Err(BuildError::NoCredential),
                    Err(e) => return Err(BuildError::Retry(e)),
                }
                let Some(modes_toml) = pin.modes_toml.clone() else {
                    return Err(BuildError::Fatal(format!(
                        "{MODES_PATH} is missing at {}",
                        run.sha
                    )));
                };
                StepExec::Agent {
                    runtime: Box::new(Runtime {
                        name: "ez".into(),
                        install: Vec::new(),
                        protocol: "acp".into(),
                        command: vec!["ez".into(), "acp".into()],
                        env: BTreeMap::new(),
                        capabilities: Value::Null,
                    }),
                    model: None,
                    prompt: instruction.clone(),
                    result_schema: result_schema.clone(),
                    mode: Some(mode.clone()),
                    ez: Some(EzSpec {
                        modes_toml,
                        instruction_files: pin.instructions.clone(),
                    }),
                }
            }
            Rendered::Command { task, args, script } => StepExec::Command {
                run: match (task, script) {
                    (Some(t), _) => {
                        let mut s = format!("mise run {}", shell_quote(t));
                        if !args.is_empty() {
                            s.push_str(" --");
                            for a in args {
                                s.push(' ');
                                s.push_str(&shell_quote(a));
                            }
                        }
                        s
                    }
                    (None, Some(sc)) => sc.clone(),
                    (None, None) => {
                        return Err(BuildError::Fatal(format!("node `{node}` has no command")));
                    }
                },
                tools: Vec::new(),
            },
            _ => return Err(BuildError::Fatal(format!("node `{node}` runs in tracks"))),
        };
        let _ = visit;
        Ok(StepStart {
            run_id: String::new(),
            attempt: state.run.attempt,
            task_id: run.id.clone(),
            workspaces: vec![ws],
            exec,
            outputs: BTreeMap::new(),
            env,
            files: BTreeMap::new(),
            github_token: None,
        })
    }
}

enum StepOutcome {
    Advanced,
    Ended,
}

enum PinError {
    Retry(String),
    Fatal(String),
}

enum BuildError {
    NoCredential,
    Fatal(String),
    Retry(String),
}

/// `[A-Za-z0-9._-]{1,128}`: run uuid (36) + `-` + node run id.
pub fn step_id(run_id: &str, node_run_id: i64) -> String {
    format!("{run_id}-{node_run_id}")
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n:x}")
}

/// The node's outputs for `advance`, from what the sandbox reported.
pub fn outputs_of(rendered: &Rendered, fin: &StepFinished) -> Value {
    match rendered {
        Rendered::Agent { .. } => {
            let git = fin.git.first().map_or_else(
                || json!({"repo": null, "branch": null, "head_sha": null, "pushed": false}),
                |g| json!({"repo": g.repo, "branch": g.branch, "head_sha": g.head_sha, "pushed": g.pushed}),
            );
            json!({
                "result": fin.result.clone().unwrap_or(Value::Null),
                "git": git,
                "exit": fin.exit,
            })
        }
        _ => {
            let code = if fin.exit == enzarb_proto::step::exit::SUCCESS {
                0
            } else {
                fin.error
                    .as_deref()
                    .and_then(|e| e.split("exited with code ").nth(1))
                    .and_then(|c| c.trim().parse::<i64>().ok())
                    .unwrap_or(1)
            };
            // The sandbox reports no stdout tail in step_finished yet: `output` is empty and
            // `json` is null (see docs/gaps.md).
            json!({ "exit_code": code, "output": "", "json": Value::Null })
        }
    }
}
