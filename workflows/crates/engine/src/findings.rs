//! Liveness findings (ADR-0024). A finding says "this looks stuck" or "the platform looks
//! unhealthy"; it never acts. The engine knows nothing about what a check means: checks are
//! data from the workflow library (`checks/*.yaml`, [`enzarb_flow::format::CheckFile`]).
//!
//! Two steps, so evaluation can be tested without a database and cannot touch tasks:
//! - [`evaluate`] is pure: checks + a [`Snapshot`] of facts and clocks -> desired findings.
//! - [`reconcile`] is the only writer. It writes `engine.findings` and, when a finding opens
//!   and its check has an `issue:`, enqueues one `file_issue` job in the same transaction.
//!   It never touches tasks, the projection, timers, step runs or transitions.
//!
//! [`Engine::run_findings`] (called from the ticker) loads the snapshot (reads only), evaluates
//! and reconciles.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use enzarb_connector::file_issue::has_file_issue_marker;
use enzarb_connector::{Connector, RepoKey};
use enzarb_flow::format::{
    CheckFile, CheckScope, CmpOp, FindingKind, IssueSpec, PlatformCmp, PlatformFactName, Severity,
    When, parse_duration,
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

    /// A fact that takes an argument (`workflow_run.status`: `<owner>/<repo>:<workflow file>`).
    /// The default has no source for it.
    async fn get_arg(&self, _project: &str, _fact: PlatformFactName, _arg: &str) -> Fact {
        Fact::Unknown
    }
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

type ArgFacts = HashMap<(String, PlatformFactName, String), Fact>;

/// A map-backed provider for tests.
#[derive(Debug, Clone, Default)]
pub struct FakePlatformFacts {
    facts: Arc<Mutex<HashMap<(String, PlatformFactName), Fact>>>,
    args: Arc<Mutex<ArgFacts>>,
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

    pub fn set_arg(&self, project: &str, fact: PlatformFactName, arg: &str, value: Fact) {
        self.args
            .lock()
            .expect("facts")
            .insert((project.to_string(), fact, arg.to_string()), value);
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

    async fn get_arg(&self, project: &str, fact: PlatformFactName, arg: &str) -> Fact {
        self.args
            .lock()
            .expect("facts")
            .get(&(project.to_string(), fact, arg.to_string()))
            .cloned()
            .unwrap_or(Fact::Unknown)
    }
}

/// How long a connector or updater answer is reused: longer than one pass over every Project,
/// shorter than the pass interval, so each pass asks once however many Projects there are.
const FACT_TTL: Duration = Duration::from_secs(30);

/// Where the chart deploy state comes from.
#[async_trait]
pub trait DeployStateSource: Send + Sync {
    /// `deployed`, `pending` or `stuck` as text; `Unknown` when it cannot be told.
    async fn state(&self) -> Fact;
}

/// No source configured: the deploy state is unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDeployState;

#[async_trait]
impl DeployStateSource for NoDeployState {
    async fn state(&self) -> Fact {
        Fact::Unknown
    }
}

/// Reads the updater's `GET /status` (`{installed, desired, latest, stuck}`).
pub struct HttpDeployState {
    url: String,
    client: reqwest::Client,
}

impl HttpDeployState {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap_or_default(),
        }
    }
}

#[async_trait]
impl DeployStateSource for HttpDeployState {
    async fn state(&self) -> Fact {
        let body: Result<Value, _> = async {
            self.client
                .get(&self.url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await
        }
        .await;
        match body {
            Ok(v) => deploy_state(&v),
            Err(e) => {
                tracing::debug!("findings: updater status: {e}");
                Fact::Unknown
            }
        }
    }
}

/// `x.y.z` (optional `v` prefix, pre-release and build suffixes dropped) as numbers.
fn version_parts(v: &str) -> Option<Vec<u64>> {
    let core = v.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    core.split('.').map(|p| p.parse().ok()).collect()
}

/// The updater's status as a deploy state: `stuck` when it says so, `deployed` when the
/// installed chart is the latest (or newer), `pending` when a newer one exists. A missing
/// field, or versions that cannot be compared, is unknown.
pub fn deploy_state(status: &Value) -> Fact {
    if status.get("stuck").and_then(Value::as_bool) == Some(true) {
        return Fact::Text("stuck".into());
    }
    let field = |k: &str| status.get(k).and_then(Value::as_str);
    let (Some(installed), Some(latest)) = (field("installed"), field("latest")) else {
        return Fact::Unknown;
    };
    if installed == latest {
        return Fact::Text("deployed".into());
    }
    match (version_parts(installed), version_parts(latest)) {
        (Some(i), Some(l)) if i >= l => Fact::Text("deployed".into()),
        (Some(_), Some(_)) => Fact::Text("pending".into()),
        _ => Fact::Unknown,
    }
}

/// The real provider: disk and memory use from the latest sandbox sample, the pending sandbox
/// change from the Project, the latest workflow run from the connector and the chart deploy
/// state from the updater. A source that is missing, failed or has no answer is `Unknown`.
pub struct DbPlatformFacts {
    pub pool: PgPool,
    pub sandboxes: Arc<dyn SandboxResolver>,
    /// A sample older than this is not a fact.
    pub max_age: Duration,
    connector: Option<Arc<dyn Connector>>,
    deploy: Arc<dyn DeployStateSource>,
    cache: Mutex<HashMap<String, (Instant, Fact)>>,
}

impl DbPlatformFacts {
    pub fn new(pool: PgPool, sandboxes: Arc<dyn SandboxResolver>) -> Self {
        Self {
            pool,
            sandboxes,
            max_age: Duration::from_secs(600),
            connector: None,
            deploy: Arc::new(NoDeployState),
            cache: Mutex::default(),
        }
    }

    /// The connector `workflow_run.status` reads through.
    pub fn with_connector(mut self, connector: Arc<dyn Connector>) -> Self {
        self.connector = Some(connector);
        self
    }

    pub fn with_deploy_state(mut self, source: Arc<dyn DeployStateSource>) -> Self {
        self.deploy = source;
        self
    }

    fn cached(&self, key: &str) -> Option<Fact> {
        let c = self.cache.lock().expect("cache");
        c.get(key)
            .filter(|(at, _)| at.elapsed() < FACT_TTL)
            .map(|(_, f)| f.clone())
    }

    fn remember(&self, key: String, fact: &Fact) {
        self.cache
            .lock()
            .expect("cache")
            .insert(key, (Instant::now(), fact.clone()));
    }

    async fn workflow_run(&self, arg: &str) -> Fact {
        let Some(connector) = &self.connector else {
            return Fact::Unknown;
        };
        let key = format!("run:{arg}");
        if let Some(f) = self.cached(&key) {
            return f;
        }
        let Some((repo, file)) = arg.split_once(':') else {
            return Fact::Unknown;
        };
        // `owner/repo` is a GitHub repository, as everywhere else in the engine.
        let Some((owner, name)) = repo.split_once('/') else {
            return Fact::Unknown;
        };
        let repo = RepoKey::new("github", owner, name);
        let fact = match connector.latest_workflow_run(&repo, file, None).await {
            Ok(Some(run)) => Fact::Text(run.conclusion),
            Ok(None) => Fact::Unknown,
            Err(e) => {
                tracing::debug!("findings: latest run of {arg}: {e}");
                Fact::Unknown
            }
        };
        self.remember(key, &fact);
        fact
    }

    async fn chart_state(&self) -> Fact {
        if let Some(f) = self.cached("chart") {
            return f;
        }
        let fact = self.deploy.state().await;
        self.remember("chart".into(), &fact);
        fact
    }
}

#[async_trait]
impl PlatformFacts for DbPlatformFacts {
    async fn get_arg(&self, _project: &str, fact: PlatformFactName, arg: &str) -> Fact {
        match fact {
            PlatformFactName::WorkflowRunStatus => self.workflow_run(arg).await,
            _ => Fact::Unknown,
        }
    }

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
            PlatformFactName::SandboxMemWorkingSetPct => {
                // The working set (`memory.current` less the reclaimable `inactive_file` page
                // cache) over the limit. Unknown without a limit (an unlimited cgroup or an
                // older sandbox); a sandbox not reporting `inactive_file` counts it as 0.
                let row: Result<Option<(i64, i64)>, _> = sqlx::query_as(
                    "SELECT GREATEST(memory_bytes - COALESCE(memory_inactive_file_bytes, 0), 0),
                            memory_limit_bytes FROM engine.sandbox_stats
                     WHERE project = $1 AND at > now() - make_interval(secs => $2)
                       AND memory_bytes IS NOT NULL AND memory_limit_bytes > 0
                     ORDER BY at DESC LIMIT 1",
                )
                .bind(project)
                .bind(self.max_age.as_secs_f64())
                .fetch_optional(&self.pool)
                .await;
                match row {
                    Ok(Some((used, limit))) => Fact::Num(used as f64 * 100.0 / limit as f64),
                    _ => Fact::Unknown,
                }
            }
            PlatformFactName::SandboxHealthy => {
                let fresh: Result<bool, _> = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM engine.sandbox_stats
                     WHERE project = $1 AND at > now() - make_interval(secs => $2))",
                )
                .bind(project)
                .bind(self.max_age.as_secs_f64())
                .fetch_one(&self.pool)
                .await;
                fresh.map_or(Fact::Unknown, Fact::Bool)
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
            PlatformFactName::ChartDeployState => self.chart_state().await,
            PlatformFactName::WorkflowRunStatus => Fact::Unknown,
        }
    }
}

// ------------------------------------------------------------------ snapshot

/// What a task looks like to the checks.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskSnap {
    pub id: i64,
    pub project: String,
    /// The task's repo, where its issues are filed unless the check names one.
    pub repo: String,
    pub phase: String,
    pub node: String,
    pub waiting_on: Option<String>,
    /// The failure kind of the platform wait the task is in (ADR-0026), if it is in one.
    pub platform_wait: Option<String>,
    /// Since when an inbox event of the task keeps failing, with its attempts and last error.
    pub event_failing: Option<EventFailing>,
    /// Stored facts by name.
    pub facts: BTreeMap<String, FactRow>,
    /// Relation rows the task holds, by kind (`blocked_by`, `child`, ...).
    pub relations: BTreeMap<String, u64>,
    /// Relation rows other tasks hold towards the task's subjects, by kind (`parent`, ...).
    pub relations_in: BTreeMap<String, u64>,
    /// The `depends_on` entries the epic's breakdown declared for this sub-issue.
    pub declared_dependencies: u64,
    /// The latest command event routed to the task, and its name (no slash).
    pub last_command: Option<(DateTime<Utc>, String)>,
    /// The last transition that changed the task's node or status, or had effects.
    pub last_handled_at: Option<DateTime<Utc>>,
    /// The last event routed to the task, else its creation.
    pub last_event_at: DateTime<Utc>,
    /// The last step activity, else the task's creation. A running step counts as `now`.
    pub last_step_at: DateTime<Utc>,
    /// The task's own source issue was filed by a finding (it carries the `file_issue` marker).
    /// Findings on it are recorded but never file another issue (ADR-0024).
    pub issue_from_finding: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventFailing {
    pub since: DateTime<Utc>,
    pub attempts: i32,
    pub error: String,
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
    /// Platform facts that take an argument, by `(project, fact, arg)`.
    pub platform_args: HashMap<(String, PlatformFactName, String), Fact>,
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
    /// The issue to file when the finding opens.
    pub issue: Option<DesiredIssue>,
}

/// A rendered `issue:` block. `repo` is `None` for a task without one.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredIssue {
    /// `finding:<finding key>`: stable, so the connector dedupes by it.
    pub key: String,
    pub repo: Option<String>,
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
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
    let mut out = Vec::new();
    for c in checks {
        when_reads(&c.when, &mut out);
    }
    out
}

/// The platform facts one condition names, added to `out` once each.
pub fn when_reads(w: &When, out: &mut Vec<PlatformFactName>) {
    match w {
        When::All(v) | When::Any(v) => v.iter().for_each(|w| when_reads(w, out)),
        When::Not(w) => when_reads(w, out),
        When::Platform(p) if p.arg.is_none() && !out.contains(&p.fact) => out.push(p.fact),
        _ => {}
    }
}

/// The `(fact, arg)` pairs the checks read with an argument.
pub fn platform_arg_reads(checks: &[CheckFile]) -> Vec<(PlatformFactName, String)> {
    fn walk(w: &When, out: &mut Vec<(PlatformFactName, String)>) {
        match w {
            When::All(v) | When::Any(v) => v.iter().for_each(|w| walk(w, out)),
            When::Not(w) => walk(w, out),
            When::Platform(p) => {
                if let Some(a) = &p.arg
                    && !out.iter().any(|(f, x)| *f == p.fact && x == a)
                {
                    out.push((p.fact, a.clone()));
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for c in checks {
        walk(&c.when, &mut out);
    }
    out
}

/// Pure: does a platform-only condition hold for `project`? `None` when a fact it needs is
/// unknown (a platform wait then keeps waiting).
pub fn until_holds(
    until: &When,
    project: &str,
    platform: &HashMap<(String, PlatformFactName), Fact>,
) -> Option<bool> {
    let snap = Snapshot {
        platform: platform.clone(),
        ..Snapshot::default()
    };
    let mut cx = Cx {
        snap: &snap,
        project,
        task: None,
        now: Utc::now(),
        unknown: false,
    };
    let hit = cx.eval(until);
    (!cx.unknown).then_some(hit)
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
                // A task born from a finding's issue must not file another one: that loops
                // (finding -> issue -> task -> finding -> ...). The finding is still recorded.
                issue: c
                    .issue
                    .as_ref()
                    .filter(|_| !self.task.is_some_and(|t| t.issue_from_finding))
                    .map(|i| self.render_issue(i, &key)),
                key,
            });
        }
    }

    fn render_issue(&self, i: &IssueSpec, key: &str) -> DesiredIssue {
        DesiredIssue {
            key: format!("finding:{key}"),
            repo: i
                .repo
                .clone()
                .or_else(|| self.task.map(|t| t.repo.clone()).filter(|r| !r.is_empty())),
            title: self.render(&i.title),
            body: self.render(&i.body),
            labels: i.labels.clone(),
        }
    }

    fn render(&self, text: &str) -> String {
        let t = self.task;
        text.replace("{project}", self.project)
            .replace("{task}", &t.map_or(String::new(), |t| t.id.to_string()))
            .replace("{node}", t.map_or("", |t| t.node.as_str()))
            .replace("{phase}", t.map_or("", |t| t.phase.as_str()))
            .replace(
                "{attempts}",
                &t.and_then(|t| t.event_failing.as_ref())
                    .map_or(String::new(), |f| f.attempts.to_string()),
            )
            .replace(
                "{error}",
                t.and_then(|t| t.event_failing.as_ref())
                    .map_or("", |f| f.error.as_str()),
            )
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
            When::CommandIgnored(c) => self.task.is_some_and(|t| {
                let Some((at, name)) = &t.last_command else {
                    return false;
                };
                let named = c
                    .command
                    .as_ref()
                    .is_none_or(|w| w.trim_start_matches('/').eq_ignore_ascii_case(name));
                let handled = t.last_handled_at.is_some_and(|h| h > *at);
                let old = parse_duration(&c.for_).is_some_and(|secs| {
                    (self.now - *at).num_seconds() >= i64::try_from(secs).unwrap_or(i64::MAX)
                });
                named && old && !handled
            }),
            When::RelationCount(r) => self.task.is_some_and(|t| {
                let n = if r.inbound {
                    &t.relations_in
                } else {
                    &t.relations
                };
                let n = n.get(&r.kind).copied().unwrap_or(0);
                match (r.eq, r.gt) {
                    (Some(eq), _) => n == eq,
                    (_, Some(gt)) => n > gt,
                    _ => false,
                }
            }),
            When::DeclaredDependencies(c) => self.task.is_some_and(|t| match (c.eq, c.gt) {
                (Some(eq), _) => t.declared_dependencies == eq,
                (_, Some(gt)) => t.declared_dependencies > gt,
                _ => false,
            }),
            When::Platform(p) => self.platform(p),
            When::PlatformWait(w) => self.task.is_some_and(|t| {
                t.platform_wait
                    .as_ref()
                    .is_some_and(|k| w.kind.as_ref().is_none_or(|want| want == k))
            }),
            When::EventFailing(e) => self.task.is_some_and(|t| {
                t.event_failing.as_ref().is_some_and(|f| match &e.for_ {
                    None => true,
                    Some(d) => parse_duration(d).is_some_and(|secs| {
                        (self.now - f.since).num_seconds()
                            >= i64::try_from(secs).unwrap_or(i64::MAX)
                    }),
                })
            }),
        }
    }

    fn platform(&mut self, p: &PlatformCmp) -> bool {
        let fact = match &p.arg {
            Some(arg) => {
                self.snap
                    .platform_args
                    .get(&(self.project.to_string(), p.fact, arg.clone()))
            }
            None => self.snap.platform.get(&(self.project.to_string(), p.fact)),
        }
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
            // The finding and its issue job commit together, so a failed enqueue leaves no open
            // finding and the next pass tries again.
            let mut tx = pool.begin().await?;
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
            .execute(&mut *tx)
            .await;
            match r {
                Ok(_) => {}
                // The task was deleted since the snapshot.
                Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => continue,
                Err(e) => return Err(e.into()),
            }
            if let Some(issue) = &d.issue
                && let Err(e) = enqueue_issue(&mut tx, d, issue).await
            {
                tracing::warn!(key = %d.key, "findings: enqueuing the issue: {e}");
                next.insert(d.key.clone());
                continue;
            }
            tx.commit().await?;
            st.raised += 1;
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

/// Queues the `file_issue` job of a finding that just opened. The job's idempotency key is the
/// issue key, so a finding that clears and opens again never queues a second one. A task
/// without a repo keeps its finding and files nothing.
async fn enqueue_issue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    d: &Desired,
    issue: &DesiredIssue,
) -> Result<(), sqlx::Error> {
    let Some(repo) = &issue.repo else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO engine.jobs (task_id, project, kind, input, idempotency_key, priority)
         VALUES ($1, $2, 'file_issue', $3, $4, 1) ON CONFLICT (idempotency_key) DO NOTHING",
    )
    .bind(d.task_id)
    .bind(&d.project)
    .bind(serde_json::json!({
        "repo": repo, "key": issue.key, "title": issue.title, "body": issue.body,
        "labels": issue.labels, "finding": d.key,
    }))
    .bind(&issue.key)
    .execute(&mut **tx)
    .await?;
    Ok(())
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
            for (fact, arg) in platform_arg_reads(c) {
                let v = self.findings.facts.get_arg(project, fact, &arg).await;
                snap.platform_args.insert((project.clone(), fact, arg), v);
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
                platform_args: snap.platform_args.clone(),
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
                END,
                t.repo,
                c.at, c.name,
                (SELECT max(x.at) FROM engine.transitions x
                 WHERE x.task_id = t.id
                   AND (x.from_state IS DISTINCT FROM x.to_state OR x.effects <> '[]'::jsonb)),
                t.state#>>'{platform_wait,kind}',
                t.state #>> '{doc,subjects,issue,body}',
                f.since, f.attempts, f.last_error
         FROM engine.tasks t LEFT JOIN engine.projection p ON p.task_id = t.id
         LEFT JOIN LATERAL (
             SELECT e.received_at AS at,
                    CASE WHEN e.type = 'comment' THEN e.payload->>'command'
                         ELSE e.payload#>>'{data,command}' END AS name
             FROM engine.events e
             WHERE e.task_id = t.id
               AND ((e.type = 'comment' AND COALESCE(e.payload->>'command', '') <> '')
                    OR (e.type = 'ui.command' AND COALESCE(e.payload#>>'{data,command}', '') <> ''))
             ORDER BY e.id DESC LIMIT 1
         ) c ON true
         LEFT JOIN LATERAL (
             SELECT min(e.received_at) AS since,
                    (array_agg(e.attempts ORDER BY e.id))[1] AS attempts,
                    (array_agg(e.last_error ORDER BY e.id))[1] AS last_error
             FROM engine.events e
             WHERE e.task_id = t.id AND e.processed_at IS NULL AND e.attempts > 0
         ) f ON true
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
            platform_wait: r.get(11),
            event_failing: r
                .get::<Option<DateTime<Utc>>, _>(13)
                .map(|since| EventFailing {
                    since,
                    attempts: r.get::<Option<i32>, _>(14).unwrap_or(0),
                    error: r.get::<Option<String>, _>(15).unwrap_or_default(),
                }),
            repo: r.get(7),
            facts: BTreeMap::new(),
            relations: BTreeMap::new(),
            relations_in: BTreeMap::new(),
            declared_dependencies: 0,
            last_command: r.get::<Option<DateTime<Utc>>, _>(8).map(|at| {
                let name: Option<String> = r.get(9);
                let name = name.unwrap_or_default();
                (at, name.trim_start_matches('/').to_ascii_lowercase())
            }),
            last_handled_at: r.get(10),
            last_event_at: r.get(5),
            last_step_at: r.get(6),
            issue_from_finding: r
                .get::<Option<String>, _>(12)
                .is_some_and(|b| has_file_issue_marker(&b)),
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
    // Relation rows: the ones each task holds, and the ones other tasks hold towards its
    // subjects (`task:<id>` too).
    let held = sqlx::query(
        "SELECT task_id, kind, count(*) FROM engine.relations WHERE task_id = ANY($1)
         GROUP BY task_id, kind",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    for r in held {
        if let Some(&i) = index.get(&r.get::<i64, _>(0)) {
            tasks[i]
                .relations
                .insert(r.get(1), u64::try_from(r.get::<i64, _>(2)).unwrap_or(0));
        }
    }
    let towards = sqlx::query(
        "SELECT k.task_id, r.kind, count(DISTINCT (r.task_id, r.kind, r.target_key))
         FROM (SELECT task_id, key FROM engine.subjects WHERE task_id = ANY($1)
               UNION SELECT id, 'task:' || id FROM engine.tasks WHERE id = ANY($1)) k
         JOIN engine.relations r ON r.target_key = k.key AND r.task_id <> k.task_id
         GROUP BY k.task_id, r.kind",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    for r in towards {
        if let Some(&i) = index.get(&r.get::<i64, _>(0)) {
            tasks[i]
                .relations_in
                .insert(r.get(1), u64::try_from(r.get::<i64, _>(2)).unwrap_or(0));
        }
    }
    // What each sub-issue's epic breakdown declared for it: the epic's `children` result lists
    // the created issues in the order of its `breakdown` issues, which carry `depends_on`.
    let declared = sqlx::query(
        "SELECT k.task_id,
                COALESCE(max(jsonb_array_length(CASE WHEN jsonb_typeof(b.issue->'depends_on') = 'array'
                                                THEN b.issue->'depends_on' ELSE '[]'::jsonb END)), 0)
         FROM (SELECT task_id, key FROM engine.subjects WHERE task_id = ANY($1)) k
         JOIN engine.relations r ON r.target_key = k.key AND r.kind = 'parent' AND r.task_id <> k.task_id
         JOIN engine.tasks e ON e.id = r.task_id
         CROSS JOIN LATERAL jsonb_array_elements(COALESCE(e.state->'doc'->'outputs'->'children'->'issues', '[]'::jsonb))
                    WITH ORDINALITY AS c(issue, n)
         JOIN LATERAL (SELECT x.issue FROM jsonb_array_elements(
                           COALESCE(e.state->'doc'->'outputs'->'breakdown'->'issues', '[]'::jsonb))
                       WITH ORDINALITY AS x(issue, n) WHERE x.n = c.n) b ON true
         WHERE c.issue->>'key' = k.key
         GROUP BY k.task_id",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    for r in declared {
        if let Some(&i) = index.get(&r.get::<i64, _>(0)) {
            tasks[i].declared_dependencies = u64::try_from(r.get::<i32, _>(1)).unwrap_or(0);
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
            repo: "o/r".into(),
            phase: "review".into(),
            node: "review/wait".into(),
            waiting_on: Some("external".into()),
            platform_wait: None,
            event_failing: None,
            facts: BTreeMap::new(),
            relations: BTreeMap::new(),
            relations_in: BTreeMap::new(),
            declared_dependencies: 0,
            last_command: None,
            last_handled_at: None,
            last_event_at: now() - chrono::Duration::hours(2),
            last_step_at: now() - chrono::Duration::hours(3),
            issue_from_finding: false,
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
            platform_args: HashMap::new(),
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
    fn event_failing_leaf_and_placeholders() {
        let mut t = task(1);
        t.event_failing = Some(EventFailing {
            since: now() - chrono::Duration::minutes(10),
            attempts: 4,
            error: "boom".into(),
        });
        let failing = snap(vec![t]);
        let healthy = snap(vec![task(1)]);
        for (when, s, want) in [
            (json!({"event_failing": {}}), &failing, true),
            (json!({"event_failing": {"for": "5m"}}), &failing, true),
            (json!({"event_failing": {"for": "30m"}}), &failing, false),
            (json!({"event_failing": {}}), &healthy, false),
        ] {
            assert_eq!(!hits(&check(when.clone()), s).is_empty(), want, "{when}");
        }
        let mut c = check(json!({"event_failing": {}}));
        c.text = "attempt {attempts}: {error}".into();
        let e = evaluate(&[c], &failing, now());
        assert_eq!(e.desired[0].text, "attempt 4: boom");
    }

    #[test]
    fn declared_dependencies_leaf() {
        let mut t = task(1);
        let none = snap(vec![t.clone()]);
        t.declared_dependencies = 2;
        let two = snap(vec![t]);
        for (when, snap_, want) in [
            (json!({"declared_dependencies": {"gt": 0}}), &none, false),
            (json!({"declared_dependencies": {"gt": 0}}), &two, true),
            (json!({"declared_dependencies": {"gt": 2}}), &two, false),
            (json!({"declared_dependencies": {"eq": 2}}), &two, true),
            (json!({"declared_dependencies": {"eq": 0}}), &none, true),
        ] {
            let got = !hits(&check(when.clone()), snap_).is_empty();
            assert_eq!(got, want, "{when}");
        }
    }

    #[test]
    fn command_ignored_and_relation_leaves() {
        let mut t = task(1);
        t.last_command = Some((now() - chrono::Duration::minutes(20), "retry".into()));
        t.relations_in.insert("parent".into(), 1);
        t.relations.insert("child".into(), 2);
        let s = snap(vec![t.clone()]);
        let mut handled = t.clone();
        handled.last_handled_at = Some(now() - chrono::Duration::minutes(5));
        let mut earlier = t.clone();
        earlier.last_handled_at = Some(now() - chrono::Duration::minutes(30));
        let mut recent = t.clone();
        recent.last_command = Some((now() - chrono::Duration::minutes(5), "retry".into()));
        let mut none = t.clone();
        none.last_command = None;
        for (when, snap_, want) in [
            (json!({"command_ignored": {"for": "15m"}}), &s, true),
            (json!({"command_ignored": {"for": "30m"}}), &s, false),
            (
                json!({"command_ignored": {"for": "15m", "command": "retry"}}),
                &s,
                true,
            ),
            (
                json!({"command_ignored": {"for": "15m", "command": "/Retry"}}),
                &s,
                true,
            ),
            (
                json!({"command_ignored": {"for": "15m", "command": "plan"}}),
                &s,
                false,
            ),
            (
                json!({"relation_count": {"kind": "parent", "inbound": true, "gt": 0}}),
                &s,
                true,
            ),
            (
                json!({"relation_count": {"kind": "parent", "gt": 0}}),
                &s,
                false,
            ),
            (
                json!({"relation_count": {"kind": "blocked_by", "eq": 0}}),
                &s,
                true,
            ),
            (
                json!({"relation_count": {"kind": "child", "eq": 2}}),
                &s,
                true,
            ),
            (
                json!({"relation_count": {"kind": "child", "gt": 2}}),
                &s,
                false,
            ),
        ] {
            let sn = snap(vec![snap_.tasks[0].clone()]);
            assert_eq!(!hits(&check(when.clone()), &sn).is_empty(), want, "{when}");
        }
        // A transition after the command (a change or an effect) means it was not ignored; one
        // before it does not.
        let when = json!({"command_ignored": {"for": "15m"}});
        for (task_, want) in [
            (handled, false),
            (earlier, true),
            (recent, false),
            (none, false),
        ] {
            let sn = snap(vec![task_]);
            assert_eq!(!hits(&check(when.clone()), &sn).is_empty(), want);
        }
    }

    #[test]
    fn issue_from_a_finding_is_not_filed_again() {
        let mut c = check(json!({"waiting_on": "external"}));
        c.issue = Some(IssueSpec {
            title: "t".into(),
            body: "b".into(),
            labels: vec![],
            repo: None,
        });
        let mut t = task(7);
        let e = evaluate(std::slice::from_ref(&c), &snap(vec![t.clone()]), now());
        assert!(e.desired[0].issue.is_some());
        t.issue_from_finding = true;
        let e = evaluate(std::slice::from_ref(&c), &snap(vec![t]), now());
        assert_eq!(e.desired.len(), 1);
        assert!(e.desired[0].issue.is_none());
    }

    #[test]
    fn issue_is_rendered_with_a_stable_key_and_the_task_repo() {
        let mut c = check(json!({"waiting_on": "external"}));
        c.issue = Some(IssueSpec {
            title: "Task {task} at {node}".into(),
            body: "in {project}".into(),
            labels: vec!["liveness".into()],
            repo: None,
        });
        let s = snap(vec![task(7)]);
        let e = evaluate(std::slice::from_ref(&c), &s, now());
        let i = e.desired[0].issue.clone().unwrap();
        assert_eq!(
            (
                i.key.as_str(),
                i.repo.as_deref(),
                i.title.as_str(),
                i.body.as_str()
            ),
            ("finding:c:7", Some("o/r"), "Task 7 at review/wait", "in p")
        );
        // Stable across evaluations; an explicit repo wins.
        assert_eq!(e, evaluate(std::slice::from_ref(&c), &s, now()));
        c.issue.as_mut().unwrap().repo = Some("x/y".into());
        let e = evaluate(std::slice::from_ref(&c), &s, now());
        assert_eq!(
            e.desired[0].issue.as_ref().unwrap().repo.as_deref(),
            Some("x/y")
        );
        // Platform scope: keyed by project.
        let mut p = platform_check(
            json!({"platform": {"fact": "sandbox.disk_pct", "op": "ge", "value": 90}}),
        );
        p.issue = c.issue.clone();
        let mut s = snap(vec![]);
        s.platform.insert(
            ("p".into(), PlatformFactName::SandboxDiskPct),
            Fact::Num(95.0),
        );
        let e = evaluate(&[p], &s, now());
        assert_eq!(e.desired[0].issue.as_ref().unwrap().key, "finding:c:p");
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
            json!({"platform": {"fact": "sandbox.mem_working_set_pct", "op": "gt", "value": 80}}),
        );
        put(
            &mut s,
            PlatformFactName::SandboxMemWorkingSetPct,
            Fact::Num(80.5),
        );
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

    #[test]
    fn deploy_state_from_the_updater_status() {
        let st = |v: Value| deploy_state(&v);
        let text = |s: &str| Fact::Text(s.into());
        let base =
            json!({"installed": "0.62.0", "desired": "0.62.0", "latest": "0.62.0", "stuck": false});
        assert_eq!(st(base), text("deployed"));
        assert_eq!(
            st(json!({"installed": "0.9.0", "latest": "0.10.0", "stuck": false})),
            text("pending")
        );
        assert_eq!(
            st(json!({"installed": "v0.10.0", "latest": "0.9.0"})),
            text("deployed")
        );
        assert_eq!(
            st(json!({"installed": "0.1.0", "latest": "0.2.0", "stuck": true})),
            text("stuck")
        );
        assert_eq!(
            st(json!({"installed": "0.1.0", "stuck": false})),
            Fact::Unknown
        );
        assert_eq!(st(json!({"latest": "0.1.0"})), Fact::Unknown);
        assert_eq!(
            st(json!({"installed": "abc", "latest": "def"})),
            Fact::Unknown
        );
    }

    /// Serves `body` once per connection with `status` on a local port.
    async fn stub(status: u16, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/status", l.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let r = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(r.as_bytes()).await;
            }
        });
        url
    }

    #[tokio::test]
    async fn http_deploy_state_maps_the_status_and_errors_are_unknown() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let url = stub(
            200,
            r#"{"installed":"0.1.0","desired":"0.2.0","latest":"0.2.0","stuck":false}"#,
        )
        .await;
        assert_eq!(
            HttpDeployState::new(url).state().await,
            Fact::Text("pending".into())
        );
        let url = stub(500, "{}").await;
        assert_eq!(HttpDeployState::new(url).state().await, Fact::Unknown);
        let url = stub(200, "not json").await;
        assert_eq!(HttpDeployState::new(url).state().await, Fact::Unknown);
        assert_eq!(
            HttpDeployState::new("http://127.0.0.1:1/status")
                .state()
                .await,
            Fact::Unknown
        );
    }
}
