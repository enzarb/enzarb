//! Routes connector events to the owning Project's workflows and records a decision for each
//! (ADR-0029 section 5, rollout step 3). Nothing executes: a match creates a `pending` run.
//!
//! Delivery guarantees:
//! * `LISTEN connector_event` gives low latency; a periodic sweep (and one after every
//!   reconnect) picks up events missed while down or whose processing failed transiently.
//! * An event is decided at most once: the `event_decisions` primary key is claimed inside the
//!   transaction that creates the runs.
//! * Concurrency is bounded by a semaphore.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ez_workflow::validate::Severity;
use ez_workflow::{Engine, Event, ModeCatalog, RunState, Subject, TriggerEval, WorkflowsFile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::postgres::PgListener;
use sqlx::{PgPool, Row};
use tokio::sync::{Semaphore, watch};

use crate::github::{RATE_LIMITED, RepoSource, Snapshot, SourceError};
use crate::projects::{ProjectLookup, ProjectRef};

/// Failed attempts (transient errors) before the event gets an `error` decision.
pub const MAX_ATTEMPTS: u32 = 5;
const SWEEP_BATCH: i64 = 500;

/// One row of `connectors.event_v`.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: i64,
    pub event: String,
    pub action: Option<String>,
    pub repo: Option<String>,
    pub payload: Value,
    pub received_at: Option<String>,
}

/// One workflow's trigger evaluation, as stored in `event_decisions.reasons`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub workflow: String,
    pub matched: bool,
    #[serde(default)]
    pub on: Option<String>,
    pub reason: String,
}

impl From<TriggerEval> for Reason {
    fn from(t: TriggerEval) -> Self {
        Self {
            workflow: t.workflow,
            matched: t.matched,
            on: t.on,
            reason: t.reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Started,
    NoMatch,
    Skipped,
    Error,
    /// The event started no run but resumed runs parked in a `wait` node.
    Resumed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::NoMatch => "no_match",
            Self::Skipped => "skipped",
            Self::Error => "error",
            Self::Resumed => "resumed",
        }
    }
}

/// What routing concluded, before the runs are created.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub event_id: i64,
    pub repo: Option<String>,
    pub project: Option<String>,
    pub sha: Option<String>,
    pub outcome: Outcome,
    /// Workflows whose trigger matched (they start a run unless the subject already has one).
    pub matched: Vec<String>,
    pub reasons: Vec<Reason>,
    pub errors: Vec<String>,
    /// Runs this event resumed out of a `wait` node: `{run, workflow, node}` each.
    pub resumed: Vec<Value>,
}

impl Decision {
    fn new(row: &EventRow, outcome: Outcome) -> Self {
        Self {
            event_id: row.id,
            repo: row.repo.clone(),
            project: None,
            sha: None,
            outcome,
            matched: Vec::new(),
            reasons: Vec::new(),
            errors: Vec::new(),
            resumed: Vec::new(),
        }
    }

    fn error(row: &EventRow, project: Option<&ProjectRef>, msg: impl Into<String>) -> Self {
        let mut d = Self::new(row, Outcome::Error);
        d.project = project.map(|p| p.name.clone());
        d.errors.push(msg.into());
        d
    }
}

fn event_of(row: &EventRow) -> Event {
    Event::new(row.event.clone(), row.action.clone(), row.payload.clone())
}

/// The pure part: what the files at `snap` say about `row`. `Decision::outcome` is `Started`
/// when some workflow matched (the commit step may still skip one whose subject is busy).
pub fn evaluate(
    engine: &Engine,
    row: &EventRow,
    project: &ProjectRef,
    snap: &Snapshot,
) -> (Decision, Option<WorkflowsFile>) {
    let mut d = Decision::new(row, Outcome::NoMatch);
    d.project = Some(project.name.clone());
    d.sha = Some(snap.sha.clone());
    let Some(text) = &snap.workflows else {
        d.reasons.push(Reason {
            workflow: ".ez/workflows.toml".into(),
            matched: false,
            on: None,
            reason: "the repository defines no workflows".into(),
        });
        return (d, None);
    };
    let fail = |mut d: Decision, msg: String| {
        d.outcome = Outcome::Error;
        d.errors.push(msg);
        (d, None)
    };
    let modes = match &snap.modes {
        None => None,
        Some(m) => match ModeCatalog::from_modes_toml(m) {
            Ok(c) => Some(c),
            Err(e) => return fail(d, format!("invalid .ez/modes.toml: {e}")),
        },
    };
    let file = match ez_workflow::parse(text) {
        Ok(f) => f,
        Err(e) => return fail(d, format!("invalid .ez/workflows.toml: {e}")),
    };
    let report = ez_workflow::validate(engine, &file, modes.as_ref());
    if !report.is_ok() {
        for diag in report
            .diagnostics
            .iter()
            .filter(|x| x.severity == Severity::Error)
        {
            d.errors.push(if diag.path.is_empty() {
                format!("invalid .ez/workflows.toml: {}", diag.message)
            } else {
                format!(
                    "invalid .ez/workflows.toml: {}: {}",
                    diag.path, diag.message
                )
            });
        }
        d.outcome = Outcome::Error;
        return (d, None);
    }
    let evals = ez_workflow::evaluate_triggers(engine, &file, &event_of(row));
    d.matched = evals
        .iter()
        .filter(|e| e.matched)
        .map(|e| e.workflow.clone())
        .collect();
    d.reasons = evals.into_iter().map(Reason::from).collect();
    if d.reasons.is_empty() {
        d.reasons.push(Reason {
            workflow: ".ez/workflows.toml".into(),
            matched: false,
            on: None,
            reason: "the repository defines no workflows".into(),
        });
    }
    if !d.matched.is_empty() {
        d.outcome = Outcome::Started;
    }
    (d, Some(file))
}

/// Decision for one event; `Err` is transient (nothing is recorded, the sweep retries).
pub async fn route(
    engine: &Engine,
    projects: &dyn ProjectLookup,
    source: &dyn RepoSource,
    row: &EventRow,
) -> Result<(Decision, Option<WorkflowsFile>), String> {
    let Some(repo) = &row.repo else {
        return Ok((Decision::new(row, Outcome::Skipped), None));
    };
    // The App is installed org-wide, so most repositories belong to no Project: not an error.
    let Some(project) = projects.owner(repo).await? else {
        return Ok((Decision::new(row, Outcome::Skipped), None));
    };
    match source.snapshot(repo, &project.default_branch).await {
        Ok(snap) => Ok(evaluate(engine, row, &project, &snap)),
        Err(SourceError::Permanent(m)) => Ok((Decision::error(row, Some(&project), m), None)),
        Err(SourceError::Transient(m)) => Err(m),
    }
}

/// Records `decision` and creates a `pending` run per matched workflow, in one transaction.
/// Returns `false` when the event was already decided (nothing is written).
pub async fn commit(
    pool: &PgPool,
    decision: Decision,
    file: Option<&WorkflowsFile>,
    row: &EventRow,
) -> Result<bool, sqlx::Error> {
    commit_resuming(pool, decision, file, row, &[]).await
}

/// A run parked in a `wait` node that the event satisfies.
#[derive(Debug, Clone)]
pub struct Resume {
    pub run_id: String,
    pub workflow: String,
    pub node: String,
    pub node_run_id: i64,
    /// The wait node's outputs (`event`, `timed_out`).
    pub outputs: Value,
}

/// The head branch of the pull request an event is about, if any.
fn head_ref(row: &EventRow) -> Option<String> {
    row.payload
        .pointer("/pull_request/head/ref")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The parked runs of `row`'s repository whose wait accepts the event. A run is a candidate when
/// the event is about its subject (`owner/repo#number`) or about a pull request whose head is
/// the run's branch (`ez/<workflow>/<run>`); then the wait's `on` and `if` decide. Returns the
/// matches and notes for events that could not be evaluated. `Err` is transient.
pub async fn resume_candidates(
    pool: &PgPool,
    engine: &Engine,
    source: &dyn RepoSource,
    row: &EventRow,
) -> Result<(Vec<Resume>, Vec<String>), String> {
    let Some(repo) = &row.repo else {
        return Ok((Vec::new(), Vec::new()));
    };
    let event = event_of(row);
    let key = event.subject().map(|s| s.key(repo));
    let rows = sqlx::query(
        "SELECT id::text AS id, workflow, repo, sha, state, wait_node_run_id FROM tracks.runs
          WHERE status = 'waiting' AND NOT test AND finished_at IS NULL
            AND wait_node_run_id IS NOT NULL AND lower(repo) = lower($1)
            AND (subject_key = $2 OR 'ez/' || workflow || '/' || id::text = $3)",
    )
    .bind(repo)
    .bind(&key)
    .bind(head_ref(row))
    .fetch_all(pool)
    .await
    .map_err(|e| format!("reading waiting runs: {e}"))?;
    let mut files: HashMap<(String, String), Result<WorkflowsFile, String>> = HashMap::new();
    let (mut out, mut notes) = (Vec::new(), Vec::new());
    for r in rows {
        let (id, workflow, run_repo, sha): (String, String, String, String) =
            (r.get("id"), r.get("workflow"), r.get("repo"), r.get("sha"));
        let state: RunState = match serde_json::from_value(r.get("state")) {
            Ok(s) => s,
            Err(e) => {
                notes.push(format!("run {id}: corrupt state: {e}"));
                continue;
            }
        };
        let k = (run_repo.clone(), sha.clone());
        if !files.contains_key(&k) {
            let loaded = match source
                .file_at(&run_repo, &sha, crate::github::WORKFLOWS_PATH)
                .await
            {
                Ok(Some(t)) => ez_workflow::parse(&t).map_err(|e| e.to_string()),
                Ok(None) => Err("the workflow file is gone at the pinned commit".into()),
                Err(SourceError::Transient(m)) => return Err(m),
                Err(SourceError::Permanent(m)) => Err(m),
            };
            files.insert(k.clone(), loaded);
        }
        let file = match &files[&k] {
            Ok(f) => f,
            Err(e) => {
                notes.push(format!("run {id}: {e}"));
                continue;
            }
        };
        match state.wait_accepts(engine, file, &event) {
            Ok(Ok(true)) => out.push(Resume {
                run_id: id,
                workflow,
                node: state.current().unwrap_or_default().to_string(),
                node_run_id: r.get("wait_node_run_id"),
                outputs: ez_workflow::wait_outputs(Some(&event)),
            }),
            Ok(Ok(false)) => {}
            Ok(Err(m)) => notes.push(format!("run {id}: {m}")),
            Err(e) => notes.push(format!("run {id}: {e}")),
        }
    }
    Ok((out, notes))
}

/// [`commit`], also resuming the parked runs in `resumes` in the same transaction. The decision
/// row is claimed first, so an event resumes a given wait at most once.
pub async fn commit_resuming(
    pool: &PgPool,
    mut decision: Decision,
    file: Option<&WorkflowsFile>,
    row: &EventRow,
    resumes: &[Resume],
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let claimed = sqlx::query(
        "INSERT INTO tracks.event_decisions (event_id, repo, project, sha, outcome, event_received_at)
         VALUES ($1, $2, $3, $4, 'error', $5::timestamptz) ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(decision.event_id)
    .bind(&decision.repo)
    .bind(&decision.project)
    .bind(&decision.sha)
    .bind(&row.received_at)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if claimed == 0 {
        tx.rollback().await?;
        return Ok(false);
    }

    for r in resumes {
        let res = sqlx::query(
            "UPDATE tracks.runs SET status = 'running', wait_until = NULL, updated_at = now(),
                    wait_result = $3
              WHERE id = $1::uuid AND status = 'waiting' AND wait_node_run_id = $2",
        )
        .bind(&r.run_id)
        .bind(r.node_run_id)
        .bind(json!({ "outputs": r.outputs, "event_id": decision.event_id }))
        .execute(&mut *tx)
        .await?;
        if res.rows_affected() == 1 {
            decision
                .resumed
                .push(json!({ "run": r.run_id, "workflow": r.workflow, "node": r.node }));
        }
    }

    let event = event_of(row);
    let subject: Option<Subject> = event.subject();
    let subject_key = match (&subject, &decision.repo) {
        (Some(s), Some(r)) => Some(s.key(r)),
        _ => None,
    };
    let mut started: Vec<String> = Vec::new();
    let (d_repo, d_project, d_sha) = (
        decision.repo.clone(),
        decision.project.clone(),
        decision.sha.clone(),
    );
    if let (Some(file), Some(repo), Some(project), Some(sha)) = (file, &d_repo, &d_project, &d_sha)
    {
        if let Some(k) = &subject_key {
            // Serialize starts for one subject so two events cannot both see "no live run".
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(k)
                .execute(&mut *tx)
                .await?;
        }
        for wf in decision.matched.clone() {
            if let Some(k) = &subject_key {
                let busy: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM tracks.runs
                      WHERE subject_key = $1 AND workflow = $2 AND finished_at IS NULL AND NOT test)",
                )
                .bind(k)
                .bind(&wf)
                .fetch_one(&mut *tx)
                .await?;
                if busy {
                    note(
                        &mut decision,
                        &wf,
                        format!("{k} already has a live run of this workflow"),
                    );
                    continue;
                }
            }
            let id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
                .fetch_one(&mut *tx)
                .await?;
            let info = ez_workflow::RunInfo {
                id: id.clone(),
                attempt: 1,
                test: false,
            };
            let state = match RunState::start(file, &wf, &event, info) {
                Ok(s) => s,
                Err(e) => {
                    decision.errors.push(format!("could not start {wf}: {e}"));
                    note(&mut decision, &wf, format!("could not start: {e}"));
                    continue;
                }
            };
            let current = state.current().map(str::to_string);
            sqlx::query(
                "INSERT INTO tracks.runs (id, workflow, project, repo, subject_key, subject, event_id,
                                          sha, status, test, current_node, visits, state)
                 VALUES ($1::uuid, $2, $3, $4, $5, $6, $7, $8, 'pending', false, $9, $10, $11)
                 ON CONFLICT (event_id, workflow, test) DO NOTHING",
            )
            .bind(&id)
            .bind(&wf)
            .bind(project)
            .bind(repo)
            .bind(&subject_key)
            .bind(subject.as_ref().and_then(|s| serde_json::to_value(s).ok()))
            .bind(decision.event_id)
            .bind(sha)
            .bind(current)
            .bind(serde_json::to_value(&state.visits).unwrap_or(json!({})))
            .bind(serde_json::to_value(&state).unwrap_or(Value::Null))
            .execute(&mut *tx)
            .await?;
            started.push(wf);
        }
    }
    if decision.outcome == Outcome::Started && started.is_empty() {
        decision.outcome = Outcome::NoMatch;
    }
    decision.matched = started;
    if !decision.resumed.is_empty() && decision.outcome != Outcome::Started {
        decision.outcome = Outcome::Resumed;
    }
    sqlx::query(
        "UPDATE tracks.event_decisions
            SET outcome = $2, matched = $3, reasons = $4, errors = $5, resumed = $6,
                decided_at = now()
          WHERE event_id = $1",
    )
    .bind(decision.event_id)
    .bind(decision.outcome.as_str())
    .bind(json!(decision.matched))
    .bind(serde_json::to_value(&decision.reasons).unwrap_or(json!([])))
    .bind(json!(decision.errors))
    .bind(json!(decision.resumed))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Marks `wf`'s reason as not started, with `why` appended.
fn note(d: &mut Decision, wf: &str, why: String) {
    if let Some(r) = d.reasons.iter_mut().find(|r| r.workflow == wf) {
        r.matched = false;
        r.reason = format!("{}; {why}", r.reason);
    }
}

pub struct Router {
    pub pool: PgPool,
    pub engine: Arc<Engine>,
    pub source: Arc<dyn RepoSource>,
    pub projects: Arc<dyn ProjectLookup>,
    pub concurrency: usize,
    pub sweep: Duration,
    attempts: Mutex<HashMap<i64, u32>>,
    inflight: Mutex<HashSet<i64>>,
}

impl Router {
    pub fn new(
        pool: PgPool,
        source: Arc<dyn RepoSource>,
        projects: Arc<dyn ProjectLookup>,
        concurrency: usize,
        sweep: Duration,
    ) -> Self {
        Self {
            pool,
            engine: Arc::new(Engine::new()),
            source,
            projects,
            concurrency,
            sweep,
            attempts: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashSet::new()),
        }
    }

    /// Events before tracks first ran are history: the floor is the highest id at first start.
    pub async fn init_floor(&self) -> Result<i64, sqlx::Error> {
        sqlx::query(
            "INSERT INTO tracks.router_state (id, floor)
             SELECT true, coalesce(max(id), 0) FROM connectors.event_v ON CONFLICT (id) DO NOTHING",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query_scalar("SELECT floor FROM tracks.router_state WHERE id")
            .fetch_one(&self.pool)
            .await
    }

    async fn load(&self, id: i64) -> Result<Option<EventRow>, sqlx::Error> {
        let r = sqlx::query(
            "SELECT id, event, action, repo, payload, received_at::text AS received_at
               FROM connectors.event_v WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(r.map(|r| EventRow {
            id: r.get("id"),
            event: r.get("event"),
            action: r.get("action"),
            repo: r.get("repo"),
            payload: r.get("payload"),
            received_at: r.get("received_at"),
        }))
    }

    /// Routes one event end to end. Transient failures are counted; after [`MAX_ATTEMPTS`] the
    /// event gets an `error` decision instead of being retried forever.
    pub async fn process(&self, id: i64) {
        if !self.inflight.lock().unwrap().insert(id) {
            return;
        }
        let r = self.process_inner(id).await;
        self.inflight.lock().unwrap().remove(&id);
        if let Err(e) = r {
            // A GitHub rate limit says nothing about the event: it stays undecided for the sweep
            // however long the limit lasts, and is not counted towards giving up.
            if e.contains(RATE_LIMITED) {
                tracing::warn!(event_id = id, error = %e, "routing delayed by a GitHub rate limit");
                return;
            }
            let n = {
                let mut a = self.attempts.lock().unwrap();
                let n = a.entry(id).or_insert(0);
                *n += 1;
                *n
            };
            tracing::warn!(event_id = id, attempt = n, error = %e, "routing failed; will retry");
            if n >= MAX_ATTEMPTS {
                self.give_up(id, &e).await;
            }
        } else {
            self.attempts.lock().unwrap().remove(&id);
        }
    }

    async fn give_up(&self, id: i64, err: &str) {
        let Ok(Some(row)) = self.load(id).await else {
            return;
        };
        let d = Decision::error(
            &row,
            None,
            format!("gave up after {MAX_ATTEMPTS} attempts: {err}"),
        );
        match commit(&self.pool, d, None, &row).await {
            Ok(_) => {
                tracing::error!(event_id = id, error = err, "routing gave up");
                self.attempts.lock().unwrap().remove(&id);
            }
            Err(e) => tracing::warn!(event_id = id, error = %e, "could not record the failure"),
        }
    }

    async fn process_inner(&self, id: i64) -> Result<(), String> {
        let row = match self
            .load(id)
            .await
            .map_err(|e| format!("reading event: {e}"))?
        {
            Some(r) => r,
            None => {
                tracing::debug!(event_id = id, "event not found; skipping");
                return Ok(());
            }
        };
        let decided: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM tracks.event_decisions WHERE event_id = $1)",
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("reading decision: {e}"))?;
        if decided {
            return Ok(());
        }
        let (resumes, notes) =
            resume_candidates(&self.pool, &self.engine, self.source.as_ref(), &row).await?;
        let (mut d, file) = route(
            &self.engine,
            self.projects.as_ref(),
            self.source.as_ref(),
            &row,
        )
        .await?;
        d.errors.extend(notes);
        let outcome_hint = d.outcome;
        let wrote = commit_resuming(&self.pool, d, file.as_ref(), &row, &resumes)
            .await
            .map_err(|e| format!("recording decision: {e}"))?;
        if wrote {
            tracing::info!(event_id = id, repo = ?row.repo, outcome = outcome_hint.as_str(), "routed");
        }
        Ok(())
    }

    /// Ids of events past the floor with no decision yet.
    pub async fn undecided(&self, limit: i64) -> Result<Vec<i64>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT e.id FROM connectors.event_v e
              WHERE e.id > (SELECT floor FROM tracks.router_state WHERE id)
                AND NOT EXISTS (SELECT 1 FROM tracks.event_decisions d WHERE d.event_id = e.id)
              ORDER BY e.id LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
    }

    /// Processes every undecided event; returns how many it attempted.
    pub async fn sweep_once(self: &Arc<Self>, sem: &Arc<Semaphore>) -> Result<usize, sqlx::Error> {
        let ids = self.undecided(SWEEP_BATCH).await?;
        let n = ids.len();
        if n > 0 {
            tracing::info!(count = n, "catching up on undecided events");
        }
        for id in ids {
            self.spawn(sem, id).await;
        }
        Ok(n)
    }

    async fn spawn(self: &Arc<Self>, sem: &Arc<Semaphore>, id: i64) {
        let Ok(permit) = sem.clone().acquire_owned().await else {
            return;
        };
        let me = self.clone();
        tokio::spawn(async move {
            me.process(id).await;
            drop(permit);
        });
    }

    /// Runs until `shutdown` flips to true: LISTEN, reconnect with backoff, catch up.
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
        let sem = Arc::new(Semaphore::new(self.concurrency));
        let mut delay = Duration::from_millis(500);
        loop {
            if *shutdown.borrow() {
                break;
            }
            match self.init_floor().await {
                Ok(f) => tracing::debug!(floor = f, "router floor"),
                Err(e) => {
                    tracing::warn!(error = %e, "router state unavailable; retrying");
                    if sleep_or_stop(&mut shutdown, delay).await {
                        break;
                    }
                    delay = (delay * 2).min(Duration::from_secs(30));
                    continue;
                }
            }
            let mut listener = match PgListener::connect_with(&self.pool).await {
                Ok(mut l) => match l.listen("connector_event").await {
                    Ok(()) => l,
                    Err(e) => {
                        tracing::warn!(error = %e, "LISTEN failed; retrying");
                        if sleep_or_stop(&mut shutdown, delay).await {
                            break;
                        }
                        delay = (delay * 2).min(Duration::from_secs(30));
                        continue;
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, retry_ms = delay.as_millis() as u64, "listen connect failed");
                    if sleep_or_stop(&mut shutdown, delay).await {
                        break;
                    }
                    delay = (delay * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            tracing::info!("listening on connector_event");
            let connected = tokio::time::Instant::now();
            // Notifications sent while disconnected are gone: catch up after every connect.
            if let Err(e) = self.sweep_once(&sem).await {
                tracing::warn!(error = %e, "catch-up failed");
            }
            let mut tick = tokio::time::interval(self.sweep);
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() { return self.drain(&sem).await; }
                    }
                    _ = tick.tick() => {
                        if let Err(e) = self.sweep_once(&sem).await {
                            tracing::warn!(error = %e, "sweep failed");
                        }
                    }
                    n = listener.recv() => match n {
                        Ok(n) => match n.payload().trim().parse::<i64>() {
                            Ok(id) => self.spawn(&sem, id).await,
                            Err(_) => tracing::warn!(payload = n.payload(), "bad connector_event payload"),
                        },
                        Err(e) => {
                            tracing::warn!(error = %e, "listen connection lost");
                            break;
                        }
                    }
                }
            }
            if connected.elapsed() > Duration::from_secs(10) {
                delay = Duration::from_millis(500);
            }
            if sleep_or_stop(&mut shutdown, delay).await {
                break;
            }
            delay = (delay * 2).min(Duration::from_secs(30));
        }
        self.drain(&sem).await;
    }

    async fn drain(&self, sem: &Arc<Semaphore>) {
        let _ = tokio::time::timeout(
            Duration::from_secs(20),
            sem.acquire_many(self.concurrency as u32),
        )
        .await;
    }
}

/// Sleeps `d`; true when shutdown was requested meanwhile.
async fn sleep_or_stop(shutdown: &mut watch::Receiver<bool>, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => *shutdown.borrow(),
        _ = shutdown.changed() => *shutdown.borrow(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKFLOWS: &str = r#"
version = 1
[workflows.fix]
on = ["issues.labeled"]
if = "event.label.name == 'ez'"
start = "done"
[workflows.fix.nodes.done]
type = "end"
status = "done"
[workflows.other]
on = ["push"]
start = "done"
[workflows.other.nodes.done]
type = "end"
status = "done"
"#;

    fn row(label: &str) -> EventRow {
        EventRow {
            id: 7,
            event: "issues".into(),
            action: Some("labeled".into()),
            repo: Some("o/r".into()),
            payload: json!({"action":"labeled","label":{"name":label},
                "issue":{"number":3,"title":"T","user":{"login":"a"},"html_url":"u"}}),
            received_at: None,
        }
    }

    fn project() -> ProjectRef {
        ProjectRef {
            name: "p".into(),
            default_branch: "main".into(),
        }
    }

    fn snap(workflows: Option<&str>, modes: Option<&str>) -> Snapshot {
        Snapshot {
            sha: "abc1234".into(),
            workflows: workflows.map(Into::into),
            modes: modes.map(Into::into),
        }
    }

    #[test]
    fn matching_event_starts_and_records_every_reason() {
        let (d, file) = evaluate(
            &Engine::new(),
            &row("ez"),
            &project(),
            &snap(Some(WORKFLOWS), None),
        );
        assert_eq!(d.outcome, Outcome::Started);
        assert_eq!(d.matched, vec!["fix"]);
        assert_eq!(d.reasons.len(), 2);
        assert!(d.reasons[1].reason.contains("not in on"));
        assert_eq!(d.sha.as_deref(), Some("abc1234"));
        assert_eq!(d.project.as_deref(), Some("p"));
        assert!(file.is_some());
    }

    #[test]
    fn false_condition_is_no_match_with_reason() {
        let (d, _) = evaluate(
            &Engine::new(),
            &row("bug"),
            &project(),
            &snap(Some(WORKFLOWS), None),
        );
        assert_eq!(d.outcome, Outcome::NoMatch);
        assert!(d.matched.is_empty());
        assert!(d.reasons[0].reason.contains("is false"), "{:?}", d.reasons);
    }

    #[test]
    fn missing_file_is_no_match() {
        let (d, f) = evaluate(&Engine::new(), &row("ez"), &project(), &snap(None, None));
        assert_eq!(d.outcome, Outcome::NoMatch);
        assert!(f.is_none());
        assert!(d.reasons[0].reason.contains("no workflows"));
    }

    #[test]
    fn invalid_files_are_errors() {
        let (d, _) = evaluate(
            &Engine::new(),
            &row("ez"),
            &project(),
            &snap(Some("not = [toml"), None),
        );
        assert_eq!(d.outcome, Outcome::Error);
        assert!(d.errors[0].contains("workflows.toml"));
        let (d, _) = evaluate(
            &Engine::new(),
            &row("ez"),
            &project(),
            &snap(Some(WORKFLOWS), Some("= bad")),
        );
        assert_eq!(d.outcome, Outcome::Error);
        assert!(d.errors[0].contains("modes.toml"));
        let bad = WORKFLOWS.replace(
            "start = \"done\"\n[workflows.other.nodes",
            "start = \"nope\"\n[workflows.other.nodes",
        );
        let (d, _) = evaluate(
            &Engine::new(),
            &row("ez"),
            &project(),
            &snap(Some(&bad), None),
        );
        assert_eq!(d.outcome, Outcome::Error);
    }

    struct Projects(Option<ProjectRef>);
    #[async_trait::async_trait]
    impl ProjectLookup for Projects {
        async fn owner(&self, _: &str) -> Result<Option<ProjectRef>, String> {
            Ok(self.0.clone())
        }
    }
    struct Src(Result<Snapshot, SourceError>);
    #[async_trait::async_trait]
    impl RepoSource for Src {
        async fn snapshot(&self, _: &str, _: &str) -> Result<Snapshot, SourceError> {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn route_handles_missing_project_repo_and_source_errors() {
        let e = Engine::new();
        let ok = Src(Ok(snap(Some(WORKFLOWS), None)));
        let (d, _) = route(&e, &Projects(None), &ok, &row("ez")).await.unwrap();
        assert_eq!(d.outcome, Outcome::Skipped);
        assert!(d.errors.is_empty());

        let mut no_repo = row("ez");
        no_repo.repo = None;
        let (d, _) = route(&e, &Projects(Some(project())), &ok, &no_repo)
            .await
            .unwrap();
        assert_eq!(d.outcome, Outcome::Skipped);

        let perm = Src(Err(SourceError::Permanent("not installed".into())));
        let (d, _) = route(&e, &Projects(Some(project())), &perm, &row("ez"))
            .await
            .unwrap();
        assert_eq!(
            (d.outcome, d.project.as_deref()),
            (Outcome::Error, Some("p"))
        );

        let trans = Src(Err(SourceError::Transient("502".into())));
        assert!(
            route(&e, &Projects(Some(project())), &trans, &row("ez"))
                .await
                .is_err()
        );
    }
}
