//! Executor integration tests: Postgres (testcontainers), `FakeConnector`, and a fake
//! sandbox-init (a real enzarb-proto websocket server in this process that accepts
//! `run_start` and POSTs the callback).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use engine::db::Job;
use engine::executor::{SandboxTarget, StaticSandboxResolver, callback_token};
use engine::machine::JobKind;
use engine::testing::TestDb;
use engine::{
    ConnectorRegistry, Executor, ExecutorConfig, Reducer, ReducerConfig, StaticPermissions,
    StaticProjectIndex,
};
use enzarb_connector::CommentKind;
use enzarb_connector::fake::{Call, FakeConnector};
use enzarb_connector::{
    Actor, ChangeRequestSpec, CommentRef, CredScope, Event, EventData, EventType, ItemKind,
    ItemRef, Permission, Progress, ProgressTarget, RepoKey, Thread, ThreadComment, WorkItemKey,
};
use enzarb_proto::session::{Event as SessionEvent, ServerOptions, Session};
use enzarb_proto::{Ctrl, RunFinished, RunOutcome, RunPhase, RunStart};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

macro_rules! db {
    () => {
        match TestDb::start().await {
            Some(db) => db,
            None => return,
        }
    };
}

const REPO: &str = "github:enzarb/enzarb-ui";
const SANDBOX_TOKEN: &str = "sandbox-secret";

// ---------------------------------------------------------------------------
// Fake sandbox-init
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Mode {
    /// Accept the run and keep it running; the test finishes it.
    Hold,
    /// Accept, then finish. `callback: false` leaves the end to `run_status`.
    Finish {
        outcome: RunOutcome,
        summary: String,
        pushed: bool,
        callback: bool,
    },
}

struct Run {
    start: RunStart,
    phase: RunPhase,
    summary: Option<String>,
}

struct Sb {
    mode: Mutex<Mode>,
    runs: Mutex<HashMap<String, Run>>,
    starts: Mutex<usize>,
    cancels: Mutex<Vec<String>>,
    callbacks: Mutex<Vec<u16>>,
}

#[derive(Clone)]
struct FakeSandbox {
    sb: Arc<Sb>,
    url: String,
}

impl FakeSandbox {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/v1/mux", listener.local_addr().unwrap());
        let sb = Arc::new(Sb {
            mode: Mutex::new(Mode::Hold),
            runs: Mutex::default(),
            starts: Mutex::new(0),
            cancels: Mutex::default(),
            callbacks: Mutex::default(),
        });
        let s = sb.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(s.clone(), stream));
            }
        });
        Self { sb, url }
    }

    fn set_mode(&self, m: Mode) {
        *self.sb.mode.lock().unwrap() = m;
    }

    fn starts(&self) -> usize {
        *self.sb.starts.lock().unwrap()
    }

    fn run(&self, run_id: &str) -> Option<RunStart> {
        self.sb
            .runs
            .lock()
            .unwrap()
            .get(run_id)
            .map(|r| r.start.clone())
    }

    fn cancels(&self) -> Vec<String> {
        self.sb.cancels.lock().unwrap().clone()
    }

    /// Make the sandbox forget a run, as after a restart.
    fn forget(&self, run_id: &str) {
        self.sb.runs.lock().unwrap().remove(run_id);
    }

    fn finish_with_state(&self, run_id: &str, phase: RunPhase, summary: &str) {
        let mut runs = self.sb.runs.lock().unwrap();
        let r = runs.get_mut(run_id).unwrap();
        r.phase = phase;
        r.summary = Some(summary.into());
    }
}

async fn serve(sb: Arc<Sb>, stream: tokio::net::TcpStream) {
    let want = format!("Bearer {SANDBOX_TOKEN}");
    #[allow(clippy::result_large_err)]
    let check = move |req: &Request, resp: Response| {
        let ok = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            == Some(want.as_str());
        if ok {
            Ok(resp)
        } else {
            Err(ErrorResponse::new(Some("unauthorized".into())))
        }
    };
    let ws = tokio_tungstenite::accept_hdr_async(stream, check).await;
    let Ok(ws) = ws else { return };
    let Ok((session, mut events)) = Session::accept(ws, ServerOptions::default()).await else {
        return;
    };
    while let Some(ev) = events.next().await {
        let SessionEvent::Ctrl(ctrl) = ev else {
            continue;
        };
        match ctrl {
            Ctrl::RunStart(run) => {
                let existing = sb.runs.lock().unwrap().contains_key(&run.run_id);
                let phase = if existing {
                    sb.runs.lock().unwrap()[&run.run_id].phase
                } else {
                    *sb.starts.lock().unwrap() += 1;
                    sb.runs.lock().unwrap().insert(
                        run.run_id.clone(),
                        Run {
                            start: run.clone(),
                            phase: RunPhase::Running,
                            summary: None,
                        },
                    );
                    RunPhase::Running
                };
                let _ = session.accept_run(&run.run_id, phase, existing, None);
                if existing {
                    continue;
                }
                let mode = sb.mode.lock().unwrap().clone();
                if let Mode::Finish {
                    outcome,
                    summary,
                    pushed,
                    callback,
                } = mode
                {
                    let phase = match outcome {
                        RunOutcome::Succeeded => RunPhase::Succeeded,
                        RunOutcome::Failed => RunPhase::Failed,
                        RunOutcome::Cancelled => RunPhase::Cancelled,
                    };
                    {
                        let mut runs = sb.runs.lock().unwrap();
                        let r = runs.get_mut(&run.run_id).unwrap();
                        r.phase = phase;
                        r.summary = Some(summary.clone());
                    }
                    if callback {
                        let sb = sb.clone();
                        tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(150)).await;
                            let body = RunFinished {
                                run_id: run.run_id.clone(),
                                outcome,
                                exit_code: Some(0),
                                summary,
                                branch: run.branch.clone(),
                                head_sha: "abc123".into(),
                                pushed,
                            };
                            let resp = reqwest::Client::new()
                                .post(&run.callback_url)
                                .bearer_auth(&run.callback_token)
                                .json(&body)
                                .send()
                                .await
                                .unwrap();
                            sb.callbacks.lock().unwrap().push(resp.status().as_u16());
                        });
                    }
                }
            }
            Ctrl::RunStatus { run_id } => {
                let info =
                    sb.runs
                        .lock()
                        .unwrap()
                        .get(&run_id)
                        .map(|r| enzarb_proto::session::RunInfo {
                            run_id: run_id.clone(),
                            state: r.phase,
                            exit_code: r.phase.is_terminal().then_some(0),
                            summary: r.summary.clone(),
                        });
                match info {
                    Some(i) => {
                        let _ = session.send_run_state(&i);
                    }
                    None => {
                        let _ = session.send_run_error(&run_id, "unknown_run", "unknown run");
                    }
                }
            }
            Ctrl::RunCancel { run_id } => {
                sb.cancels.lock().unwrap().push(run_id.clone());
                if let Some(r) = sb.runs.lock().unwrap().get_mut(&run_id) {
                    r.phase = RunPhase::Cancelled;
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct H {
    db: TestDb,
    fake: Arc<FakeConnector>,
    sandbox: FakeSandbox,
    resolver: Arc<StaticSandboxResolver>,
    exec: Arc<Executor>,
    base: String,
}

impl H {
    async fn new(db: TestDb) -> Self {
        Self::with(db, |_| {}).await
    }

    async fn with(db: TestDb, tweak: impl FnOnce(&mut ExecutorConfig)) -> Self {
        let fake = Arc::new(FakeConnector::new());
        let sandbox = FakeSandbox::start().await;
        let resolver = Arc::new(StaticSandboxResolver::new());
        resolver.insert(
            "enzarb",
            SandboxTarget {
                endpoint: sandbox.url.clone(),
                token: SANDBOX_TOKEN.into(),
            },
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut cfg = ExecutorConfig::new(base.clone(), b"test-secret".to_vec());
        cfg.backoff_base = Duration::from_millis(1);
        cfg.sandbox_timeout = Duration::from_secs(5);
        tweak(&mut cfg);
        let exec = Arc::new(Executor::new(
            db.pool.clone(),
            ConnectorRegistry::new().with("github", fake.clone()),
            resolver.clone(),
            cfg,
        ));
        let router = exec.callback_router();
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            db,
            fake,
            sandbox,
            resolver,
            exec,
            base,
        }
    }

    fn pool(&self) -> &PgPool {
        &self.db.pool
    }

    fn executor_like(&self) -> Arc<Executor> {
        Arc::new(Executor::new(
            self.db.pool.clone(),
            ConnectorRegistry::new().with("github", self.fake.clone()),
            self.resolver.clone(),
            self.exec.config().clone(),
        ))
    }
}

fn key(n: u64) -> WorkItemKey {
    WorkItemKey::new(REPO.parse().unwrap(), n)
}

async fn new_item(pool: &PgPool, n: u64, project: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflows.work_items (key, project_id, workflow, state)
         VALUES ($1, $2, 'issue', 'Triaging') RETURNING id",
    )
    .bind(key(n).to_string())
    .bind(project)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn new_job(
    pool: &PgPool,
    wi: i64,
    project: &str,
    kind: &str,
    priority: i32,
    idem: &str,
    input: Value,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflows.jobs (work_item_id, project_id, kind, priority, idempotency_key, input)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(wi)
    .bind(project)
    .bind(kind)
    .bind(priority)
    .bind(idem)
    .bind(input)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn comment_input(n: u64, body: &str) -> Value {
    json!({ "item": key(n), "body": body })
}

fn progress_input(n: u64, p: Progress) -> Value {
    json!({ "target": ProgressTarget { item: key(n), comment_id: None, comment_kind: Default::default() }, "progress": p })
}

fn agent_input(n: u64, branch: &str, push: bool) -> Value {
    let kind = if push {
        JobKind::AgentImplement
    } else {
        JobKind::AgentTriage
    };
    let ctx = engine::machine::PromptContext {
        repo: REPO.into(),
        title: "Add dark mode".into(),
        number: n.to_string(),
        ..Default::default()
    };
    json!({
        "kind": kind, "repo": REPO.parse::<RepoKey>().unwrap(), "base_ref": "main",
        "branch": branch, "push": push, "timeout_secs": 600,
        "context": ctx, "prompt": "stale prompt",
    })
}

async fn job(pool: &PgPool, id: i64) -> Job {
    sqlx::query_as("SELECT * FROM workflows.jobs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn events_of(pool: &PgPool, ty: &str) -> Vec<(String, Value)> {
    sqlx::query_as("SELECT dedupe_key, payload FROM workflows.events WHERE type = $1 ORDER BY id")
        .bind(ty)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn wait_for<F, Fut>(what: &str, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..100 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out waiting for {what}");
}

fn thread_with(text: &str) -> Thread {
    Thread {
        item: ItemRef {
            kind: ItemKind::Issue,
            number: 1,
            url: "https://github.com/enzarb/enzarb-ui/issues/1".into(),
            title: "Add dark mode".into(),
            body: "Please add a dark theme.".into(),
            author: "alice".into(),
            head_ref: None,
            base_ref: None,
            head_sha: None,
        },
        comments: vec![ThreadComment {
            author: Actor {
                login: "bob".into(),
                id: 2,
                is_bot: false,
            },
            comment: CommentRef {
                id: 11,
                kind: Default::default(),
                body: text.into(),
                url: String::new(),
            },
            created_at: Some(Utc::now()),
        }],
    }
}

// ---------------------------------------------------------------------------
// Connector jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn connector_jobs_run_with_idempotency_keys_and_report() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let p = new_job(
        pool,
        wi,
        "enzarb",
        "set_progress",
        100,
        "k:p",
        progress_input(1, Progress::Received),
    )
    .await;
    let c = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        10,
        "k:c",
        comment_input(1, "hello"),
    )
    .await;
    let spec = ChangeRequestSpec {
        repo: REPO.parse().unwrap(),
        title: "t".into(),
        body: "b".into(),
        head_ref: "enzarb/issue-1".into(),
        base_ref: "main".into(),
        draft: false,
    };
    let o = new_job(
        pool,
        wi,
        "enzarb",
        "open_change_request",
        10,
        "k:o",
        json!({ "spec": spec }),
    )
    .await;

    assert_eq!(h.exec.drain().await, 3);

    for id in [p, c, o] {
        let j = job(pool, id).await;
        assert_eq!((j.status.as_str(), j.attempt), ("succeeded", 1));
        assert!(j.lease_owner.is_none() && j.lease_until.is_none());
    }
    // outputs are the shapes the machines read
    let out: CommentRef = serde_json::from_value(job(pool, c).await.output.unwrap()).unwrap();
    assert_eq!(out.body, "hello");
    let pr: ItemRef = serde_json::from_value(job(pool, o).await.output.unwrap()).unwrap();
    assert_eq!(pr.kind, ItemKind::ChangeRequest);

    let calls = h.fake.applied_calls();
    assert!(calls.contains(&Call::SetProgress {
        target: ProgressTarget {
            item: key(1),
            comment_id: None,
            comment_kind: Default::default()
        },
        progress: Progress::Received,
        idempotency_key: "k:p".into(),
    }));
    assert!(calls.iter().any(
        |c| matches!(c, Call::PostComment { idempotency_key, .. } if idempotency_key == "k:c")
    ));
    assert!(calls.iter().any(
        |c| matches!(c, Call::OpenChangeRequest { idempotency_key, .. } if idempotency_key == "k:o")
    ));

    let evs = events_of(pool, "JobSucceeded").await;
    assert_eq!(evs.len(), 3);
    assert!(evs.iter().any(|(k, _)| *k == format!("job:{p}:1")));
}

fn readiness(
    sha: &str,
    ci: enzarb_connector::CiState,
    mergeable: Option<bool>,
) -> enzarb_connector::MergeReadiness {
    enzarb_connector::MergeReadiness {
        head_sha: sha.into(),
        mergeable,
        mergeable_state: "clean".into(),
        ci,
    }
}

fn merge_calls(h: &H) -> Vec<Call> {
    h.fake
        .applied_calls()
        .into_iter()
        .filter(|c| matches!(c, Call::MergeChangeRequest { .. }))
        .collect()
}

#[tokio::test]
async fn check_merge_readiness_job_reports_the_aggregate() {
    use enzarb_connector::CiState;
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    h.fake
        .set_merge_readiness(key(1), readiness("sha1", CiState::Pending, None));
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "check_merge_readiness",
        10,
        "k:r",
        json!({ "item": key(1), "number": 1, "head_sha": "sha1" }),
    )
    .await;
    assert_eq!(h.exec.drain().await, 1);
    let out = job(pool, id).await.output.unwrap();
    assert_eq!(out["head_sha"], "sha1");
    assert_eq!(out["ci"], "pending");
    assert_eq!(out["mergeable"], Value::Null);
}

#[tokio::test]
async fn merge_job_rechecks_readiness_and_passes_the_idempotency_key() {
    use enzarb_connector::{CiState, MergeOutcome};
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let input = |sha: &str| {
        json!({ "item": key(1), "number": 1, "title": "feat: dark mode",
                "expected_head_sha": sha, "actor": "carol" })
    };

    // Not ready: the merge is refused without calling the connector's merge.
    for (n, (r, want, reason)) in [
        (
            readiness("sha2", CiState::Success, Some(true)),
            "head_moved",
            "",
        ),
        (
            readiness("sha1", CiState::Failure, Some(true)),
            "not_mergeable",
            "CI",
        ),
        (
            readiness("sha1", CiState::Pending, Some(true)),
            "not_mergeable",
            "CI",
        ),
        (
            readiness("sha1", CiState::Success, Some(false)),
            "not_mergeable",
            "not mergeable",
        ),
        (
            readiness("sha1", CiState::Success, None),
            "not_mergeable",
            "computed",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        h.fake.set_merge_readiness(key(1), r);
        let id = new_job(
            pool,
            wi,
            "enzarb",
            "merge_change_request",
            10,
            &format!("k:refused{n}"),
            input("sha1"),
        )
        .await;
        assert_eq!(h.exec.drain().await, 1);
        let j = job(pool, id).await;
        assert_eq!(j.status, "succeeded", "refusals are outputs, not failures");
        let out = j.output.unwrap();
        assert_eq!(out["outcome"], want, "case {n}");
        assert_eq!(out["actor"], "carol");
        assert!(out["reason"].as_str().unwrap_or("").contains(reason));
    }
    assert!(merge_calls(&h).is_empty(), "{:?}", merge_calls(&h));

    // Ready: merged with the job's idempotency key and the squash title.
    h.fake
        .set_merge_readiness(key(1), readiness("sha1", CiState::Success, Some(true)));
    h.fake
        .queue_merge_outcome(MergeOutcome::Merged { sha: "m1".into() });
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "merge_change_request",
        10,
        "k:merge",
        input("sha1"),
    )
    .await;
    assert_eq!(h.exec.drain().await, 1);
    let out = job(pool, id).await.output.unwrap();
    assert_eq!(out["outcome"], "merged");
    assert_eq!(out["sha"], "m1");
    assert_eq!(
        merge_calls(&h),
        [Call::MergeChangeRequest {
            item: key(1),
            number: 1,
            expected_head_sha: "sha1".into(),
            commit_title: "feat: dark mode".into(),
            commit_body: None,
            idempotency_key: "k:merge".into(),
        }]
    );
}

#[tokio::test]
async fn claims_by_priority_and_serializes_one_work_item() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    for (prio, body) in [(1, "low"), (50, "high"), (10, "mid")] {
        new_job(
            pool,
            wi,
            "enzarb",
            "post_comment",
            prio,
            &format!("k:{body}"),
            comment_input(1, body),
        )
        .await;
    }
    // one running connector job of the item blocks its siblings
    let running = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        99,
        "k:running",
        comment_input(1, "r"),
    )
    .await;
    sqlx::query("UPDATE workflows.jobs SET status='running', attempt=1 WHERE id=$1")
        .bind(running)
        .execute(pool)
        .await
        .unwrap();
    assert!(!h.exec.run_once().await.unwrap());

    sqlx::query("UPDATE workflows.jobs SET status='succeeded' WHERE id=$1")
        .bind(running)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.drain().await, 3);
    let order: Vec<String> = h
        .fake
        .applied_calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::PostComment { body, .. } => Some(body),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["high", "mid", "low"]);
}

#[tokio::test]
async fn concurrent_executors_never_double_claim() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    for n in 1..=30u64 {
        let wi = new_item(pool, n, "enzarb").await;
        new_job(
            pool,
            wi,
            "enzarb",
            "post_comment",
            10,
            &format!("k:{n}"),
            comment_input(n, "x"),
        )
        .await;
    }
    let (a, b) = (h.executor_like(), h.executor_like());
    let (x, y) = tokio::join!(
        tokio::spawn(async move { a.drain().await }),
        tokio::spawn(async move { b.drain().await }),
    );
    assert_eq!(x.unwrap() + y.unwrap(), 30);
    let attempts: Vec<i32> = sqlx::query_scalar("SELECT attempt FROM workflows.jobs")
        .fetch_all(pool)
        .await
        .unwrap();
    assert!(attempts.iter().all(|a| *a == 1));
    assert_eq!(h.fake.applied_calls().len(), 30);
}

#[tokio::test]
async fn transient_errors_back_off_then_succeed() {
    let h = H::with(db!(), |c| {
        c.backoff_base = Duration::from_secs(3600);
        c.backoff_max = Duration::from_secs(7200);
    })
    .await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        10,
        "k:c",
        comment_input(1, "x"),
    )
    .await;

    h.fake
        .fail_next(enzarb_connector::Error::Transient("502".into()));
    assert_eq!(h.exec.drain().await, 1);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.attempt), ("queued", 1));
    assert!(j.run_after > Utc::now() + chrono::Duration::minutes(30));
    assert!(events_of(pool, "JobFailed").await.is_empty());
    // not claimable while backing off
    assert_eq!(h.exec.drain().await, 0);

    sqlx::query("UPDATE workflows.jobs SET run_after = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.drain().await, 1);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.attempt), ("succeeded", 2));
    let evs = events_of(pool, "JobSucceeded").await;
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].0, format!("job:{id}:2"));
}

#[tokio::test]
async fn rate_limit_retry_after_is_honored() {
    let h = H::with(db!(), |c| c.backoff_base = Duration::from_millis(1)).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        10,
        "k:c",
        comment_input(1, "x"),
    )
    .await;
    h.fake.fail_next(enzarb_connector::Error::RateLimited {
        retry_after: Some(Duration::from_secs(600)),
    });
    h.exec.drain().await;
    let j = job(pool, id).await;
    assert_eq!(j.status, "queued");
    assert!(j.run_after > Utc::now() + chrono::Duration::minutes(9));
}

#[tokio::test]
async fn permanent_errors_and_exhausted_attempts_report_job_failed() {
    let h = H::with(db!(), |c| c.backoff_base = Duration::from_secs(60)).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let perm = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        10,
        "k:perm",
        comment_input(1, "x"),
    )
    .await;
    h.fake
        .fail_next(enzarb_connector::Error::PermissionDenied("nope".into()));
    h.exec.drain().await;
    let j = job(pool, perm).await;
    assert_eq!((j.status.as_str(), j.attempt), ("failed", 1));

    let wi2 = new_item(pool, 2, "enzarb").await;
    let tired = new_job(
        pool,
        wi2,
        "enzarb",
        "post_comment",
        10,
        "k:tired",
        comment_input(2, "x"),
    )
    .await;
    sqlx::query("UPDATE workflows.jobs SET max_attempts = 2 WHERE id = $1")
        .bind(tired)
        .execute(pool)
        .await
        .unwrap();
    for _ in 0..2 {
        h.fake
            .fail_next(enzarb_connector::Error::Transient("down".into()));
        sqlx::query("UPDATE workflows.jobs SET run_after = now()")
            .execute(pool)
            .await
            .unwrap();
        h.exec.drain().await;
    }
    let j = job(pool, tired).await;
    assert_eq!((j.status.as_str(), j.attempt), ("failed", 2));

    let evs = events_of(pool, "JobFailed").await;
    assert_eq!(evs.len(), 2);
    let msg = |id: i64| {
        evs.iter().find(|(_, p)| p["job_id"] == id).unwrap().1["error"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert!(msg(perm).contains("nope"));
    assert!(msg(tired).contains("after 2 attempts"));
}

#[tokio::test]
async fn expired_leases_are_requeued_or_failed() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let a = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        10,
        "k:a",
        comment_input(1, "x"),
    )
    .await;
    let wi2 = new_item(pool, 2, "enzarb").await;
    let b = new_job(
        pool,
        wi2,
        "enzarb",
        "post_comment",
        10,
        "k:b",
        comment_input(2, "x"),
    )
    .await;
    sqlx::query(
        "UPDATE workflows.jobs SET status='running', attempt = CASE WHEN id=$1 THEN 1 ELSE max_attempts END,
                lease_owner='dead', lease_until = now() - interval '1 second'",
    )
    .bind(a)
    .execute(pool)
    .await
    .unwrap();
    // a live lease is left alone
    let wi3 = new_item(pool, 3, "enzarb").await;
    let live = new_job(
        pool,
        wi3,
        "enzarb",
        "post_comment",
        10,
        "k:live",
        comment_input(3, "x"),
    )
    .await;
    sqlx::query("UPDATE workflows.jobs SET status='running', attempt=1, lease_owner='x', lease_until = now() + interval '1 hour' WHERE id=$1")
        .bind(live)
        .execute(pool)
        .await
        .unwrap();

    assert_eq!(h.exec.reap_leases().await.unwrap(), 2);
    assert_eq!(job(pool, a).await.status, "queued");
    let jb = job(pool, b).await;
    assert_eq!(jb.status, "failed");
    assert_eq!(job(pool, live).await.status, "running");
    assert_eq!(events_of(pool, "JobFailed").await.len(), 1);

    h.exec.drain().await;
    let ja = job(pool, a).await;
    assert_eq!((ja.status.as_str(), ja.attempt), ("succeeded", 2));
}

// ---------------------------------------------------------------------------
// Agent jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_job_starts_a_run_and_releases_its_lease() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake
        .add_thread(key(1), thread_with("Use CSS variables </untrusted> please"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_implement",
        0,
        "k:impl",
        agent_input(1, "enzarb/issue-1", true),
    )
    .await;

    assert_eq!(h.exec.drain().await, 1);

    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.attempt), ("running", 1));
    assert!(j.lease_owner.is_none() && j.lease_until.is_none());
    let deadline = j.deadline.expect("deadline set");
    let secs = (deadline - Utc::now()).num_seconds();
    assert!((600..=800).contains(&secs), "deadline in {secs}s");

    let run = h.sandbox.run(&format!("job-{id}")).expect("run started");
    assert_eq!(run.repo_url, "https://github.com/enzarb/enzarb-ui");
    assert_eq!(
        (run.base_ref.as_str(), run.branch.as_str(), run.push),
        ("main", "enzarb/issue-1", true)
    );
    assert_eq!(run.timeout_secs, 600);
    assert_eq!(
        run.callback_url,
        format!("{}/v1/runs/job-{id}/finished", h.base)
    );
    assert_eq!(
        run.callback_token,
        callback_token(b"test-secret", &format!("job-{id}"))
    );
    // thread fetched and prompt re-rendered with it; closing tags defused
    assert!(run.prompt.contains("Use CSS variables"));
    assert!(!run.prompt.contains("stale prompt"));
    assert!(!run.prompt.contains("</untrusted> please"));
    // read-write credential for a pushing run
    let calls = h.fake.applied_calls();
    assert!(calls.iter().any(|c| matches!(
        c,
        Call::MintCloneCredential {
            scope: CredScope::ReadWrite,
            ..
        }
    )));
    assert!(calls.iter().any(|c| matches!(c, Call::FetchThread { .. })));
    assert!(run.clone_token.len() > 3);
}

fn proj_repos() -> Vec<RepoKey> {
    vec![
        "github:enzarb/enzarb-proto".parse().unwrap(),
        "github:enzarb/enzarb-connectors".parse().unwrap(),
    ]
}

async fn kind_job(h: &H, kind: &str, n: u64, push: bool) -> i64 {
    let pool = h.pool();
    h.fake.add_thread(key(n), thread_with("hi"));
    let wi = new_item(pool, n, "enzarb").await;
    new_job(
        pool,
        wi,
        "enzarb",
        kind,
        0,
        &format!("k:{kind}{n}"),
        agent_input(n, &format!("enzarb/b-{n}"), push),
    )
    .await
}

#[tokio::test]
async fn deps_token_for_implement_address_review_only() {
    let h = H::new(db!()).await;
    h.resolver.set_repos("enzarb", proj_repos());
    let kinds = [
        ("agent_implement", true, true),
        ("agent_address_comments", true, true),
        ("agent_review", false, true),
        ("agent_triage", false, false),
        ("agent_plan", false, false),
    ];
    let mut ids = vec![];
    for (i, (k, push, _)) in kinds.iter().enumerate() {
        ids.push(kind_job(&h, k, i as u64 + 1, *push).await);
    }
    // session cap is per project; drain until all are started
    for _ in 0..kinds.len() {
        h.exec.drain().await;
        sqlx::query("UPDATE workflows.jobs SET status='succeeded' WHERE status='running'")
            .execute(h.pool())
            .await
            .unwrap();
    }
    for (id, (k, _, want)) in ids.iter().zip(kinds) {
        let run = h.sandbox.run(&format!("job-{id}")).expect(k);
        assert_eq!(run.deps_token.is_some(), want, "{k}");
        // never in the stored job input
        let j = job(h.pool(), *id).await;
        let input = j.input.to_string();
        if let Some(t) = &run.deps_token {
            assert!(!input.contains(t));
            assert!(!format!("{run:?}").contains(t));
        }
        assert!(!format!("{run:?}").contains(&run.clone_token));
    }
    let reads: Vec<_> = h
        .fake
        .applied_calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::MintReadCredential { repos, .. } => Some(repos),
            _ => None,
        })
        .collect();
    assert_eq!(reads.len(), 3);
    for r in reads {
        let names: Vec<_> = r.iter().map(|k| k.full_name()).collect();
        // the Project's repos plus the run's own, sorted
        assert_eq!(
            names,
            [
                "enzarb/enzarb-connectors",
                "enzarb/enzarb-proto",
                "enzarb/enzarb-ui"
            ]
        );
    }
    // the clone path is unchanged: still a ReadWrite credential for the pushing runs
    let rw = h
        .fake
        .applied_calls()
        .iter()
        .filter(|c| {
            matches!(
                c,
                Call::MintCloneCredential {
                    scope: CredScope::ReadWrite,
                    ..
                }
            )
        })
        .count();
    assert_eq!(rw, 2);
}

/// Delegates to a `FakeConnector` but fails `mint_read_credential`.
struct ReadFails {
    inner: Arc<FakeConnector>,
    err: enzarb_connector::Error,
}

#[async_trait::async_trait]
impl enzarb_connector::Connector for ReadFails {
    async fn fetch_thread(&self, i: &WorkItemKey) -> enzarb_connector::Result<Thread> {
        self.inner.fetch_thread(i).await
    }
    async fn post_comment(
        &self,
        i: &WorkItemKey,
        b: &str,
        k: &str,
    ) -> enzarb_connector::Result<enzarb_connector::CommentRef> {
        self.inner.post_comment(i, b, k).await
    }
    async fn open_change_request(
        &self,
        r: ChangeRequestSpec,
        k: &str,
    ) -> enzarb_connector::Result<ItemRef> {
        self.inner.open_change_request(r, k).await
    }
    async fn set_status(
        &self,
        t: &enzarb_connector::CommitRef,
        s: enzarb_connector::Status,
        k: &str,
    ) -> enzarb_connector::Result<()> {
        self.inner.set_status(t, s, k).await
    }
    async fn set_progress(
        &self,
        t: &ProgressTarget,
        p: Progress,
        k: &str,
    ) -> enzarb_connector::Result<()> {
        self.inner.set_progress(t, p, k).await
    }
    async fn merge_change_request(
        &self,
        i: &WorkItemKey,
        n: u64,
        s: &str,
        t: &str,
        b: Option<&str>,
        k: &str,
    ) -> enzarb_connector::Result<enzarb_connector::MergeOutcome> {
        self.inner.merge_change_request(i, n, s, t, b, k).await
    }
    async fn get_merge_readiness(
        &self,
        i: &WorkItemKey,
        n: u64,
    ) -> enzarb_connector::Result<enzarb_connector::MergeReadiness> {
        self.inner.get_merge_readiness(i, n).await
    }
    async fn check_permission(
        &self,
        r: &RepoKey,
        a: &Actor,
    ) -> enzarb_connector::Result<Permission> {
        self.inner.check_permission(r, a).await
    }
    async fn mint_clone_credential(
        &self,
        r: &RepoKey,
        s: CredScope,
        k: &str,
    ) -> enzarb_connector::Result<enzarb_connector::ShortLivedCred> {
        self.inner.mint_clone_credential(r, s, k).await
    }
    async fn mint_read_credential(
        &self,
        _: &[RepoKey],
        _: &str,
    ) -> enzarb_connector::Result<Option<enzarb_connector::ReadCred>> {
        Err(match &self.err {
            enzarb_connector::Error::Invalid(m) => enzarb_connector::Error::Invalid(m.clone()),
            _ => enzarb_connector::Error::Transient("502".into()),
        })
    }
}

async fn read_fails_harness(db: TestDb, err: enzarb_connector::Error) -> (H, Arc<Executor>) {
    let h = H::new(db).await;
    let conn = Arc::new(ReadFails {
        inner: h.fake.clone(),
        err,
    });
    let exec = Arc::new(Executor::new(
        h.db.pool.clone(),
        ConnectorRegistry::new().with("github", conn),
        h.resolver.clone(),
        h.exec.config().clone(),
    ));
    (h, exec)
}

#[tokio::test]
async fn read_credential_transient_error_retries_the_job() {
    let (h, exec) =
        read_fails_harness(db!(), enzarb_connector::Error::Transient("502".into())).await;
    let id = kind_job(&h, "agent_implement", 1, true).await;
    exec.drain().await;
    let j = job(h.pool(), id).await;
    // retried (queued with a backoff, or failed after exhausting attempts), never started
    assert!(j.attempt >= 1);
    assert_ne!(j.status, "running");
    assert_eq!(h.sandbox.starts(), 0);
}

#[tokio::test]
async fn read_credential_invalid_runs_without_deps_token() {
    let (h, exec) = read_fails_harness(
        db!(),
        enzarb_connector::Error::Invalid("foreign repo".into()),
    )
    .await;
    let id = kind_job(&h, "agent_implement", 1, true).await;
    assert_eq!(exec.drain().await, 1);
    assert_eq!(job(h.pool(), id).await.status, "running");
    let run = h.sandbox.run(&format!("job-{id}")).expect("started");
    assert!(run.deps_token.is_none());
    assert!(run.clone_token.len() > 3);
}

#[tokio::test]
async fn agent_runs_are_capped_per_project() {
    let db = db!();
    let h = H::new(db).await;
    let pool = h.pool();
    h.resolver.insert(
        "other",
        SandboxTarget {
            endpoint: h.sandbox.url.clone(),
            token: SANDBOX_TOKEN.into(),
        },
    );
    h.fake.add_thread(key(1), thread_with("a"));
    h.fake.add_thread(key(2), thread_with("b"));
    let w1 = new_item(pool, 1, "enzarb").await;
    let w2 = new_item(pool, 2, "enzarb").await;
    let a1 = new_job(
        pool,
        w1,
        "enzarb",
        "agent_triage",
        0,
        "k:a1",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    let a2 = new_job(
        pool,
        w2,
        "enzarb",
        "agent_triage",
        0,
        "k:a2",
        agent_input(2, "enzarb/triage-2", false),
    )
    .await;
    // another project's job is not held back by the first project's cap
    let w3 = new_item(pool, 3, "other").await;
    h.fake.add_thread(key(3), thread_with("c"));
    let b1 = new_job(
        pool,
        w3,
        "other",
        "agent_triage",
        0,
        "k:b1",
        agent_input(3, "enzarb/triage-3", false),
    )
    .await;

    assert_eq!(h.exec.drain().await, 2);
    assert_eq!(job(pool, a1).await.status, "running");
    assert_eq!(job(pool, b1).await.status, "running");
    assert_eq!(job(pool, a2).await.status, "queued");
    assert_eq!(h.sandbox.starts(), 2);

    // the slot frees when the run ends
    sqlx::query("UPDATE workflows.jobs SET status='succeeded' WHERE id=$1")
        .bind(a1)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.drain().await, 1);
    assert_eq!(job(pool, a2).await.status, "running");
}

#[tokio::test]
async fn working_marker_waits_for_the_accepted_run() {
    let h = H::with(db!(), |c| c.backoff_base = Duration::from_secs(60)).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let received = new_job(
        pool,
        wi,
        "enzarb",
        "set_progress",
        100,
        "w:1:ev:set_progress",
        progress_input(1, Progress::Received),
    )
    .await;
    let working = new_job(
        pool,
        wi,
        "enzarb",
        "set_progress",
        90,
        "w:1:ev:set_progress#2",
        progress_input(1, Progress::Working),
    )
    .await;
    let agent = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "w:1:ev:agent_triage",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;

    // the sandbox is unknown: the agent job backs off, Working stays queued
    h.resolver.remove("enzarb");
    h.resolver.insert(
        "enzarb",
        SandboxTarget {
            endpoint: "ws://127.0.0.1:1/v1/mux".into(),
            token: "x".into(),
        },
    );
    assert_eq!(h.exec.drain().await, 2); // Received + the failed agent attempt
    assert_eq!(job(pool, received).await.status, "succeeded");
    assert_eq!(job(pool, working).await.status, "queued");
    assert_eq!(job(pool, agent).await.status, "queued");
    assert_eq!(job(pool, agent).await.attempt, 1);

    // sandbox reachable: the run is accepted, then Working is applied
    h.resolver.insert(
        "enzarb",
        SandboxTarget {
            endpoint: h.sandbox.url.clone(),
            token: SANDBOX_TOKEN.into(),
        },
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    sqlx::query("UPDATE workflows.jobs SET run_after = now() WHERE id = $1")
        .bind(agent)
        .execute(pool)
        .await
        .unwrap();
    h.exec.drain().await;
    assert_eq!(job(pool, agent).await.status, "running");
    assert_eq!(job(pool, working).await.status, "succeeded");
    let progress: Vec<Progress> = h
        .fake
        .applied_calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::SetProgress { progress, .. } => Some(progress),
            _ => None,
        })
        .collect();
    assert_eq!(progress, [Progress::Received, Progress::Working]);
}

#[tokio::test]
async fn working_marker_is_dropped_when_the_run_already_ended() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let working = new_job(
        pool,
        wi,
        "enzarb",
        "set_progress",
        90,
        "w:1:ev:set_progress",
        progress_input(1, Progress::Working),
    )
    .await;
    let agent = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "w:1:ev:agent_triage",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    sqlx::query("UPDATE workflows.jobs SET status='succeeded', attempt=1 WHERE id=$1")
        .bind(agent)
        .execute(pool)
        .await
        .unwrap();
    h.exec.drain().await;
    assert_eq!(job(pool, working).await.status, "succeeded");
    assert!(h.fake.applied_calls().is_empty());
}

#[tokio::test]
async fn unreachable_or_unauthorized_sandbox_is_retried() {
    let h = H::with(db!(), |c| c.backoff_base = Duration::from_secs(60)).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.resolver.insert(
        "enzarb",
        SandboxTarget {
            endpoint: h.sandbox.url.clone(),
            token: "wrong".into(),
        },
    );
    h.exec.drain().await;
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.attempt), ("queued", 1));
    assert_eq!(h.sandbox.starts(), 0);
    assert!(j.deadline.is_none());

    // unknown project: also retried (the sandbox may not exist yet)
    h.resolver.remove("enzarb");
    sqlx::query("UPDATE workflows.jobs SET run_after = now()")
        .execute(pool)
        .await
        .unwrap();
    h.exec.drain().await;
    assert_eq!(job(pool, id).await.attempt, 2);
    assert_eq!(job(pool, id).await.status, "queued");
}

// ---------------------------------------------------------------------------
// Callback
// ---------------------------------------------------------------------------

async fn post_callback(url: &str, token: &str, body: &RunFinished) -> u16 {
    reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

fn finished(run_id: &str, outcome: RunOutcome) -> RunFinished {
    RunFinished {
        run_id: run_id.into(),
        outcome,
        exit_code: Some(1),
        summary: "boom".into(),
        branch: "enzarb/triage-1".into(),
        head_sha: "abc".into(),
        pushed: false,
    }
}

#[tokio::test]
async fn callback_authenticates_and_is_idempotent() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    let rid = format!("job-{id}");
    let url = format!("{}/v1/runs/{rid}/finished", h.base);
    let good = callback_token(b"test-secret", &rid);

    assert_eq!(
        post_callback(&url, "nope", &finished(&rid, RunOutcome::Failed)).await,
        401
    );
    assert_eq!(
        post_callback(
            &url,
            &callback_token(b"other", &rid),
            &finished(&rid, RunOutcome::Failed)
        )
        .await,
        401
    );
    assert_eq!(
        post_callback(&url, &good, &finished("job-999", RunOutcome::Failed)).await,
        400
    );
    let missing = "job-424242";
    let murl = format!("{}/v1/runs/{missing}/finished", h.base);
    assert_eq!(
        post_callback(
            &murl,
            &callback_token(b"test-secret", missing),
            &finished(missing, RunOutcome::Failed)
        )
        .await,
        404
    );
    assert_eq!(job(pool, id).await.status, "running");

    assert_eq!(
        post_callback(&url, &good, &finished(&rid, RunOutcome::Failed)).await,
        200
    );
    let j = job(pool, id).await;
    assert_eq!(j.status, "failed");
    let evs = events_of(pool, "JobFailed").await;
    assert_eq!(evs.len(), 1);
    assert!(evs[0].1["error"].as_str().unwrap().contains("boom"));
    // sandbox-init retrying is harmless
    assert_eq!(
        post_callback(&url, &good, &finished(&rid, RunOutcome::Failed)).await,
        200
    );
    assert_eq!(events_of(pool, "JobFailed").await.len(), 1);

    let health = reqwest::get(format!("{}/healthz", h.base)).await.unwrap();
    assert_eq!(health.status(), 200);
}

// ---------------------------------------------------------------------------
// Fallbacks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deadline_poll_cancels_a_stuck_run_and_reports_failure() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    // not due yet
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 0);
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 1);
    assert_eq!(job(pool, id).await.status, "failed");
    wait_for("run_cancel", || async { !h.sandbox.cancels().is_empty() }).await;
    assert_eq!(h.sandbox.cancels(), [format!("job-{id}")]);
    let evs = events_of(pool, "JobFailed").await;
    assert!(evs[0].1["error"].as_str().unwrap().contains("deadline"));
}

#[tokio::test]
async fn deadline_poll_reports_a_run_that_finished_without_callback() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    h.sandbox
        .finish_with_state(&format!("job-{id}"), RunPhase::Succeeded, "all good");
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 1);
    let j = job(pool, id).await;
    assert_eq!(j.status, "succeeded");
    let out = j.output.unwrap();
    assert_eq!(out["summary"], "all good");
    assert_eq!(out["pushed"], false);
    assert_eq!(out["branch"], "enzarb/triage-1");
}

#[tokio::test]
async fn deadline_poll_gives_up_on_an_unreachable_sandbox() {
    let h = H::with(db!(), |c| c.poll_give_up = Duration::from_secs(60)).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    h.resolver.remove("enzarb");
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 0);
    assert_eq!(job(pool, id).await.status, "running");
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '2 minutes' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 1);
    assert_eq!(job(pool, id).await.status, "failed");
}

#[tokio::test]
async fn cancelled_jobs_get_their_run_cancelled_once() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    // what the reducer's CancelJobs does
    sqlx::query("UPDATE workflows.jobs SET status='cancelled', lease_owner=NULL, lease_until=NULL WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.cancel_runs().await.unwrap(), 1);
    wait_for("run_cancel", || async { !h.sandbox.cancels().is_empty() }).await;
    assert_eq!(h.sandbox.cancels(), [format!("job-{id}")]);
    assert!(job(pool, id).await.deadline.is_none());
    assert_eq!(h.exec.cancel_runs().await.unwrap(), 0);
}

// ---------------------------------------------------------------------------
// End to end with the reducer
// ---------------------------------------------------------------------------

async fn open_issue(pool: &PgPool, n: u64) {
    let repo: RepoKey = REPO.parse().unwrap();
    let data = EventData {
        repo: repo.clone(),
        actor: Actor {
            login: "alice".into(),
            id: 7,
            is_bot: false,
        },
        item: ItemRef {
            kind: ItemKind::Issue,
            number: n,
            url: format!("https://github.com/enzarb/enzarb-ui/issues/{n}"),
            title: "Add dark mode".into(),
            body: "Please add a dark theme.".into(),
            author: "alice".into(),
            head_ref: None,
            base_ref: None,
            head_sha: None,
        },
        comment: None,
        merged: None,
        ci: None,
        review: None,
    };
    let ev = Event::new("github", "d1", EventType::WorkItemOpened, Utc::now(), data);
    let _: Option<i64> =
        sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
            .bind("github")
            .bind(ev.dedupe_key())
            .bind(ev.subject.to_string())
            .bind("WorkItemOpened")
            .bind(serde_json::to_value(&ev).unwrap())
            .fetch_one(pool)
            .await
            .unwrap();
}

#[tokio::test]
async fn issue_triage_runs_end_to_end_through_the_sandbox() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let index = StaticProjectIndex::shared();
    index.insert(
        REPO,
        engine::ProjectInfo {
            name: "enzarb".into(),
            triggers: engine::machine::Triggers::default(),
            default_branch: "main".into(),
        },
    );
    let perms = Arc::new(StaticPermissions::new(Permission::Write));
    let reducer = Reducer::new(pool.clone(), index, perms, ReducerConfig::default());
    h.fake
        .add_thread(key(1), thread_with("Please keep the contrast high"));

    let block = json!({ "decision": "ready", "body": "Clear enough to plan." });
    h.sandbox.set_mode(Mode::Finish {
        outcome: RunOutcome::Succeeded,
        summary: format!("Looked around.\n\n```json\n{block}\n```\n"),
        pushed: false,
        callback: true,
    });

    open_issue(pool, 1).await;
    assert_eq!(reducer.drain().await, 1);
    // markers + triage start; the callback arrives from the fake sandbox
    h.exec.drain().await;
    let triage_id: i64 =
        sqlx::query_scalar("SELECT id FROM workflows.jobs WHERE kind='agent_triage'")
            .fetch_one(pool)
            .await
            .unwrap();
    wait_for("triage job to finish", || async {
        job(pool, triage_id).await.status == "succeeded"
    })
    .await;
    wait_for("callback 200", || async {
        h.sandbox.sb.callbacks.lock().unwrap().as_slice() == [200]
    })
    .await;

    let run = h.sandbox.run(&format!("job-{triage_id}")).unwrap();
    assert!(!run.push);
    assert!(run.prompt.contains("Please keep the contrast high"));
    assert!(run.prompt.contains("Add dark mode"));
    // output keeps the RunFinished fields the machine reads
    let out = job(pool, triage_id).await.output.unwrap();
    assert_eq!(out["head_sha"], "abc123");
    assert_eq!(out["pushed"], false);
    assert!(out["summary"].as_str().unwrap().contains("Clear enough"));

    // the engine consumes JobSucceeded and moves on to planning
    reducer.drain().await;
    let state: String = sqlx::query_scalar("SELECT state FROM workflows.work_items WHERE key=$1")
        .bind(key(1).to_string())
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(state, "Planning");
    // the plan job is picked up too, and the progress markers reached the connector
    h.exec.drain().await;
    let plan: Job = sqlx::query_as("SELECT * FROM workflows.jobs WHERE kind='agent_plan'")
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(plan.status == "running" || plan.status == "succeeded");
    let progress: Vec<Progress> = h
        .fake
        .applied_calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::SetProgress { progress, .. } => Some(progress),
            _ => None,
        })
        .collect();
    assert_eq!(progress.first(), Some(&Progress::Received));
    assert!(progress.contains(&Progress::Working));
}

#[tokio::test]
async fn run_loop_processes_jobs_and_stops_on_shutdown() {
    let h = H::with(db!(), |c| {
        c.poll_interval = Duration::from_millis(50);
        c.sweep_interval = Duration::from_millis(50);
    })
    .await;
    let pool = h.pool();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(h.exec.clone().run(rx));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "post_comment",
        10,
        "k:c",
        comment_input(1, "x"),
    )
    .await;
    wait_for("job to finish", || async {
        job(pool, id).await.status == "succeeded"
    })
    .await;
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("run returns after shutdown")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn set_progress_passes_comment_kind_to_the_connector() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let wi = new_item(pool, 1, "enzarb").await;
    let review_target = ProgressTarget {
        item: key(1),
        comment_id: Some(7),
        comment_kind: CommentKind::Review,
    };
    let new = new_job(
        pool,
        wi,
        "enzarb",
        "set_progress",
        100,
        "k:review",
        json!({ "target": review_target, "progress": Progress::Received }),
    )
    .await;
    // Rows written before `comment_kind` existed still run, as conversation comments.
    let old = new_job(
        pool,
        wi,
        "enzarb",
        "set_progress",
        90,
        "k:old",
        json!({ "target": { "item": key(1), "comment_id": 8 }, "progress": "Received" }),
    )
    .await;
    assert_eq!(h.exec.drain().await, 2);
    assert_eq!(job(pool, new).await.status, "succeeded");
    assert_eq!(job(pool, old).await.status, "succeeded");
    let calls = h.fake.applied_calls();
    assert!(calls.contains(&Call::SetProgress {
        target: review_target,
        progress: Progress::Received,
        idempotency_key: "k:review".into(),
    }));
    assert!(calls.contains(&Call::SetProgress {
        target: ProgressTarget {
            item: key(1),
            comment_id: Some(8),
            comment_kind: CommentKind::Conversation,
        },
        progress: Progress::Received,
        idempotency_key: "k:old".into(),
    }));
}

// ---------------------------------------------------------------------------
// Session limits
// ---------------------------------------------------------------------------

/// The failure text of enzarb-ui#48, with the reset time `reset`.
fn limit_summary(reset: &str) -> String {
    format!(
        "session/prompt: Internal error: You've hit your session limit · resets {reset} (UTC): {{\n  \"errorKind\": \"rate_limit\"\n}}"
    )
}

/// A UTC clock time `hours` from now, as `HH:MM`.
fn reset_in(hours: i64) -> String {
    (Utc::now() + chrono::Duration::hours(hours))
        .format("%H:%M")
        .to_string()
}

/// POST a `Failed` callback for the run `rid` of `id`.
async fn fail_run(h: &H, rid: &str, summary: &str) -> u16 {
    let url = format!("{}/v1/runs/{rid}/finished", h.base);
    let mut body = finished(rid, RunOutcome::Failed);
    body.summary = summary.into();
    body.exit_code = None;
    post_callback(&url, &callback_token(b"test-secret", rid), &body).await
}

async fn hold_until(pool: &PgPool, project: &str) -> Option<chrono::DateTime<Utc>> {
    sqlx::query_scalar("SELECT blocked_until FROM workflows.project_holds WHERE project_id = $1")
        .bind(project)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn session_limit_requeues_without_spending_the_budget_and_holds_the_project() {
    let h = H::with(db!(), |c| c.max_agent_per_project = 2).await;
    let pool = h.pool();
    h.resolver.insert(
        "other",
        SandboxTarget {
            endpoint: h.sandbox.url.clone(),
            token: SANDBOX_TOKEN.into(),
        },
    );
    for n in 1..=4 {
        h.fake.add_thread(key(n), thread_with("a"));
    }
    let w1 = new_item(pool, 1, "enzarb").await;
    let w2 = new_item(pool, 2, "enzarb").await;
    let a1 = new_job(
        pool,
        w1,
        "enzarb",
        "agent_triage",
        0,
        "k:a1",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    let a2 = new_job(
        pool,
        w2,
        "enzarb",
        "agent_triage",
        0,
        "k:a2",
        agent_input(2, "enzarb/triage-2", false),
    )
    .await;
    assert_eq!(h.exec.drain().await, 2);

    // a1 hits the limit, which resets in about two hours.
    let before = Utc::now();
    assert_eq!(
        fail_run(&h, &format!("job-{a1}"), &limit_summary(&reset_in(2))).await,
        200
    );
    let j = job(pool, a1).await;
    assert_eq!(
        (j.status.as_str(), j.attempt, j.max_attempts),
        ("queued", 1, 6)
    );
    assert_eq!(j.waiting_for.as_deref(), Some("session_limit"));
    assert_eq!(j.resumes_at, Some(j.run_after));
    assert_eq!(j.limit_wait_count, 1);
    assert!(j.limit_wait_started_at.is_some());
    assert!(j.lease_owner.is_none() && j.deadline.is_none());
    assert!(j.run_after > before + chrono::Duration::minutes(60));
    assert!(j.run_after < before + chrono::Duration::minutes(130));
    assert_eq!(hold_until(pool, "enzarb").await, Some(j.run_after));
    // The work item is waiting, not failed.
    assert!(events_of(pool, "JobFailed").await.is_empty());
    let (state, waiting): (String, Option<String>) =
        sqlx::query_as("SELECT state, waiting_for FROM workflows_api.work_item_v WHERE id = $1")
            .bind(w1)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), waiting.as_deref()),
        ("Triaging", Some("session_limit"))
    );

    // a2's run hits the same limit but reports no reset time: it backs off 5 minutes, and the
    // hold does not move backwards.
    assert_eq!(
        fail_run(&h, &format!("job-{a2}"), "You've hit your session limit").await,
        200
    );
    let j2 = job(pool, a2).await;
    assert_eq!(j2.status, "queued");
    assert!(j2.run_after > before + chrono::Duration::minutes(4));
    assert!(j2.run_after < before + chrono::Duration::minutes(7));
    assert_eq!(hold_until(pool, "enzarb").await, Some(j.run_after));

    // While held, agent jobs of the Project wait; other Projects and non-agent jobs do not.
    sqlx::query("UPDATE workflows.jobs SET run_after = now() WHERE id = $1")
        .bind(a2)
        .execute(pool)
        .await
        .unwrap();
    let w3 = new_item(pool, 3, "other").await;
    let b1 = new_job(
        pool,
        w3,
        "other",
        "agent_triage",
        0,
        "k:b1",
        agent_input(3, "enzarb/triage-3", false),
    )
    .await;
    let w4 = new_item(pool, 4, "enzarb").await;
    let p = new_job(
        pool,
        w4,
        "enzarb",
        "set_progress",
        0,
        "k:p",
        progress_input(4, Progress::Received),
    )
    .await;
    assert_eq!(h.exec.drain().await, 2);
    assert_eq!(job(pool, b1).await.status, "running");
    assert_eq!(job(pool, p).await.status, "succeeded");
    assert_eq!(job(pool, a1).await.status, "queued");
    assert_eq!(job(pool, a2).await.status, "queued");

    // After the reset the job runs again as a new attempt and a new sandbox run.
    sqlx::query("UPDATE workflows.jobs SET run_after = now() - interval '1 second' WHERE id = $1")
        .bind(a1)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE workflows.project_holds SET blocked_until = now() - interval '1 second'")
        .execute(pool)
        .await
        .unwrap();
    let starts = h.sandbox.starts();
    assert!(h.exec.drain().await >= 1);
    let j = job(pool, a1).await;
    assert_eq!(
        (j.status.as_str(), j.attempt, j.max_attempts),
        ("running", 2, 6)
    );
    assert!(j.waiting_for.is_none() && j.resumes_at.is_none());
    assert!(h.sandbox.starts() > starts);
    let rid = format!("job-{a1}-1");
    assert!(h.sandbox.run(&rid).is_some());

    // The first run's callback arriving late is ignored; the new run's finishes the job.
    assert_eq!(fail_run(&h, &format!("job-{a1}"), "boom").await, 200);
    assert_eq!(job(pool, a1).await.status, "running");
    let url = format!("{}/v1/runs/{rid}/finished", h.base);
    let ok = finished(&rid, RunOutcome::Succeeded);
    assert_eq!(
        post_callback(&url, &callback_token(b"test-secret", &rid), &ok).await,
        200
    );
    assert_eq!(job(pool, a1).await.status, "succeeded");
}

#[tokio::test]
async fn session_limit_gives_up_after_seven_days() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    sqlx::query(
        "UPDATE workflows.jobs SET limit_wait_started_at = now() - interval '8 days' WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    assert_eq!(
        fail_run(&h, &format!("job-{id}"), &limit_summary(&reset_in(1))).await,
        200
    );
    let j = job(pool, id).await;
    assert_eq!(j.status, "failed");
    assert!(j.waiting_for.is_none());
    let evs = events_of(pool, "JobFailed").await;
    assert_eq!(evs.len(), 1);
    let err = evs[0].1["error"].as_str().unwrap();
    assert!(
        err.contains("session limit never reset after 7 days of waiting"),
        "{err}"
    );
    assert!(err.contains("hit your session limit"), "{err}");
    assert!(hold_until(pool, "enzarb").await.is_none());
}

#[tokio::test]
async fn a_plain_agent_failure_still_fails_the_job() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    assert_eq!(
        fail_run(&h, &format!("job-{id}"), "compile error").await,
        200
    );
    let j = job(pool, id).await;
    assert_eq!(
        (j.status.as_str(), j.attempt, j.max_attempts),
        ("failed", 1, 5)
    );
    assert!(j.waiting_for.is_none() && j.limit_wait_count == 0);
    assert_eq!(events_of(pool, "JobFailed").await.len(), 1);
    assert!(hold_until(pool, "enzarb").await.is_none());
}

#[tokio::test]
async fn deadline_poll_waits_out_a_session_limit() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    h.sandbox.finish_with_state(
        &format!("job-{id}"),
        RunPhase::Failed,
        &limit_summary(&reset_in(2)),
    );
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 1);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.max_attempts), ("queued", 6));
    assert_eq!(j.waiting_for.as_deref(), Some("session_limit"));
    assert!(j.run_after > Utc::now() + chrono::Duration::minutes(60));
    assert_eq!(hold_until(pool, "enzarb").await, Some(j.run_after));
    assert!(events_of(pool, "JobFailed").await.is_empty());
}

// ---------------------------------------------------------------------------
// Sandbox restarts
// ---------------------------------------------------------------------------

/// POST a `Cancelled` callback for the run `rid`.
async fn cancel_run(h: &H, rid: &str) -> u16 {
    let url = format!("{}/v1/runs/{rid}/finished", h.base);
    let body = finished(rid, RunOutcome::Cancelled);
    post_callback(&url, &callback_token(b"test-secret", rid), &body).await
}

/// A claimed (running) agent job on a fresh item.
async fn running_agent_job(h: &H) -> i64 {
    let pool = h.pool();
    h.fake.add_thread(key(1), thread_with("a"));
    let wi = new_item(pool, 1, "enzarb").await;
    let id = new_job(
        pool,
        wi,
        "enzarb",
        "agent_triage",
        0,
        "k:a",
        agent_input(1, "enzarb/triage-1", false),
    )
    .await;
    h.exec.drain().await;
    assert_eq!(job(pool, id).await.status, "running");
    id
}

async fn make_due(pool: &PgPool, id: i64) {
    sqlx::query("UPDATE workflows.jobs SET run_after = now() - interval '1 second' WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_restart_cancel_requeues_the_job_with_a_new_run() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let id = running_agent_job(&h).await;
    let before = Utc::now();
    assert_eq!(cancel_run(&h, &format!("job-{id}")).await, 200);
    let j = job(pool, id).await;
    assert_eq!(
        (
            j.status.as_str(),
            j.attempt,
            j.max_attempts,
            j.restart_retries
        ),
        ("queued", 1, 6, 1)
    );
    assert!(j.run_after > before);
    assert!(j.lease_owner.is_none() && j.deadline.is_none());
    assert_eq!(j.limit_wait_count, 0);
    assert!(events_of(pool, "JobFailed").await.is_empty());

    // A late duplicate of the same callback is ignored.
    assert_eq!(cancel_run(&h, &format!("job-{id}")).await, 200);
    assert_eq!(job(pool, id).await.restart_retries, 1);

    // The retry is a second attempt under a new run id.
    make_due(pool, id).await;
    assert!(h.exec.drain().await >= 1);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.attempt), ("running", 2));
    assert!(h.sandbox.run(&format!("job-{id}-1")).is_some());
}

async fn cancel_accepted(h: &H, rid: &str) -> bool {
    let url = format!("{}/v1/runs/{rid}/finished", h.base);
    let resp = reqwest::Client::new()
        .post(url)
        .bearer_auth(callback_token(b"test-secret", rid))
        .json(&finished(rid, RunOutcome::Cancelled))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    body["accepted"].as_bool().unwrap()
}

#[tokio::test]
async fn a_requeue_is_accepted_and_a_stale_callback_is_not() {
    let h = H::new(db!()).await;
    let id = running_agent_job(&h).await;
    let rid = format!("job-{id}");
    assert!(cancel_accepted(&h, &rid).await);
    // The same run id again: the job is queued now, so the call is stale.
    assert!(!cancel_accepted(&h, &rid).await);
    assert_eq!(job(h.pool(), id).await.restart_retries, 1);
}

#[tokio::test]
async fn restart_retries_are_capped() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let id = running_agent_job(&h).await;
    for n in 0..3 {
        let rid = if n == 0 {
            format!("job-{id}")
        } else {
            format!("job-{id}-{n}")
        };
        assert_eq!(cancel_run(&h, &rid).await, 200);
        let j = job(pool, id).await;
        assert_eq!((j.status.as_str(), j.restart_retries), ("queued", n + 1));
        make_due(pool, id).await;
        assert!(h.exec.drain().await >= 1);
    }
    assert_eq!(cancel_run(&h, &format!("job-{id}-3")).await, 200);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.restart_retries), ("failed", 3));
    let evs = events_of(pool, "JobFailed").await;
    assert_eq!(evs.len(), 1);
    let err = evs[0].1["error"].as_str().unwrap();
    assert!(err.contains("gave up after 3 retries"), "{err}");
}

#[tokio::test]
async fn a_user_cancelled_job_is_not_requeued() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let id = running_agent_job(&h).await;
    sqlx::query("UPDATE workflows.jobs SET status='cancelled', lease_owner=NULL, lease_until=NULL WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(cancel_run(&h, &format!("job-{id}")).await, 200);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.restart_retries), ("cancelled", 0));
}

#[tokio::test]
async fn a_genuine_agent_failure_uses_no_restart_retry() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let id = running_agent_job(&h).await;
    assert_eq!(
        fail_run(&h, &format!("job-{id}"), "compile error").await,
        200
    );
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.restart_retries), ("failed", 0));
}

#[tokio::test]
async fn deadline_poll_requeues_a_cancelled_run() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let id = running_agent_job(&h).await;
    h.sandbox
        .finish_with_state(&format!("job-{id}"), RunPhase::Cancelled, "restarted");
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 1);
    let j = job(pool, id).await;
    assert_eq!(
        (j.status.as_str(), j.max_attempts, j.restart_retries),
        ("queued", 6, 1)
    );
    assert!(events_of(pool, "JobFailed").await.is_empty());
}

#[tokio::test]
async fn deadline_poll_requeues_a_run_the_sandbox_forgot() {
    let h = H::new(db!()).await;
    let pool = h.pool();
    let id = running_agent_job(&h).await;
    h.sandbox.forget(&format!("job-{id}"));
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id=$1")
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(h.exec.poll_deadlines().await.unwrap(), 1);
    let j = job(pool, id).await;
    assert_eq!((j.status.as_str(), j.restart_retries), ("queued", 1));
    assert!(events_of(pool, "JobFailed").await.is_empty());
}
