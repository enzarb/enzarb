//! Liveness findings (ADR-0024). A finding says "this looks stuck" or "the platform looks
//! unhealthy"; it never acts. The engine knows nothing about what a check means: checks are
//! data from the workflow library (`checks/*.yaml`, [`enzarb_flow::format::CheckFile`]).
//!
//! Two steps, so evaluation can be tested without a database and cannot touch tasks:
//! - [`evaluate`] is pure: checks + a [`Snapshot`] of facts and clocks -> desired findings.
//! - [`reconcile`] is the only writer, and writes only `engine.findings`.
//!
//! [`Engine::run_findings`] (called from the ticker) loads the snapshot (reads only), evaluates
//! and reconciles.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use enzarb_flow::format::{
    CheckFile, CheckScope, CmpOp, FindingKind, PlatformCmp, PlatformFactName, Severity, When,
    parse_duration,
};
use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::engine::Engine;
use crate::error::Error;
use crate::sandbox::SandboxResolver;

/// How often the findings pass runs (`ENZARB_FINDINGS_SECS`).
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

// ------------------------------------------------------------------ platform facts

/// A platform fact, or `Unknown` when its source is missing or failed (never a failure).
#[derive(Debug, Clone, PartialEq)]
pub enum Fact {
    Unknown,
    Bool(bool),
    Num(f64),
    Text(String),
}

/// Where platform facts come from. Real providers are thin adapters over what exists.
#[async_trait]
pub trait PlatformFacts: Send + Sync {
    async fn get(&self, project: &str, fact: PlatformFactName) -> Fact;
}

/// Every fact is unknown: no platform check ever fires.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPlatformFacts;

#[async_trait]
impl PlatformFacts for NoPlatformFacts {
    async fn get(&self, _project: &str, _fact: PlatformFactName) -> Fact {
        Fact::Unknown
    }
}

/// A map-backed provider for tests.
#[derive(Debug, Clone, Default)]
pub struct FakePlatformFacts {
    facts: Arc<Mutex<HashMap<(String, PlatformFactName), Fact>>>,
}

impl FakePlatformFacts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, project: &str, fact: PlatformFactName, value: Fact) {
        self.facts
            .lock()
            .expect("facts")
            .insert((project.to_string(), fact), value);
    }
}

#[async_trait]
impl PlatformFacts for FakePlatformFacts {
    async fn get(&self, project: &str, fact: PlatformFactName) -> Fact {
        self.facts
            .lock()
            .expect("facts")
            .get(&(project.to_string(), fact))
            .cloned()
            .unwrap_or(Fact::Unknown)
    }
}

/// The real provider: disk use from the latest sandbox sample and the pending sandbox change
/// from the Project. Workflow run status, chart deploy state and memory percentage have no
/// source yet and stay unknown (sandbox samples carry no memory limit; the connector has no
/// workflow run call; nothing reports the chart deploy).
pub struct DbPlatformFacts {
    pub pool: PgPool,
    pub sandboxes: Arc<dyn SandboxResolver>,
    /// A sample older than this is not a fact.
    pub max_age: Duration,
}

impl DbPlatformFacts {
    pub fn new(pool: PgPool, sandboxes: Arc<dyn SandboxResolver>) -> Self {
        Self {
            pool,
            sandboxes,
            max_age: Duration::from_secs(600),
        }
    }
}

#[async_trait]
impl PlatformFacts for DbPlatformFacts {
    async fn get(&self, project: &str, fact: PlatformFactName) -> Fact {
        match fact {
            PlatformFactName::SandboxDiskPct => {
                let row: Result<Option<(i64, i64)>, _> = sqlx::query_as(
                    "SELECT storage_used_bytes, storage_total_bytes FROM engine.sandbox_stats
                     WHERE project = $1 AND at > now() - make_interval(secs => $2)
                       AND storage_used_bytes IS NOT NULL AND storage_total_bytes > 0
                     ORDER BY at DESC LIMIT 1",
                )
                .bind(project)
                .bind(self.max_age.as_secs_f64())
                .fetch_optional(&self.pool)
                .await;
                match row {
                    Ok(Some((used, total))) => Fact::Num(used as f64 * 100.0 / total as f64),
                    _ => Fact::Unknown,
                }
            }
            PlatformFactName::ProjectPendingChange => match self.sandboxes.change_states().await {
                Ok(states) => states
                    .iter()
                    .find(|s| s.name == project)
                    .map_or(Fact::Unknown, |s| {
                        Fact::Bool(s.pending_generation.is_some())
                    }),
                Err(_) => Fact::Unknown,
            },
            _ => Fact::Unknown,
        }
    }
}

// ------------------------------------------------------------------ snapshot

/// What a task looks like to the checks.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskSnap {
    pub id: i64,
    pub project: String,
    pub phase: String,
    pub node: String,
    pub waiting_on: Option<String>,
    /// Stored facts by name.
    pub facts: BTreeMap<String, FactRow>,
    /// The last event routed to the task, else its creation.
    pub last_event_at: DateTime<Utc>,
    /// The last step activity, else the task's creation. A running step counts as `now`.
    pub last_step_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FactRow {
    pub value: Value,
    pub unknown_refreshes: i32,
}

/// Everything [`evaluate`] reads.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// Open tasks.
    pub tasks: Vec<TaskSnap>,
    /// Projects the platform-scope checks run for.
    pub projects: Vec<String>,
    /// Platform facts, fetched ahead for the checks that name them.
    pub platform: HashMap<(String, PlatformFactName), Fact>,
}

// ------------------------------------------------------------------ evaluate

/// One finding that should be open.
#[derive(Debug, Clone, PartialEq)]
pub struct Desired {
    pub key: String,
    pub task_id: Option<i64>,
    pub project: String,
    pub check_id: String,
    pub kind: FindingKind,
    pub severity: Severity,
    pub text: String,
}

/// The outcome of [`evaluate`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Evaluation {
    pub desired: Vec<Desired>,
    /// Keys whose state could not be told (a platform fact was unknown): an open finding stays
    /// as it is, and none is raised.
    pub undetermined: Vec<String>,
}

/// Which `(project, fact)` pairs the checks read, so the provider is asked once each.
pub fn platform_reads(checks: &[CheckFile]) -> Vec<PlatformFactName> {
    fn walk(w: &When, out: &mut Vec<PlatformFactName>) {
        match w {
            When::All(v) | When::Any(v) => v.iter().for_each(|w| walk(w, out)),
            When::Not(w) => walk(w, out),
            When::Platform(p) if !out.contains(&p.fact) => out.push(p.fact),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in checks {
        walk(&c.when, &mut out);
    }
    out
}

/// Pure: no I/O, no clock, no side effects. Tasks are never touched.
pub fn evaluate(checks: &[CheckFile], snap: &Snapshot, now: DateTime<Utc>) -> Evaluation {
    let mut out = Evaluation::default();
    for c in checks {
        match c.scope {
            CheckScope::Task => {
                for t in &snap.tasks {
                    let mut cx = Cx {
                        snap,
                        project: &t.project,
                        task: Some(t),
                        now,
                        unknown: false,
                    };
                    let hit = cx.eval(&c.when);
                    let key = format!("{}:{}", c.id, t.id);
                    cx.finish(&mut out, c, key, Some(t.id), hit);
                }
            }
            CheckScope::Platform => {
                for p in &snap.projects {
                    let mut cx = Cx {
                        snap,
                        project: p,
                        task: None,
                        now,
                        unknown: false,
                    };
                    let hit = cx.eval(&c.when);
                    let key = format!("{}:{p}", c.id);
                    cx.finish(&mut out, c, key, None, hit);
                }
            }
        }
    }
    out
}

struct Cx<'a> {
    snap: &'a Snapshot,
    project: &'a str,
    task: Option<&'a TaskSnap>,
    now: DateTime<Utc>,
    /// A platform fact needed by the condition was unknown.
    unknown: bool,
}

impl Cx<'_> {
    fn finish(
        &self,
        out: &mut Evaluation,
        c: &CheckFile,
        key: String,
        task: Option<i64>,
        hit: bool,
    ) {
        if self.unknown {
            out.undetermined.push(key);
        } else if hit {
            out.desired.push(Desired {
                task_id: task,
                project: self.project.to_string(),
                check_id: c.id.clone(),
                kind: c.kind,
                severity: c.severity,
                text: self.render(&c.text),
                key,
            });
        }
    }

    fn render(&self, text: &str) -> String {
        let t = self.task;
        text.replace("{project}", self.project)
            .replace("{task}", &t.map_or(String::new(), |t| t.id.to_string()))
            .replace("{node}", t.map_or("", |t| t.node.as_str()))
            .replace("{phase}", t.map_or("", |t| t.phase.as_str()))
    }

    fn eval(&mut self, w: &When) -> bool {
        match w {
            When::All(v) => {
                let mut all = true;
                for w in v {
                    // No short circuit: an unknown anywhere makes the result undetermined.
                    all &= self.eval(w);
                }
                all
            }
            When::Any(v) => {
                let mut any = false;
                for w in v {
                    any |= self.eval(w);
                }
                any
            }
            When::Not(w) => !self.eval(w),
            When::WaitingOn(x) => self
                .task
                .is_some_and(|t| t.waiting_on.as_deref() == Some(x)),
            When::Node(x) => self.task.is_some_and(|t| &t.node == x),
            When::Phase(x) => self.task.is_some_and(|t| &t.phase == x),
            When::Fact(f) => self.task.is_some_and(|t| {
                let v = lookup(t, &f.name);
                match &f.is {
                    Value::String(s) if s == "unknown" => v.is_none(),
                    want => v == Some(want),
                }
            }),
            When::FactUnknownFor(f) => self.task.is_some_and(|t| {
                let root = f.name.split('.').next().unwrap_or(&f.name);
                t.facts.get(root).is_some_and(|r| {
                    r.value.is_null()
                        && r.unknown_refreshes >= i32::try_from(f.refreshes).unwrap_or(i32::MAX)
                })
            }),
            When::FactCount(f) => self.task.is_some_and(|t| match lookup(t, &f.name) {
                Some(Value::Array(a)) => a.len() as u64 == f.eq,
                Some(Value::Number(n)) => n.as_u64() == Some(f.eq),
                _ => false,
            }),
            When::IdleFor(i) => self.task.is_some_and(|t| {
                let idle = |since: DateTime<Utc>, d: &Option<String>| match d {
                    None => true,
                    Some(d) => parse_duration(d).is_some_and(|secs| {
                        (self.now - since).num_seconds() >= i64::try_from(secs).unwrap_or(i64::MAX)
                    }),
                };
                idle(t.last_event_at, &i.no_event) && idle(t.last_step_at, &i.no_step)
            }),
            When::Platform(p) => self.platform(p),
        }
    }

    fn platform(&mut self, p: &PlatformCmp) -> bool {
        let fact = self
            .snap
            .platform
            .get(&(self.project.to_string(), p.fact))
            .unwrap_or(&Fact::Unknown);
        match compare(fact, p.op, &p.value) {
            Some(b) => b,
            None => {
                self.unknown = true;
                false
            }
        }
    }
}

/// The fact's value, or `None` when it is missing or null (unknown). A dotted name descends
/// into the fact's JSON value (`ci.check_runs`).
fn lookup<'a>(t: &'a TaskSnap, name: &str) -> Option<&'a Value> {
    let mut parts = name.split('.');
    let mut v = &t.facts.get(parts.next()?)?.value;
    for p in parts {
        v = v.get(p)?;
    }
    (!v.is_null()).then_some(v)
}

/// `None` when the fact is unknown or the types do not compare.
fn compare(fact: &Fact, op: CmpOp, want: &Value) -> Option<bool> {
    use std::cmp::Ordering;
    let ord = match (fact, want) {
        (Fact::Unknown, _) => return None,
        (Fact::Num(a), Value::Number(b)) => a.partial_cmp(&b.as_f64()?)?,
        (Fact::Text(a), Value::String(b)) => a.as_str().cmp(b.as_str()),
        (Fact::Bool(a), Value::Bool(b)) => a.cmp(b),
        _ => return None,
    };
    Some(match op {
        CmpOp::Eq => ord == Ordering::Equal,
        CmpOp::Ne => ord != Ordering::Equal,
        CmpOp::Gt => ord == Ordering::Greater,
        CmpOp::Ge => ord != Ordering::Less,
        CmpOp::Lt => ord == Ordering::Less,
        CmpOp::Le => ord != Ordering::Greater,
    })
}

// ------------------------------------------------------------------ reconcile

/// Counts of one reconcile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    pub raised: u64,
    pub refreshed: u64,
    pub cleared: u64,
}

/// The only writer: brings `engine.findings` in line with `eval` for `projects` (the ones
/// that were evaluated). A finding is raised on the second consecutive pass that wants it
/// (`candidates` holds the keys wanted by the previous pass); an open one is refreshed; an open
/// one that is no longer wanted (and not undetermined) is cleared. Touches nothing else.
pub async fn reconcile(
    pool: &PgPool,
    eval: &Evaluation,
    projects: &[String],
    candidates: &mut HashSet<String>,
) -> Result<ReconcileStats, Error> {
    let mut st = ReconcileStats::default();
    let mut next = HashSet::new();
    for d in &eval.desired {
        let updated = sqlx::query(
            "UPDATE engine.findings SET text = $2, severity = $3, kind = $4, last_seen = now()
             WHERE key = $1 AND cleared_at IS NULL",
        )
        .bind(&d.key)
        .bind(&d.text)
        .bind(d.severity.as_str())
        .bind(d.kind.as_str())
        .execute(pool)
        .await?
        .rows_affected();
        if updated > 0 {
            st.refreshed += 1;
        } else if candidates.contains(&d.key) {
            let r = sqlx::query(
                "INSERT INTO engine.findings (key, task_id, project, check_id, kind, severity, text)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (key) DO UPDATE SET kind = EXCLUDED.kind, severity = EXCLUDED.severity,
                     text = EXCLUDED.text, first_seen = now(), last_seen = now(), cleared_at = NULL",
            )
            .bind(&d.key)
            .bind(d.task_id)
            .bind(&d.project)
            .bind(&d.check_id)
            .bind(d.kind.as_str())
            .bind(d.severity.as_str())
            .bind(&d.text)
            .execute(pool)
            .await;
            match r {
                Ok(_) => st.raised += 1,
                // The task was deleted since the snapshot.
                Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {}
                Err(e) => return Err(e.into()),
            }
        } else {
            next.insert(d.key.clone());
        }
    }
    let keep: Vec<&str> = eval
        .desired
        .iter()
        .map(|d| d.key.as_str())
        .chain(eval.undetermined.iter().map(String::as_str))
        .collect();
    st.cleared = sqlx::query(
        "UPDATE engine.findings SET cleared_at = now()
         WHERE cleared_at IS NULL AND project = ANY($1) AND key <> ALL($2)",
    )
    .bind(projects)
    .bind(&keep)
    .execute(pool)
    .await?
    .rows_affected();
    *candidates = next;
    Ok(st)
}

// ------------------------------------------------------------------ the engine's pass

/// Shared state of the findings pass. Cheap to clone.
#[derive(Clone)]
pub struct Findings {
    pub facts: Arc<dyn PlatformFacts>,
    /// Pass interval (`ENZARB_FINDINGS_SECS`).
    pub interval: Duration,
    state: Arc<Mutex<PassState>>,
}

#[derive(Default)]
struct PassState {
    last_run: Option<Instant>,
    candidates: HashSet<String>,
}

impl Findings {
    pub fn new(facts: Arc<dyn PlatformFacts>) -> Self {
        Self {
            facts,
            interval: DEFAULT_INTERVAL,
            state: Arc::default(),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Findings without any platform source (platform checks never fire).
    pub fn none() -> Self {
        Self::new(Arc::new(NoPlatformFacts))
    }

    fn due(&self) -> bool {
        let mut s = self.state.lock().expect("state");
        match s.last_run {
            Some(t) if t.elapsed() < self.interval => false,
            _ => {
                s.last_run = Some(Instant::now());
                true
            }
        }
    }
}

impl Engine {
    /// The ticker's findings pass, at most once per [`Findings::interval`].
    pub async fn run_findings(&self) -> Result<Option<ReconcileStats>, Error> {
        if !self.findings.due() {
            return Ok(None);
        }
        self.findings_pass().await.map(Some)
    }

    /// One pass regardless of the interval: load, evaluate, reconcile.
    pub async fn findings_pass(&self) -> Result<ReconcileStats, Error> {
        let mut checks = Vec::new();
        let mut evaluated: Vec<String> = Vec::new();
        let mut snap = Snapshot::default();
        let mut by_project: Vec<(String, Arc<Vec<CheckFile>>)> = Vec::new();
        for p in self.index.all() {
            match self.library.checks(&p).await {
                Ok(c) => by_project.push((p.name.clone(), c)),
                // Findings of a Project whose checks cannot be read are left as they are.
                Err(e) => tracing::warn!(project = %p.name, "findings: loading checks: {e}"),
            }
        }
        for (project, c) in &by_project {
            evaluated.push(project.clone());
            snap.projects.push(project.clone());
            for fact in platform_reads(c) {
                let v = self.findings.facts.get(project, fact).await;
                snap.platform.insert((project.clone(), fact), v);
            }
        }
        snap.tasks = load_tasks(&self.pool, &evaluated).await?;
        let now = Utc::now();
        let mut eval = Evaluation::default();
        for (project, c) in &by_project {
            checks.clear();
            checks.extend(c.iter().cloned());
            let one = Snapshot {
                tasks: snap
                    .tasks
                    .iter()
                    .filter(|t| &t.project == project)
                    .cloned()
                    .collect(),
                projects: vec![project.clone()],
                platform: snap.platform.clone(),
            };
            let e = evaluate(&checks, &one, now);
            eval.desired.extend(e.desired);
            eval.undetermined.extend(e.undetermined);
        }
        let mut candidates =
            std::mem::take(&mut self.findings.state.lock().expect("state").candidates);
        let r = reconcile(&self.pool, &eval, &evaluated, &mut candidates).await;
        self.findings.state.lock().expect("state").candidates = candidates;
        r
    }
}

/// Reads the open tasks of `projects` with their facts and clocks. Reads only.
pub async fn load_tasks(pool: &PgPool, projects: &[String]) -> Result<Vec<TaskSnap>, Error> {
    let rows = sqlx::query(
        "SELECT t.id, t.project, COALESCE(p.phase, ''), COALESCE(p.node, ''), p.waiting_on,
                GREATEST(t.created_at, (SELECT max(e.received_at) FROM engine.events e WHERE e.task_id = t.id)),
                CASE WHEN EXISTS (SELECT 1 FROM engine.step_runs s WHERE s.task_id = t.id
                                  AND s.started_at IS NOT NULL AND s.ended_at IS NULL)
                     THEN now()
                     ELSE GREATEST(t.created_at, (SELECT max(GREATEST(s.queued_at, s.started_at, s.ended_at, s.last_activity_at))
                                                  FROM engine.step_runs s WHERE s.task_id = t.id))
                END
         FROM engine.tasks t LEFT JOIN engine.projection p ON p.task_id = t.id
         WHERE t.ended_at IS NULL AND t.project = ANY($1) ORDER BY t.id",
    )
    .bind(projects)
    .fetch_all(pool)
    .await?;
    let mut tasks: Vec<TaskSnap> = rows
        .iter()
        .map(|r| TaskSnap {
            id: r.get(0),
            project: r.get(1),
            phase: r.get(2),
            node: r.get(3),
            waiting_on: r.get(4),
            facts: BTreeMap::new(),
            last_event_at: r.get(5),
            last_step_at: r.get(6),
        })
        .collect();
    let ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    let facts = sqlx::query(
        "SELECT task_id, name, value, unknown_refreshes FROM engine.facts WHERE task_id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    let index: HashMap<i64, usize> = tasks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    for r in facts {
        if let Some(&i) = index.get(&r.get::<i64, _>(0)) {
            tasks[i].facts.insert(
                r.get(1),
                FactRow {
                    value: r.get(2),
                    unknown_refreshes: r.get(3),
                },
            );
        }
    }
    Ok(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 18, 12, 0, 0).unwrap()
    }

    fn check(when: Value) -> CheckFile {
        serde_json::from_value(json!({
            "version": 1, "id": "c", "kind": "dead_wait", "severity": "warning",
            "text": "task {task} at {node} in {project}", "when": when,
        }))
        .unwrap()
    }

    fn platform_check(when: Value) -> CheckFile {
        let mut c = check(when);
        c.scope = CheckScope::Platform;
        c.kind = FindingKind::PlatformFailure;
        c
    }

    fn task(id: i64) -> TaskSnap {
        TaskSnap {
            id,
            project: "p".into(),
            phase: "review".into(),
            node: "review/wait".into(),
            waiting_on: Some("external".into()),
            facts: BTreeMap::new(),
            last_event_at: now() - chrono::Duration::hours(2),
            last_step_at: now() - chrono::Duration::hours(3),
        }
    }

    fn fact(t: &mut TaskSnap, name: &str, value: Value, unknown_refreshes: i32) {
        t.facts.insert(
            name.into(),
            FactRow {
                value,
                unknown_refreshes,
            },
        );
    }

    fn snap(tasks: Vec<TaskSnap>) -> Snapshot {
        Snapshot {
            tasks,
            projects: vec!["p".into()],
            platform: HashMap::new(),
        }
    }

    fn hits(c: &CheckFile, s: &Snapshot) -> Vec<String> {
        evaluate(std::slice::from_ref(c), s, now())
            .desired
            .into_iter()
            .map(|d| d.key)
            .collect()
    }

    #[test]
    fn task_leaves() {
        let mut t = task(1);
        fact(&mut t, "mergeable", Value::Null, 3);
        fact(&mut t, "ci", json!({"sha": "abc", "check_runs": []}), 0);
        let s = snap(vec![t]);
        for (when, want) in [
            (json!({"waiting_on": "external"}), true),
            (json!({"waiting_on": "human"}), false),
            (json!({"node": "review/wait"}), true),
            (json!({"node": "review/other"}), false),
            (json!({"phase": "review"}), true),
            (
                json!({"fact": {"name": "mergeable", "is": "unknown"}}),
                true,
            ),
            (json!({"fact": {"name": "nothing", "is": "unknown"}}), true),
            (json!({"fact": {"name": "ci.sha", "is": "unknown"}}), false),
            (json!({"fact": {"name": "ci.sha", "is": "abc"}}), true),
            (
                json!({"fact_unknown_for": {"name": "mergeable", "refreshes": 3}}),
                true,
            ),
            (
                json!({"fact_unknown_for": {"name": "mergeable", "refreshes": 4}}),
                false,
            ),
            (
                json!({"fact_unknown_for": {"name": "ci", "refreshes": 1}}),
                false,
            ),
            (
                json!({"fact_count": {"name": "ci.check_runs", "eq": 0}}),
                true,
            ),
            (
                json!({"fact_count": {"name": "ci.check_runs", "eq": 1}}),
                false,
            ),
            (
                json!({"fact_count": {"name": "ci.missing", "eq": 0}}),
                false,
            ),
            (json!({"idle_for": {"no_event": "1h"}}), true),
            (json!({"idle_for": {"no_event": "3h"}}), false),
            (
                json!({"idle_for": {"no_event": "1h", "no_step": "3h"}}),
                true,
            ),
            (
                json!({"idle_for": {"no_event": "1h", "no_step": "4h"}}),
                false,
            ),
        ] {
            assert_eq!(!hits(&check(when.clone()), &s).is_empty(), want, "{when}");
        }
    }

    #[test]
    fn false_fact_and_combinators() {
        let mut t = task(1);
        fact(&mut t, "mergeable", json!(false), 0);
        let s = snap(vec![t]);
        let f = json!({"fact": {"name": "mergeable", "is": false}});
        let ext = json!({"waiting_on": "external"});
        let human = json!({"waiting_on": "human"});
        for (when, want) in [
            (f.clone(), true),
            (json!({"all": [f, ext]}), true),
            (json!({"all": [f, human]}), false),
            (json!({"any": [human, ext]}), true),
            (json!({"any": [human, human]}), false),
            (json!({"not": human}), true),
            (json!({"not": ext}), false),
        ] {
            assert_eq!(!hits(&check(when.clone()), &s).is_empty(), want, "{when}");
        }
    }

    #[test]
    fn one_key_per_check_and_task_with_rendered_text() {
        let c = check(json!({"waiting_on": "external"}));
        let s = snap(vec![task(1), task(2)]);
        let e = evaluate(std::slice::from_ref(&c), &s, now());
        let keys: Vec<_> = e.desired.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, ["c:1", "c:2"]);
        assert_eq!(e.desired[0].text, "task 1 at review/wait in p");
        assert_eq!(e.desired[0].kind, FindingKind::DeadWait);
        assert_eq!(e.desired[0].task_id, Some(1));
    }

    #[test]
    fn platform_thresholds_pending_change_and_run_status() {
        let mut s = snap(vec![]);
        let put = |s: &mut Snapshot, f, v| {
            s.platform.insert(("p".into(), f), v);
        };
        let disk = platform_check(
            json!({"platform": {"fact": "sandbox.disk_pct", "op": "ge", "value": 90}}),
        );
        put(&mut s, PlatformFactName::SandboxDiskPct, Fact::Num(95.0));
        assert_eq!(hits(&disk, &s), ["c:p"]);
        assert_eq!(
            evaluate(std::slice::from_ref(&disk), &s, now()).desired[0].task_id,
            None
        );
        put(&mut s, PlatformFactName::SandboxDiskPct, Fact::Num(89.9));
        assert!(hits(&disk, &s).is_empty());

        let mem = platform_check(
            json!({"platform": {"fact": "sandbox.mem_pct", "op": "gt", "value": 80}}),
        );
        put(&mut s, PlatformFactName::SandboxMemPct, Fact::Num(80.5));
        assert_eq!(hits(&mem, &s), ["c:p"]);

        let pending =
            platform_check(json!({"platform": {"fact": "project.pending_change", "value": true}}));
        put(
            &mut s,
            PlatformFactName::ProjectPendingChange,
            Fact::Bool(true),
        );
        assert_eq!(hits(&pending, &s), ["c:p"]);

        let run = platform_check(
            json!({"platform": {"fact": "workflow_run.status", "op": "ne", "value": "success"}}),
        );
        put(
            &mut s,
            PlatformFactName::WorkflowRunStatus,
            Fact::Text("failure".into()),
        );
        assert_eq!(hits(&run, &s), ["c:p"]);
        put(
            &mut s,
            PlatformFactName::WorkflowRunStatus,
            Fact::Text("success".into()),
        );
        assert!(hits(&run, &s).is_empty());

        let chart =
            platform_check(json!({"platform": {"fact": "chart.deploy_state", "value": "failed"}}));
        put(
            &mut s,
            PlatformFactName::ChartDeployState,
            Fact::Text("failed".into()),
        );
        assert_eq!(hits(&chart, &s), ["c:p"]);
    }

    #[test]
    fn unknown_platform_fact_is_not_a_failure() {
        let s = snap(vec![]);
        // Even negated: unknown never raises.
        for when in [
            json!({"platform": {"fact": "sandbox.disk_pct", "op": "ge", "value": 90}}),
            json!({"not": {"platform": {"fact": "sandbox.disk_pct", "op": "ge", "value": 90}}}),
        ] {
            let e = evaluate(&[platform_check(when)], &s, now());
            assert!(e.desired.is_empty());
            assert_eq!(e.undetermined, ["c:p"]);
        }
    }

    #[test]
    fn platform_checks_run_per_project_task_checks_per_task() {
        let mut s = snap(vec![task(1)]);
        s.projects.push("q".into());
        s.platform.insert(
            ("q".into(), PlatformFactName::SandboxDiskPct),
            Fact::Num(99.0),
        );
        s.platform.insert(
            ("p".into(), PlatformFactName::SandboxDiskPct),
            Fact::Num(1.0),
        );
        let c = platform_check(
            json!({"platform": {"fact": "sandbox.disk_pct", "op": "ge", "value": 90}}),
        );
        assert_eq!(hits(&c, &s), ["c:q"]);
    }

    #[test]
    fn evaluate_is_pure() {
        let c = check(json!({"waiting_on": "external"}));
        let s = snap(vec![task(1)]);
        assert_eq!(
            evaluate(std::slice::from_ref(&c), &s, now()),
            evaluate(std::slice::from_ref(&c), &s, now())
        );
    }

    #[test]
    fn reads_are_collected_once() {
        let c = platform_check(json!({"all": [
            {"platform": {"fact": "sandbox.disk_pct", "value": 1}},
            {"platform": {"fact": "sandbox.disk_pct", "value": 2}},
            {"not": {"platform": {"fact": "project.pending_change", "value": true}}},
        ]}));
        assert_eq!(
            platform_reads(&[c]),
            [
                PlatformFactName::SandboxDiskPct,
                PlatformFactName::ProjectPendingChange
            ]
        );
    }
}
