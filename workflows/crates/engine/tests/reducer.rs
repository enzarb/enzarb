//! Reducer, ticker and job-result integration tests against Postgres (testcontainers),
//! with a fake `ProjectIndex` and a fake `PermissionResolver`.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use engine::db::{Job, Transition, WorkItem};
use engine::machine::JobKind;
use engine::testing::TestDb;
use engine::{
    JobResult, Reducer, ReducerConfig, StaticPermissions, StaticProjectIndex, Ticker,
    ingest_job_result,
};
use enzarb_connector::{
    Actor, CommentRef, Event, EventData, EventType, ItemKind, ItemRef, Permission, RepoKey,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::watch;

macro_rules! db {
    () => {
        match TestDb::start().await {
            Some(db) => db,
            None => return,
        }
    };
}

const REPO: &str = "github:enzarb/enzarb-ui";

struct Harness {
    db: TestDb,
    index: Arc<StaticProjectIndex>,
    perms: Arc<StaticPermissions>,
    reducer: Arc<Reducer>,
}

impl Harness {
    async fn new(db: TestDb) -> Self {
        Self::with_config(db, ReducerConfig::default()).await
    }

    async fn with_config(db: TestDb, cfg: ReducerConfig) -> Self {
        let index = StaticProjectIndex::shared();
        index.insert(
            REPO,
            engine::ProjectInfo {
                name: "enzarb".into(),
                triggers: engine::machine::Triggers::default(),
                default_branch: "main".into(),
            },
        );
        let perms = Arc::new(StaticPermissions::new(Permission::Read));
        perms.set("alice", Permission::Write);
        let reducer = Arc::new(Reducer::new(
            db.pool.clone(),
            index.clone(),
            perms.clone(),
            cfg,
        ));
        Self {
            db,
            index,
            perms,
            reducer,
        }
    }

    fn pool(&self) -> &PgPool {
        &self.db.pool
    }
}

fn actor(login: &str) -> Actor {
    Actor {
        login: login.into(),
        id: 7,
        is_bot: false,
    }
}

fn item(kind: ItemKind, number: u64) -> ItemRef {
    ItemRef {
        kind,
        number,
        url: format!("https://github.com/enzarb/enzarb-ui/issues/{number}"),
        title: "Add dark mode".into(),
        body: "Please add a dark theme.".into(),
        author: "alice".into(),
        head_ref: (kind == ItemKind::ChangeRequest).then(|| "feat/x".into()),
        base_ref: (kind == ItemKind::ChangeRequest).then(|| "main".into()),
        head_sha: None,
    }
}

/// Ingest a connector event for `repo` (any case). Returns the event id, `None` if a duplicate.
async fn ingest(
    pool: &PgPool,
    delivery: &str,
    repo: &str,
    number: u64,
    ty: EventType,
    who: &str,
    comment: Option<&str>,
) -> Option<i64> {
    let repo_key: RepoKey = repo.parse().unwrap();
    let data = EventData {
        repo: repo_key.clone(),
        actor: actor(who),
        item: item(ItemKind::Issue, number),
        comment: comment.map(|b| CommentRef {
            id: 9,
            kind: Default::default(),
            body: b.into(),
            url: String::new(),
        }),
        merged: None,
        review: None,
        ci: None,
    };
    let ev = Event::new("github", delivery, ty, Utc::now(), data);
    sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
        .bind("github")
        .bind(ev.dedupe_key())
        .bind(ev.subject.to_string())
        .bind(ty.as_str())
        .bind(serde_json::to_value(&ev).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn open(pool: &PgPool, delivery: &str, repo: &str, number: u64) -> Option<i64> {
    ingest(
        pool,
        delivery,
        repo,
        number,
        EventType::WorkItemOpened,
        "alice",
        None,
    )
    .await
}

async fn work_item(pool: &PgPool, key: &str) -> Option<WorkItem> {
    sqlx::query_as("SELECT * FROM workflows.work_items WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn jobs(pool: &PgPool, work_item_id: i64) -> Vec<Job> {
    sqlx::query_as("SELECT * FROM workflows.jobs WHERE work_item_id = $1 ORDER BY id")
        .bind(work_item_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn job_of_kind(pool: &PgPool, work_item_id: i64, kind: JobKind) -> Job {
    jobs(pool, work_item_id)
        .await
        .into_iter()
        .rev()
        .find(|j| j.kind == kind.as_str())
        .unwrap_or_else(|| panic!("no {kind:?} job"))
}

async fn transitions(pool: &PgPool, work_item_id: i64) -> Vec<Transition> {
    sqlx::query_as("SELECT * FROM workflows.transitions WHERE work_item_id = $1 ORDER BY id")
        .bind(work_item_id)
        .fetch_all(pool)
        .await
        .unwrap()
}

/// What an executor does when it picks a job up.
async fn start_job(pool: &PgPool, job_id: i64) -> i32 {
    sqlx::query_scalar(
        "UPDATE workflows.jobs SET status = 'running', attempt = attempt + 1 WHERE id = $1 RETURNING attempt",
    )
    .bind(job_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn finish_ok(pool: &PgPool, job: &Job, output: Value) {
    let attempt = start_job(pool, job.id).await;
    let id = ingest_job_result(pool, job.id, attempt, JobResult::Succeeded { output })
        .await
        .unwrap();
    assert!(id.is_some());
}

fn agent_output(decision: &str, body: &str) -> Value {
    let block = json!({ "decision": decision, "body": body });
    json!({
        "summary": format!("Done.\n\n```json\n{block}\n```\n"),
        "branch": "enzarb/issue-1",
        "head_sha": "abc",
        "pushed": true,
    })
}

#[tokio::test]
async fn full_issue_flow_through_handoff() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    let key = "github:enzarb/enzarb-ui#1";

    // open -> triage job queued (plus progress markers)
    open(pool, "d1", REPO, 1).await.unwrap();
    assert_eq!(h.reducer.drain().await, 1);
    let w = work_item(pool, key).await.unwrap();
    assert_eq!(
        (w.workflow.as_str(), w.state.as_str()),
        ("issue", "Triaging")
    );
    assert_eq!(w.project_id.as_deref(), Some("enzarb"));
    let js = jobs(pool, w.id).await;
    let kinds: Vec<_> = js.iter().map(|j| j.kind.as_str()).collect();
    assert_eq!(kinds, ["set_progress", "set_progress", "agent_triage"]);
    assert!(js.iter().all(|j| j.project_id.as_deref() == Some("enzarb")));
    assert_eq!(transitions(pool, w.id).await.len(), 1);

    // triage result -> plan job
    let triage = job_of_kind(pool, w.id, JobKind::AgentTriage).await;
    finish_ok(pool, &triage, agent_output("ready", "Looks clear.")).await;
    assert_eq!(h.reducer.drain().await, 1);
    assert_eq!(work_item(pool, key).await.unwrap().state, "Planning");
    let plan = job_of_kind(pool, w.id, JobKind::AgentPlan).await;
    assert_eq!(plan.status, "queued");

    // plan result -> awaiting approval
    finish_ok(pool, &plan, agent_output("plan", "1. do it")).await;
    h.reducer.drain().await;
    assert_eq!(
        work_item(pool, key).await.unwrap().state,
        "AwaitingApproval"
    );

    // approval from a reader is rejected (audit row, no state change) ...
    ingest(
        pool,
        "d2",
        REPO,
        1,
        EventType::ApprovalGranted,
        "mallory",
        Some("/approve"),
    )
    .await
    .unwrap();
    h.reducer.drain().await;
    assert_eq!(
        work_item(pool, key).await.unwrap().state,
        "AwaitingApproval"
    );
    let rejected = transitions(pool, w.id).await.pop().unwrap();
    assert_eq!(rejected.from_state.as_deref(), Some("AwaitingApproval"));
    assert_eq!(rejected.to_state, "AwaitingApproval");
    assert!(rejected.note.unwrap().starts_with("rejected:"));

    // ... a writer's approval queues the implement job
    ingest(
        pool,
        "d3",
        REPO,
        1,
        EventType::ApprovalGranted,
        "alice",
        Some("/approve"),
    )
    .await
    .unwrap();
    h.reducer.drain().await;
    assert_eq!(work_item(pool, key).await.unwrap().state, "Implementing");
    let implement = job_of_kind(pool, w.id, JobKind::AgentImplement).await;
    assert_eq!(implement.status, "queued");

    // implement result -> open_change_request job -> result -> handoff
    finish_ok(pool, &implement, agent_output("done", "PR body")).await;
    h.reducer.drain().await;
    let ocr = job_of_kind(pool, w.id, JobKind::OpenChangeRequest).await;
    finish_ok(
        pool,
        &ocr,
        serde_json::to_value(item(ItemKind::ChangeRequest, 2)).unwrap(),
    )
    .await;
    // issue -> Done, handoff event inserted, and processed by the same drain
    assert_eq!(h.reducer.drain().await, 2);
    assert_eq!(work_item(pool, key).await.unwrap().state, "Done");
    let cr = work_item(pool, "github:enzarb/enzarb-ui#2").await.unwrap();
    assert_eq!(
        (cr.workflow.as_str(), cr.state.as_str()),
        ("change_request", "Reviewing")
    );
    assert!(!jobs(pool, cr.id).await.is_empty());
    let handoffs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflows.events WHERE dedupe_key LIKE 'handoff:%' AND processed_at IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(handoffs, 1);

    // every transition of the issue chains from the previous one
    let ts = transitions(pool, w.id).await;
    for pair in ts.windows(2) {
        assert_eq!(
            pair[1].from_state.as_deref(),
            Some(pair[0].to_state.as_str())
        );
    }
    // processed results never leave events pending
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflows.events WHERE processed_at IS NULL")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn unrouted_events_are_ignored_and_create_no_work_item() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();

    // no Project for this repository
    open(pool, "u1", "github:someone/else", 5).await.unwrap();
    // a Project exists, but a plain comment on an unknown item starts nothing
    ingest(
        pool,
        "u2",
        REPO,
        6,
        EventType::CommentAdded,
        "alice",
        Some("hi"),
    )
    .await
    .unwrap();
    // triggers off
    h.index.insert(
        "github:enzarb/quiet",
        engine::ProjectInfo {
            name: "quiet".into(),
            triggers: engine::machine::Triggers {
                issues: false,
                change_requests: true,
                mentions: true,
            },
            default_branch: "main".into(),
        },
    );
    open(pool, "u3", "github:enzarb/quiet", 7).await.unwrap();
    // garbage payload for a known type
    sqlx::query("SELECT workflows_api.ingest_event('github','github:u4','github:enzarb/enzarb-ui#8','CommentAdded','{\"nope\":1}')")
        .execute(pool)
        .await
        .unwrap();

    assert_eq!(h.reducer.drain().await, 4);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workflows.work_items")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
    let rows: Vec<(Option<i64>, String, Option<String>)> = sqlx::query_as(
        "SELECT work_item_id, to_state, note FROM workflows.transitions ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 4);
    assert!(
        rows.iter()
            .all(|(w, s, n)| w.is_none() && s == "ignored" && n.is_some())
    );
    assert!(rows[0].2.as_deref().unwrap().contains("no project"));
    let pending: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflows.events WHERE processed_at IS NULL")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(pending, 0);
}

#[tokio::test]
async fn repo_keys_match_case_insensitively_and_work_items_are_stored_lowercase() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    open(pool, "c1", "github:Enzarb/Enzarb-UI", 3)
        .await
        .unwrap();
    // a later event with yet another casing lands on the same work item
    ingest(
        pool,
        "c2",
        "github:ENZARB/enzarb-ui",
        3,
        EventType::WorkItemClosed,
        "alice",
        None,
    )
    .await
    .unwrap();
    h.reducer.drain().await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workflows.work_items")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    let w = work_item(pool, "github:enzarb/enzarb-ui#3").await.unwrap();
    assert_eq!(w.state, "Cancelled");
    // job inputs use the lowercase repo
    let triage = job_of_kind(pool, w.id, JobKind::AgentTriage).await;
    assert_eq!(triage.input["repo"], json!("github:enzarb/enzarb-ui"));
}

#[tokio::test]
async fn duplicate_deliveries_do_not_duplicate_jobs() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    assert!(open(pool, "dup", REPO, 1).await.is_some());
    assert!(open(pool, "dup", REPO, 1).await.is_none());
    h.reducer.drain().await;
    let w = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    assert_eq!(jobs(pool, w.id).await.len(), 3);

    // Even a replayed event (processed_at cleared by an operator) cannot add jobs:
    // the machine rejects the reopen, and job keys are deterministic per event.
    sqlx::query("UPDATE workflows.events SET processed_at = NULL")
        .execute(pool)
        .await
        .unwrap();
    h.reducer.drain().await;
    assert_eq!(jobs(pool, w.id).await.len(), 3);
    assert_eq!(
        work_item(pool, "github:enzarb/enzarb-ui#1")
            .await
            .unwrap()
            .state,
        "Triaging"
    );
}

#[tokio::test]
async fn effects_cancel_before_new_jobs_and_accepted_noops_leave_no_transition() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    open(pool, "e1", REPO, 1).await.unwrap();
    h.reducer.drain().await;
    let w = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    let before = transitions(pool, w.id).await.len();

    // progress acks from set_progress are accepted no-ops: no transition row
    let progress = job_of_kind(pool, w.id, JobKind::SetProgress).await;
    finish_ok(pool, &progress, json!({})).await;
    h.reducer.drain().await;
    assert_eq!(transitions(pool, w.id).await.len(), before);

    // closing cancels the queued and running jobs
    start_job(pool, job_of_kind(pool, w.id, JobKind::AgentTriage).await.id).await;
    ingest(
        pool,
        "e2",
        REPO,
        1,
        EventType::WorkItemClosed,
        "alice",
        None,
    )
    .await
    .unwrap();
    h.reducer.drain().await;
    let js = jobs(pool, w.id).await;
    let triage = js.iter().find(|j| j.kind == "agent_triage").unwrap();
    assert_eq!(triage.status, "cancelled");
    // a late result for the cancelled job is dropped
    let late = ingest_job_result(
        pool,
        triage.id,
        1,
        JobResult::Succeeded { output: json!({}) },
    )
    .await
    .unwrap();
    assert!(late.is_none());
}

#[tokio::test]
async fn permission_is_resolved_per_event_cached_and_outages_retry() {
    let cfg = ReducerConfig::default();
    let h = Harness::with_config(db!(), cfg).await;
    let pool = h.pool();

    h.perms.set_failing(true);
    let id = open(pool, "p1", REPO, 1).await.unwrap();
    assert_eq!(h.reducer.drain().await, 0);
    let processed: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT processed_at FROM workflows.events WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert!(processed.is_none());
    assert!(work_item(pool, "github:enzarb/enzarb-ui#1").await.is_none());

    h.perms.set_failing(false);
    assert_eq!(h.reducer.drain().await, 1);
    assert!(work_item(pool, "github:enzarb/enzarb-ui#1").await.is_some());

    // events without an actor and bot actors never call the resolver
    let calls = h.perms.calls();
    let w = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    sqlx::query("SELECT workflows_api.ingest_event('engine','timer:x','github:enzarb/enzarb-ui#1','TimerFired','{}')")
        .execute(pool)
        .await
        .unwrap();
    h.reducer.drain().await;
    assert_eq!(h.perms.calls(), calls);
    assert!(!transitions(pool, w.id).await.is_empty());
}

#[tokio::test]
async fn concurrent_events_on_the_same_item_are_serialized() {
    let cfg = ReducerConfig {
        workers: 6,
        poll_interval: Duration::from_millis(50),
        batch: 8,
        ..Default::default()
    };
    let h = Harness::with_config(db!(), cfg).await;
    let pool = h.pool();

    // For each item: open, then close. Reordering would leave an item Triaging (the close
    // would find nothing to cancel) or drop the open entirely.
    let items = 15u64;
    for n in 1..=items {
        open(pool, &format!("o{n}"), REPO, n).await.unwrap();
        ingest(
            pool,
            &format!("x{n}"),
            REPO,
            n,
            EventType::WorkItemClosed,
            "alice",
            None,
        )
        .await
        .unwrap();
    }
    let (tx, rx) = watch::channel(false);
    let run = tokio::spawn(h.reducer.clone().run(rx));
    wait_until(
        pool,
        "SELECT count(*) = 0 FROM workflows.events WHERE processed_at IS NULL",
    )
    .await;
    tx.send(true).unwrap();
    run.await.unwrap().unwrap();

    for n in 1..=items {
        let w = work_item(pool, &format!("github:enzarb/enzarb-ui#{n}"))
            .await
            .unwrap();
        assert_eq!(w.state, "Cancelled", "item {n}");
        let ts = transitions(pool, w.id).await;
        assert_eq!(ts.len(), 2, "item {n}");
        assert!(ts[0].event_id < ts[1].event_id);
        assert_eq!(ts[1].from_state.as_deref(), Some("Triaging"));
        // exactly one set of jobs for the open event, none duplicated
        let js = jobs(pool, w.id).await;
        assert_eq!(js.len(), 3, "item {n}");
    }
}

#[tokio::test]
async fn listen_notify_wakes_workers_before_the_poll_interval() {
    let cfg = ReducerConfig {
        workers: 2,
        poll_interval: Duration::from_secs(120),
        ..Default::default()
    };
    let h = Harness::with_config(db!(), cfg).await;
    let pool = h.pool();
    let (tx, rx) = watch::channel(false);
    let run = tokio::spawn(h.reducer.clone().run(rx));
    tokio::time::sleep(Duration::from_millis(500)).await; // listener is up, workers idle
    open(pool, "n1", REPO, 1).await.unwrap();
    wait_until(pool, "SELECT count(*) = 1 FROM workflows.work_items").await;
    tx.send(true).unwrap();
    run.await.unwrap().unwrap();
}

async fn wait_until(pool: &PgPool, sql: &str) {
    for _ in 0..200 {
        if sqlx::query_scalar::<_, bool>(sql)
            .fetch_one(pool)
            .await
            .unwrap()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {sql}");
}

#[tokio::test]
async fn ticker_fires_wake_at_once_and_the_machine_consumes_it() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    let ticker = Ticker::new(pool.clone());

    // Put an issue in AwaitingAnswers with a due timer.
    open(pool, "t1", REPO, 1).await.unwrap();
    h.reducer.drain().await;
    let w = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    let triage = job_of_kind(pool, w.id, JobKind::AgentTriage).await;
    finish_ok(pool, &triage, agent_output("questions", "What colors?")).await;
    h.reducer.drain().await;
    assert_eq!(
        work_item(pool, "github:enzarb/enzarb-ui#1")
            .await
            .unwrap()
            .state,
        "AwaitingAnswers"
    );
    let wake = work_item(pool, "github:enzarb/enzarb-ui#1")
        .await
        .unwrap()
        .wake_at;
    assert!(wake.is_some(), "machine set a wake_at");

    // not due yet
    assert_eq!(ticker.tick().await.unwrap().wakes, 0);
    sqlx::query("UPDATE workflows.work_items SET wake_at = now() - interval '1 minute'")
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(ticker.tick().await.unwrap().wakes, 1);
    // second and third ticks (and a concurrent ticker) fire nothing more
    assert_eq!(ticker.tick().await.unwrap().wakes, 0);
    let other = Ticker::new(pool.clone());
    let (a, b) = tokio::join!(ticker.tick(), other.tick());
    assert_eq!(a.unwrap().wakes + b.unwrap().wakes, 0);
    let timers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM workflows.events WHERE type = 'TimerFired'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(timers, 1);

    // the reducer applies it: first timer nudges, and sets a later wake_at
    h.reducer.drain().await;
    let after = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    assert_eq!(after.state, "AwaitingAnswers");
    assert!(after.wake_at.unwrap() > Utc::now());
    assert_eq!(ticker.tick().await.unwrap().wakes, 0);
}

#[tokio::test]
async fn ticker_turns_expired_job_deadlines_into_timer_events() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    let ticker = Ticker::new(pool.clone());
    open(pool, "j1", REPO, 1).await.unwrap();
    h.reducer.drain().await;
    let w = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    let triage = job_of_kind(pool, w.id, JobKind::AgentTriage).await;
    start_job(pool, triage.id).await;

    assert_eq!(ticker.tick().await.unwrap().deadlines, 0, "no deadline set");
    sqlx::query("UPDATE workflows.jobs SET deadline = now() + interval '1 hour' WHERE id = $1")
        .bind(triage.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(ticker.tick().await.unwrap().deadlines, 0, "not expired");
    sqlx::query("UPDATE workflows.jobs SET deadline = now() - interval '1 second' WHERE id = $1")
        .bind(triage.id)
        .execute(pool)
        .await
        .unwrap();
    assert_eq!(ticker.tick().await.unwrap().deadlines, 1);
    assert_eq!(ticker.tick().await.unwrap().deadlines, 0);

    let payload: Value =
        sqlx::query_scalar("SELECT payload FROM workflows.events WHERE type = 'TimerFired'")
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(payload, json!({"job_id": triage.id}));
    // the machine fails the step and cancels the stuck job
    h.reducer.drain().await;
    assert_eq!(
        work_item(pool, "github:enzarb/enzarb-ui#1")
            .await
            .unwrap()
            .state,
        "Failed"
    );
    let j = job_of_kind(pool, w.id, JobKind::AgentTriage).await;
    assert_eq!(j.status, "cancelled");
}

#[tokio::test]
async fn ingest_job_result_is_idempotent_and_drops_stale_attempts() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    open(pool, "r1", REPO, 1).await.unwrap();
    h.reducer.drain().await;
    let w = work_item(pool, "github:enzarb/enzarb-ui#1").await.unwrap();
    let triage = job_of_kind(pool, w.id, JobKind::AgentTriage).await;

    // queued jobs have no result yet
    assert!(
        ingest_job_result(pool, triage.id, 0, JobResult::Failed { error: "x".into() })
            .await
            .unwrap()
            .is_none()
    );
    let attempt = start_job(pool, triage.id).await;
    // wrong attempt
    assert!(
        ingest_job_result(
            pool,
            triage.id,
            attempt + 1,
            JobResult::Failed { error: "x".into() }
        )
        .await
        .unwrap()
        .is_none()
    );
    let id = ingest_job_result(
        pool,
        triage.id,
        attempt,
        JobResult::Failed {
            error: "boom".into(),
        },
    )
    .await
    .unwrap()
    .unwrap();
    let (ty, payload, key): (String, Value, String) =
        sqlx::query_as("SELECT type, payload, work_item_key FROM workflows.events WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(ty, "JobFailed");
    assert_eq!(
        payload,
        json!({"job_id": triage.id, "kind": "agent_triage", "error": "boom"})
    );
    assert_eq!(key, "github:enzarb/enzarb-ui#1");
    let j = job_of_kind(pool, w.id, JobKind::AgentTriage).await;
    assert_eq!(j.status, "failed");
    // repeated report: dropped
    assert!(
        ingest_job_result(
            pool,
            triage.id,
            attempt,
            JobResult::Failed {
                error: "boom".into()
            }
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(
        ingest_job_result(pool, 9999, 1, JobResult::Failed { error: "x".into() })
            .await
            .is_err()
    );

    // the machine turns the failure into Failed
    h.reducer.drain().await;
    assert_eq!(
        work_item(pool, "github:enzarb/enzarb-ui#1")
            .await
            .unwrap()
            .state,
        "Failed"
    );
}

#[tokio::test]
async fn runtime_role_can_run_the_reducer_and_ticker() {
    // The engine runs as enzarb_workflows in production, not as the schema owner.
    let db = db!();
    let pool = db.pool_as("enzarb_workflows").await;
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
    open(&db.pool, "rr1", REPO, 1).await.unwrap();
    assert_eq!(reducer.drain().await, 1);
    let w = work_item(&db.pool, "github:enzarb/enzarb-ui#1")
        .await
        .unwrap();
    let triage = job_of_kind(&db.pool, w.id, JobKind::AgentTriage).await;
    let attempt = start_job(&pool, triage.id).await;
    ingest_job_result(
        &pool,
        triage.id,
        attempt,
        JobResult::Succeeded {
            output: agent_output("ready", "ok"),
        },
    )
    .await
    .unwrap()
    .unwrap();
    sqlx::query("UPDATE workflows.work_items SET wake_at = now() - interval '1 second'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(Ticker::new(pool.clone()).tick().await.unwrap().wakes, 1);
    assert_eq!(reducer.drain().await, 2);
}

// -- work items whose repository left every Project ---------------------------

const ITEM: &str = "github:enzarb/enzarb-ui#1";

/// Open item #1 (its triage job queued), then drop the repository from the index.
async fn orphaned_item(h: &Harness) -> WorkItem {
    open(h.pool(), "o1", REPO, 1).await.unwrap();
    assert_eq!(h.reducer.drain().await, 1);
    h.index.set_owner("enzarb", "ruckc");
    h.index.remove(REPO);
    work_item(h.pool(), ITEM).await.unwrap()
}

async fn submit(pool: &PgPool, w: &WorkItem, who: &str, action: &str) {
    sqlx::query("SELECT workflows_api.submit_action($1, $2, $3, '{}')")
        .bind(w.id)
        .bind(json!({ "login": who, "id": 1, "is_bot": false }))
        .bind(action)
        .execute(pool)
        .await
        .unwrap();
}

async fn pending_jobs(pool: &PgPool, w: &WorkItem) -> usize {
    jobs(pool, w.id)
        .await
        .iter()
        .filter(|j| matches!(j.status.as_str(), "queued" | "running"))
        .count()
}

#[tokio::test]
async fn close_reaches_an_item_whose_repo_left_every_project() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    let w = orphaned_item(&h).await;
    // open queues two set_progress markers plus the triage job
    assert_eq!(pending_jobs(pool, &w).await, 3);

    ingest(
        pool,
        "o2",
        REPO,
        1,
        EventType::WorkItemClosed,
        "alice",
        None,
    )
    .await
    .unwrap();
    assert_eq!(h.reducer.drain().await, 1);

    let w = work_item(pool, ITEM).await.unwrap();
    assert_eq!(w.state, "Cancelled");
    assert_eq!(pending_jobs(pool, &w).await, 0, "running work is cancelled");
}

#[tokio::test]
async fn orphaned_items_reject_other_events() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    let w = orphaned_item(&h).await;
    let before = w.state.clone();

    ingest(
        pool,
        "o2",
        REPO,
        1,
        EventType::CommentAdded,
        "alice",
        Some("please continue"),
    )
    .await
    .unwrap();
    h.reducer.drain().await;

    let after = work_item(pool, ITEM).await.unwrap();
    assert_eq!(after.state, before);
    let last = transitions(pool, w.id).await.pop().unwrap();
    assert!(
        last.note
            .as_deref()
            .unwrap()
            .contains("no longer in a Project"),
        "{:?}",
        last.note
    );
}

#[tokio::test]
async fn close_without_an_existing_item_is_still_ignored() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    h.index.remove(REPO);
    ingest(
        pool,
        "o1",
        REPO,
        9,
        EventType::WorkItemClosed,
        "alice",
        None,
    )
    .await
    .unwrap();
    assert_eq!(h.reducer.drain().await, 1);
    assert!(work_item(pool, "github:enzarb/enzarb-ui#9").await.is_none());
}

#[tokio::test]
async fn project_owner_cancels_an_orphaned_item_and_others_cannot() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    let w = orphaned_item(&h).await;

    submit(pool, &w, "mallory", "cancel").await;
    submit(pool, &w, "RuckC", "approve").await;
    assert_eq!(h.reducer.drain().await, 2);
    assert_eq!(work_item(pool, ITEM).await.unwrap().state, w.state);

    submit(pool, &w, "RuckC", "cancel").await;
    assert_eq!(h.reducer.drain().await, 1);
    let after = work_item(pool, ITEM).await.unwrap();
    assert_eq!(after.state, "Cancelled");
    assert_eq!(pending_jobs(pool, &after).await, 0);
}

#[tokio::test]
async fn non_writer_owner_cannot_cancel_while_the_repo_is_in_a_project() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    open(pool, "o1", REPO, 1).await.unwrap();
    assert_eq!(h.reducer.drain().await, 1);
    h.index.set_owner("enzarb", "ruckc");
    let w = work_item(pool, ITEM).await.unwrap();

    submit(pool, &w, "ruckc", "cancel").await;
    assert_eq!(h.reducer.drain().await, 1);
    assert_eq!(work_item(pool, ITEM).await.unwrap().state, w.state);
}

#[tokio::test]
async fn repo_removed_action_cancels_without_permission() {
    let h = Harness::new(db!()).await;
    let pool = h.pool();
    open(pool, "o1", REPO, 1).await.unwrap();
    assert_eq!(h.reducer.drain().await, 1);
    let w = work_item(pool, ITEM).await.unwrap();

    submit(pool, &w, "nobody", "repo-removed").await;
    assert_eq!(h.reducer.drain().await, 1);
    let after = work_item(pool, ITEM).await.unwrap();
    assert_eq!(after.state, "Cancelled");
    assert_eq!(pending_jobs(pool, &after).await, 0);
    let last = transitions(pool, w.id).await.pop().unwrap();
    assert_eq!(
        last.note.as_deref(),
        Some("enzarb: repository removed from the Project.")
    );
}
