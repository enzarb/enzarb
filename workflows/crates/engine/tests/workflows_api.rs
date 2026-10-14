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
