use engine::db::{JobView, TransitionView, WorkItemView};
use engine::testing::TestDb;
use serde_json::json;
use sqlx::postgres::PgListener;

macro_rules! db {
    () => {
        match TestDb::start().await {
            Some(db) => db,
            None => return,
        }
    };
}

async fn ingest(pool: &sqlx::PgPool, dedupe: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
        .bind("github")
        .bind(dedupe)
        .bind("github:enzarb/enzarb-ui#42")
        .bind("CommentAdded")
        .bind(json!({"n": 1}))
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn migrate_is_idempotent_and_history_is_schema_scoped() {
    let db = db!();
    engine::db::migrate(&db.pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workflows._sqlx_migrations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, engine::db::MIGRATOR.migrations.len() as i64);
    let public: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations')::text")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(public.is_none());
}

#[tokio::test]
async fn ingest_dedupes_and_returns_null_on_duplicate() {
    let db = db!();
    let first = ingest(&db.pool, "github:d1").await;
    assert!(first.is_some());
    assert_eq!(ingest(&db.pool, "github:d1").await, None);
    assert!(ingest(&db.pool, "github:d2").await.is_some());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM workflows.events")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
    let ev: engine::db::Event = sqlx::query_as("SELECT * FROM workflows.events WHERE id = $1")
        .bind(first.unwrap())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(ev.r#type, "CommentAdded");
    assert!(ev.processed_at.is_none());
}

#[tokio::test]
async fn ingest_notifies_workflow_events_once_per_new_event() {
    let db = db!();
    let mut l = PgListener::connect(&db.url).await.unwrap();
    l.listen("workflow_events").await.unwrap();
    let id = ingest(&db.pool, "github:n1").await.unwrap();
    let n = tokio::time::timeout(std::time::Duration::from_secs(5), l.recv())
        .await
        .expect("notify timeout")
        .unwrap();
    assert_eq!(n.channel(), "workflow_events");
    assert_eq!(n.payload(), id.to_string());
    // Duplicate must not notify.
    assert_eq!(ingest(&db.pool, "github:n1").await, None);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(500), l.recv())
            .await
            .is_err()
    );
}

async fn seed_work_item(db: &TestDb) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data)
         VALUES ('github:enzarb/enzarb-ui#42', 'enzarb-ui', 'issue', 'Triaging', '{\"a\":1}') RETURNING id",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn submit_action_creates_ui_event() {
    let db = db!();
    let wi = seed_work_item(&db).await;
    let ui = db.pool_as("enzarb_ui").await;
    let id: i64 = sqlx::query_scalar("SELECT workflows_api.submit_action($1, $2, $3, $4)")
        .bind(wi)
        .bind(json!({"login": "ruckc"}))
        .bind("approve")
        .bind(json!({"note": "ok"}))
        .fetch_one(&ui)
        .await
        .unwrap();
    let ev: engine::db::Event = sqlx::query_as("SELECT * FROM workflows.events WHERE id = $1")
        .bind(id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(ev.dedupe_key.starts_with("ui:"));
    assert_eq!(ev.source, "ui");
    assert_eq!(ev.work_item_key, "github:enzarb/enzarb-ui#42");
    assert_eq!(ev.r#type, "ApprovalGranted");
    assert_eq!(ev.payload["actor"]["login"], "ruckc");
    assert_eq!(ev.payload["action"], "approve");

    for (action, ty) in [
        ("cancel", "CancelRequested"),
        ("retry", "RetryRequested"),
        ("merge", "MergeRequested"),
        ("repo-removed", "RepoRemoved"),
    ] {
        let id: i64 = sqlx::query_scalar("SELECT workflows_api.submit_action($1, '{}', $2, '{}')")
            .bind(wi)
            .bind(action)
            .fetch_one(&ui)
            .await
            .unwrap();
        let t: String = sqlx::query_scalar("SELECT type FROM workflows.events WHERE id = $1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(t, ty);
    }
    // A comment carries its text; the body is required and capped.
    let id: i64 = sqlx::query_scalar("SELECT workflows_api.submit_action($1, $2, 'comment', $3)")
        .bind(wi)
        .bind(json!({"login": "ruckc"}))
        .bind(json!({"body": "use CSS vars"}))
        .fetch_one(&ui)
        .await
        .unwrap();
    let (t, payload): (String, serde_json::Value) =
        sqlx::query_as("SELECT type, payload FROM workflows.events WHERE id = $1")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(t, "CommentAdded");
    assert_eq!(payload["action"], "comment");
    assert_eq!(payload["data"]["body"], "use CSS vars");
    for bad in [
        json!({}),
        json!({"body": "   "}),
        json!({"body": 3}),
        json!({"body": "x".repeat(20001)}),
    ] {
        assert!(
            sqlx::query("SELECT workflows_api.submit_action($1, '{}', 'comment', $2)")
                .bind(wi)
                .bind(&bad)
                .execute(&ui)
                .await
                .is_err(),
            "{bad}"
        );
    }
    // Bad action and unknown work item are rejected.
    assert!(
        sqlx::query("SELECT workflows_api.submit_action($1, '{}', 'nuke', '{}')")
            .bind(wi)
            .execute(&ui)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT workflows_api.submit_action(999999, '{}', 'approve', '{}')")
            .execute(&ui)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn views_return_expected_columns() {
    let db = db!();
    let wi = seed_work_item(&db).await;
    sqlx::query(
        "INSERT INTO workflows.transitions (work_item_id, from_state, to_state, event_type)
         VALUES ($1, NULL, 'Triaging', 'WorkItemOpened')",
    )
    .bind(wi)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO workflows.jobs (work_item_id, kind, idempotency_key, output)
         VALUES ($1, 'agent_task', 'k1', '{\"url\":\"x\"}')",
    )
    .bind(wi)
    .execute(&db.pool)
    .await
    .unwrap();

    let ui = db.pool_as("enzarb_ui").await;
    let w: WorkItemView = sqlx::query_as("SELECT id, key, project, workflow, state, state_data, created_at, updated_at, waiting_for, resumes_at, usage_totals, waiting_on, wait_reason, wait_text FROM workflows_api.work_item_v")
        .fetch_one(&ui).await.unwrap();
    assert_eq!(
        (w.id, w.project.as_deref(), w.state.as_str()),
        (wi, Some("enzarb-ui"), "Triaging")
    );
    assert_eq!(
        (w.waiting_on, w.wait_reason, w.wait_text),
        (None, None, None)
    );
    assert_eq!(w.state_data["a"], 1);
    let t: TransitionView = sqlx::query_as("SELECT id, work_item_id, from_state, to_state, event_type, at FROM workflows_api.transition_v")
        .fetch_one(&ui).await.unwrap();
    assert_eq!(
        (t.from_state, t.event_type.as_str()),
        (None, "WorkItemOpened")
    );
    let j: JobView = sqlx::query_as("SELECT id, work_item_id, kind, status, attempt, created_at, updated_at, output, waiting_for, resumes_at, usage FROM workflows_api.job_v")
        .fetch_one(&ui).await.unwrap();
    assert_eq!((j.status.as_str(), j.attempt), ("queued", 0));
    assert_eq!(j.output.unwrap()["url"], "x");
    assert_eq!((j.waiting_for, j.resumes_at), (None, None));
    assert_eq!((w.waiting_for, w.resumes_at), (None, None));
    // No usage reported: unknown, not zero.
    assert_eq!((j.usage, w.usage_totals), (None, None));

    // A queued job waiting for a session limit shows on the job and its work item.
    sqlx::query(
        "UPDATE workflows.jobs SET waiting_for = 'session_limit', resumes_at = '2026-10-12T00:51:00Z'
         WHERE idempotency_key = 'k1'",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let w: WorkItemView = sqlx::query_as("SELECT id, key, project, workflow, state, state_data, created_at, updated_at, waiting_for, resumes_at, usage_totals, waiting_on, wait_reason, wait_text FROM workflows_api.work_item_v")
        .fetch_one(&ui).await.unwrap();
    let j: JobView = sqlx::query_as("SELECT id, work_item_id, kind, status, attempt, created_at, updated_at, output, waiting_for, resumes_at, usage FROM workflows_api.job_v")
        .fetch_one(&ui).await.unwrap();
    for (waiting_for, resumes_at) in [(w.waiting_for, w.resumes_at), (j.waiting_for, j.resumes_at)]
    {
        assert_eq!(waiting_for.as_deref(), Some("session_limit"));
        assert_eq!(
            resumes_at.unwrap().to_rfc3339(),
            "2026-10-12T00:51:00+00:00"
        );
    }
    assert_eq!(w.state, "Triaging");

    // The derived wait in state_data is exposed as columns.
    sqlx::query(
        "UPDATE workflows.work_items SET state_data = state_data ||
         '{\"wait\":{\"waiting_on\":\"human\",\"reason\":\"awaiting_review\",\"text\":\"Waiting for your review\"}}'
         WHERE id = $1",
    )
    .bind(wi)
    .execute(&db.pool)
    .await
    .unwrap();
    let w: WorkItemView = sqlx::query_as("SELECT id, key, project, workflow, state, state_data, created_at, updated_at, waiting_for, resumes_at, usage_totals, waiting_on, wait_reason, wait_text FROM workflows_api.work_item_v")
        .fetch_one(&ui).await.unwrap();
    assert_eq!(w.waiting_on.as_deref(), Some("human"));
    assert_eq!(w.wait_reason.as_deref(), Some("awaiting_review"));
    assert_eq!(w.wait_text.as_deref(), Some("Waiting for your review"));
    assert_eq!(w.waiting_for.as_deref(), Some("session_limit"));

    // Exact column lists (contract section 6).
    for (view, cols) in [
        (
            "work_item_v",
            "id,key,project,workflow,state,state_data,created_at,updated_at,waiting_for,resumes_at,usage_totals,waiting_on,wait_reason,wait_text,blocked_by_dependency",
        ),
        (
            "work_item_dependency_v",
            "work_item_key,depends_on_key,source,via,status,state,title,url",
        ),
        (
            "transition_v",
            "id,work_item_id,from_state,to_state,event_type,at",
        ),
        (
            "job_v",
            "id,work_item_id,kind,status,attempt,created_at,updated_at,output,waiting_for,resumes_at,usage",
        ),
    ] {
        let got: String = sqlx::query_scalar(
            "SELECT string_agg(column_name, ',' ORDER BY ordinal_position) FROM information_schema.columns
             WHERE table_schema = 'workflows_api' AND table_name = $1",
        )
        .bind(view)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(got, cols, "{view}");
    }
    // The UI role has no direct access to engine tables.
    assert!(
        sqlx::query("SELECT 1 FROM workflows.events")
            .execute(&ui)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn work_item_usage_totals_sum_the_jobs_that_reported() {
    let db = db!();
    let wi = seed_work_item(&db).await;
    let ui = db.pool_as("enzarb_ui").await;
    let totals = || async {
        let w: WorkItemView = sqlx::query_as(
            "SELECT id, key, project, workflow, state, state_data, created_at, updated_at, waiting_for, resumes_at, usage_totals, waiting_on, wait_reason, wait_text FROM workflows_api.work_item_v",
        )
        .fetch_one(&ui)
        .await
        .unwrap();
        w.usage_totals
    };
    for (key, usage) in [
        (
            "a",
            Some(
                json!({"input_tokens": 100, "output_tokens": 10, "cache_read_input_tokens": 1000,
                        "models": ["m1"], "cost_usd": 0.25}),
            ),
        ),
        (
            "b",
            Some(json!({"input_tokens": 50, "output_tokens": 5, "models": ["m1", "m2"]})),
        ),
        ("c", None),
    ] {
        sqlx::query(
            "INSERT INTO workflows.jobs (work_item_id, kind, idempotency_key, usage)
             VALUES ($1, 'agent_task', $2, $3)",
        )
        .bind(wi)
        .bind(key)
        .bind(usage)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let t = totals().await.expect("totals");
    assert_eq!(t["input_tokens"], 150);
    assert_eq!(t["output_tokens"], 15);
    assert_eq!(t["cache_read_input_tokens"], 1000);
    assert_eq!(t["cost_usd"], 0.25); // only the job that reported a cost
    assert_eq!(t["models"], json!(["m1", "m2"]));
    // nobody reported cache writes: the key is absent, not 0
    assert!(t.get("cache_creation_input_tokens").is_none());

    // Without any reported cost the total has no cost (not 0).
    sqlx::query("UPDATE workflows.jobs SET usage = usage - 'cost_usd'")
        .execute(&db.pool)
        .await
        .unwrap();
    let t = totals().await.expect("totals");
    assert!(t.get("cost_usd").is_none());
    assert_eq!(t["input_tokens"], 150);

    // The per-job view carries the job's own usage.
    let j: JobView = sqlx::query_as(
        "SELECT id, work_item_id, kind, status, attempt, created_at, updated_at, output, waiting_for, resumes_at, usage FROM workflows_api.job_v WHERE kind = 'agent_task' AND usage IS NOT NULL ORDER BY id LIMIT 1",
    )
    .fetch_one(&ui)
    .await
    .unwrap();
    assert_eq!(j.usage.unwrap()["input_tokens"], 100);

    // No job with usage: NULL.
    sqlx::query("UPDATE workflows.jobs SET usage = NULL")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(totals().await, None);
}

#[tokio::test]
async fn connectors_role_can_only_execute_ingest_event() {
    let db = db!();
    let wi = seed_work_item(&db).await;
    let c = db.pool_as("enzarb_connectors").await;
    let id: Option<i64> = sqlx::query_scalar("SELECT workflows_api.ingest_event('github','github:p1','github:enzarb/enzarb-ui#42','WorkItemOpened','{}')")
        .fetch_one(&c).await.unwrap();
    assert!(id.is_some());

    for sql in [
        "SELECT workflows_api.submit_action(1, '{}', 'approve', '{}')",
        "SELECT * FROM workflows_api.work_item_v",
        "SELECT * FROM workflows_api.job_v",
        "SELECT * FROM workflows_api.transition_v",
        "SELECT * FROM workflows.events",
        "SELECT * FROM workflows.jobs",
        "INSERT INTO workflows.events (source, dedupe_key, work_item_key, type) VALUES ('x','x','x','x')",
    ] {
        let err = sqlx::query(sql).execute(&c).await.expect_err(sql);
        assert_eq!(
            err.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501"),
            "{sql}: {err}"
        );
    }

    // And enzarb_ui cannot ingest.
    let ui = db.pool_as("enzarb_ui").await;
    assert!(
        sqlx::query("SELECT workflows_api.ingest_event('ui','ui:x','k','t','{}')")
            .execute(&ui)
            .await
            .is_err()
    );
    let _ = wi;
}

async fn metrics_item(db: &TestDb, project: &str, key: &str, created: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflows.work_items (key, project_id, workflow, state, created_at)
         VALUES ($1, $2, 'issue', 'Done', $3::timestamptz) RETURNING id",
    )
    .bind(key)
    .bind(project)
    .bind(created)
    .fetch_one(&db.pool)
    .await
    .unwrap()
}

async fn metrics_job(
    db: &TestDb,
    wi: i64,
    kind: &str,
    created: &str,
    updated: &str,
    status: &str,
    usage: Option<serde_json::Value>,
) {
    sqlx::query(
        "INSERT INTO workflows.jobs (work_item_id, kind, idempotency_key, status, usage, created_at, updated_at)
         VALUES ($1, $2, gen_random_uuid()::text, $3, $4, $5::timestamptz, $6::timestamptz)",
    )
    .bind(wi)
    .bind(kind)
    .bind(status)
    .bind(usage)
    .bind(created)
    .bind(updated)
    .execute(&db.pool)
    .await
    .unwrap();
}

/// A transition at `at` into `to_state`; ids ascend with call order.
async fn metrics_transition(db: &TestDb, wi: i64, to_state: &str, at: &str) {
    sqlx::query(
        "INSERT INTO workflows.transitions (work_item_id, to_state, event_type, at)
         VALUES ($1, $2, 'X', $3::timestamptz)",
    )
    .bind(wi)
    .bind(to_state)
    .bind(at)
    .execute(&db.pool)
    .await
    .unwrap();
}

const T0: &str = "2026-10-01T00:00:00Z";
const T1: &str = "2026-10-02T00:00:00Z";

type UsageRow = (
    String,
    Option<String>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
);

async fn usage_rows(ui: &sqlx::PgPool, project: &str, from: &str, to: &str) -> Vec<UsageRow> {
    sqlx::query_as(
        "SELECT kind, work_item_key, input_tokens::float8, cost_usd::float8, cost_usd_avg::float8, cost_usd_reporters
         FROM workflows_api.project_usage($1, $2::timestamptz, $3::timestamptz)",
    )
    .bind(project)
    .bind(from)
    .bind(to)
    .fetch_all(ui)
    .await
    .unwrap()
}

#[tokio::test]
async fn project_usage_window_null_vs_zero_and_project_scope() {
    let db = db!();
    let ui = db.pool_as("enzarb_ui").await;
    // at `from` (in), reports cost 0
    let a = metrics_item(&db, "p", "k:a", T0).await;
    metrics_job(
        &db,
        a,
        "agent_task",
        T0,
        T0,
        "succeeded",
        Some(json!({"input_tokens": 10, "cost_usd": 0})),
    )
    .await;
    // inside, reports cost 3
    let b = metrics_item(&db, "p", "k:b", "2026-10-01T12:00:00Z").await;
    metrics_job(
        &db,
        b,
        "agent_task",
        T0,
        T0,
        "succeeded",
        Some(json!({"input_tokens": 30, "cost_usd": 3})),
    )
    .await;
    // inside, reports nothing
    metrics_item(&db, "p", "k:c", "2026-10-01T13:00:00Z").await;
    // exactly at `to` (out)
    let d = metrics_item(&db, "p", "k:d", T1).await;
    metrics_job(
        &db,
        d,
        "agent_task",
        T0,
        T0,
        "succeeded",
        Some(json!({"input_tokens": 999, "cost_usd": 99})),
    )
    .await;
    // other project (out)
    let e = metrics_item(&db, "q", "k:e", "2026-10-01T12:00:00Z").await;
    metrics_job(
        &db,
        e,
        "agent_task",
        T0,
        T0,
        "succeeded",
        Some(json!({"input_tokens": 500, "cost_usd": 50})),
    )
    .await;

    let rows = usage_rows(&ui, "p", T0, T1).await;
    let total = &rows[0];
    assert_eq!(total.0, "total");
    assert_eq!(total.2, Some(40.0)); // input tokens: a + b
    assert_eq!(total.3, Some(3.0));
    assert_eq!(total.4, Some(1.5)); // avg over the two reporters, 0 counts
    assert_eq!(total.5, Some(2));
    let keys: Vec<_> = rows[1..].iter().map(|r| r.1.clone().unwrap()).collect();
    assert_eq!(keys, ["k:a", "k:b", "k:c"]);
    assert_eq!(rows[1].3, Some(0.0)); // reported 0
    assert_eq!(rows[3].3, None); // not reported
    assert_eq!(rows[3].2, None);

    let (items, median): (i64, Option<f64>) = sqlx::query_as(
        "SELECT items, cost_usd_median FROM workflows_api.project_usage('p', $1::timestamptz, $2::timestamptz) WHERE kind = 'total'",
    )
    .bind(T0)
    .bind(T1)
    .fetch_one(&ui)
    .await
    .unwrap();
    assert_eq!((items, median), (3, Some(1.5)));

    // Nobody reported in this window: NULL, not 0.
    let rows = usage_rows(&ui, "p", "2026-10-01T13:00:00Z", "2026-10-01T14:00:00Z").await;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0].2, rows[0].3, rows[0].4, rows[0].5),
        (None, None, None, Some(0))
    );
}

type TimingRow = (
    String,
    i64,
    i64,
    i64,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<f64>,
);

async fn timing(
    ui: &sqlx::PgPool,
    kind: &str,
    from: &str,
    to: &str,
    open: bool,
) -> Vec<(String, TimingRow)> {
    let rows: Vec<TimingRow> = sqlx::query_as(
        "SELECT name, n, open_excluded, wait_count, wait_s, min_s, max_s, avg_s, median_s, p95_s
         FROM workflows_api.project_step_timing('p', $1::timestamptz, $2::timestamptz, $3)
         WHERE kind = $4 ORDER BY name",
    )
    .bind(from)
    .bind(to)
    .bind(open)
    .bind(kind)
    .fetch_all(ui)
    .await
    .unwrap();
    rows.into_iter().map(|r| (r.0.clone(), r)).collect()
}

#[tokio::test]
async fn project_step_timing_statistics_windows_open_and_scope() {
    let db = db!();
    let ui = db.pool_as("enzarb_ui").await;
    // Durations in `Working`: 10, 20, 30, 40 s. Each item then ends in `Done` (open).
    for (i, secs) in [10, 20, 30, 40].into_iter().enumerate() {
        let wi = metrics_item(&db, "p", &format!("k:{i}"), T0).await;
        let start = format!("2026-10-01T00:{:02}:00Z", i);
        let end = format!("2026-10-01T00:{:02}:{:02}Z", i, secs);
        metrics_transition(&db, wi, "Working", &start).await;
        metrics_transition(&db, wi, "Done", &end).await;
    }
    // Transition exactly at `to` is out of the window; so is another project's.
    let w = metrics_item(&db, "p", "k:late", T0).await;
    metrics_transition(&db, w, "Working", T1).await;
    metrics_transition(&db, w, "Done", "2026-10-02T00:01:00Z").await;
    let o = metrics_item(&db, "q", "k:other", T0).await;
    metrics_transition(&db, o, "Working", "2026-10-01T00:00:00Z").await;
    metrics_transition(&db, o, "Done", "2026-10-01T05:00:00Z").await;

    let rows = timing(&ui, "state", T0, T1, false).await;
    let working = &rows.iter().find(|r| r.0 == "Working").unwrap().1;
    // n, open_excluded, wait_count, wait_s, min, max, avg, median, p95
    assert_eq!((working.1, working.2, working.3), (4, 0, 0));
    assert_eq!(working.5, Some(10.0));
    assert_eq!(working.6, Some(40.0));
    assert_eq!(working.7, Some(25.0));
    assert_eq!(working.8, Some(25.0));
    assert!((working.9.unwrap() - 38.5).abs() < 1e-9); // 30 + 0.85 * 10
    // The open `Done` states are excluded and flagged.
    let done = &rows.iter().find(|r| r.0 == "Done").unwrap().1;
    assert_eq!((done.1, done.2, done.5), (0, 4, None));

    // Including the open state measures it up to now().
    let rows = timing(&ui, "state", T0, T1, true).await;
    let done = &rows.iter().find(|r| r.0 == "Done").unwrap().1;
    assert_eq!((done.1, done.2), (4, 0));
    assert!(done.6.unwrap() > 86_400.0);

    // Window start is inclusive: from just after the first start drops one sample.
    let rows = timing(&ui, "state", "2026-10-01T00:00:01Z", T1, false).await;
    assert_eq!(rows.iter().find(|r| r.0 == "Working").unwrap().1.1, 3);
}

#[tokio::test]
async fn project_step_timing_separates_waiting_and_breaks_down_jobs() {
    let db = db!();
    let ui = db.pool_as("enzarb_ui").await;
    let wi = metrics_item(&db, "p", "k:w", T0).await;
    sqlx::query("UPDATE workflows.work_items SET state_data = '{\"wait\":{\"waiting_on\":\"human\",\"reason\":\"awaiting_review\",\"text\":\"t\"}}' WHERE id = $1")
        .bind(wi)
        .execute(&db.pool)
        .await
        .unwrap();
    metrics_transition(&db, wi, "Working", T0).await;
    metrics_transition(&db, wi, "AwaitingReview", "2026-10-01T00:00:10Z").await;

    for include_open in [false, true] {
        let rows = timing(&ui, "state", T0, T1, include_open).await;
        let w = &rows.iter().find(|r| r.0 == "AwaitingReview").unwrap().1;
        assert_eq!((w.1, w.2, w.3), (0, 0, 1), "include_open={include_open}");
        assert!(w.4.unwrap() > 86_400.0);
        assert_eq!(w.5, None);
        let working = &rows.iter().find(|r| r.0 == "Working").unwrap().1;
        assert_eq!((working.1, working.3, working.7), (1, 0, Some(10.0)));
    }

    // Jobs by kind: finished ones only; window half-open on created_at; other project excluded.
    metrics_job(
        &db,
        wi,
        "agent_task",
        T0,
        "2026-10-01T00:01:00Z",
        "succeeded",
        None,
    )
    .await;
    metrics_job(
        &db,
        wi,
        "agent_task",
        "2026-10-01T01:00:00Z",
        "2026-10-01T01:03:00Z",
        "failed",
        None,
    )
    .await;
    metrics_job(
        &db,
        wi,
        "agent_task",
        "2026-10-01T02:00:00Z",
        "2026-10-01T09:00:00Z",
        "running",
        None,
    )
    .await;
    metrics_job(
        &db,
        wi,
        "agent_task",
        T1,
        "2026-10-02T00:10:00Z",
        "succeeded",
        None,
    )
    .await;
    metrics_job(
        &db,
        wi,
        "merge",
        T0,
        "2026-10-01T00:00:05Z",
        "succeeded",
        None,
    )
    .await;
    let o = metrics_item(&db, "q", "k:o", T0).await;
    metrics_job(
        &db,
        o,
        "merge",
        T0,
        "2026-10-01T05:00:00Z",
        "succeeded",
        None,
    )
    .await;

    let rows = timing(&ui, "job", T0, T1, false).await;
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        ["agent_task", "merge"]
    );
    let a = &rows[0].1;
    assert_eq!(
        (a.1, a.5, a.6, a.7),
        (2, Some(60.0), Some(180.0), Some(120.0))
    );
    let m = &rows[1].1;
    assert_eq!((m.1, m.6), (1, Some(5.0)));
}

#[tokio::test]
async fn metrics_functions_are_granted_to_ui_only() {
    let db = db!();
    let c = db.pool_as("enzarb_connectors").await;
    for sql in [
        "SELECT * FROM workflows_api.project_usage('p', now(), now())",
        "SELECT * FROM workflows_api.project_step_timing('p', now(), now())",
    ] {
        let err = sqlx::query(sql).execute(&c).await.expect_err(sql);
        assert_eq!(
            err.as_database_error().and_then(|e| e.code()).as_deref(),
            Some("42501"),
            "{sql}: {err}"
        );
    }
    let ui = db.pool_as("enzarb_ui").await;
    sqlx::query("SELECT * FROM workflows_api.project_usage('p', now(), now())")
        .execute(&ui)
        .await
        .unwrap();
}

#[tokio::test]
async fn claim_query_uses_priority_then_run_after() {
    let db = db!();
    let wi = seed_work_item(&db).await;
    for (key, prio, offset) in [
        ("a", 0, "-1 minute"),
        ("b", 10, "-1 second"),
        ("c", 10, "-1 minute"),
        ("d", 20, "+1 hour"),
    ] {
        sqlx::query(&format!(
            "INSERT INTO workflows.jobs (work_item_id, kind, idempotency_key, priority, run_after)
             VALUES ($1, 'k', $2, $3, now() + interval '{offset}')"
        ))
        .bind(wi)
        .bind(key)
        .bind(prio)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let order: Vec<String> = sqlx::query_scalar(
        "SELECT idempotency_key FROM workflows.jobs WHERE status = 'queued' AND run_after <= now()
         ORDER BY priority DESC, run_after, id FOR UPDATE SKIP LOCKED",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(order, ["c", "b", "a"]);
    // `cancelled` is a valid status; junk is not.
    sqlx::query("UPDATE workflows.jobs SET status = 'cancelled' WHERE idempotency_key = 'a'")
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE workflows.jobs SET status = 'bogus'")
            .execute(&db.pool)
            .await
            .is_err()
    );
}
