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

mod samples {
    use super::*;
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use engine::db::{SandboxSample, SandboxSeriesPoint, record_sandbox_sample, sandbox_series};
    use std::time::Duration as Std;

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    fn at(secs: i64) -> DateTime<Utc> {
        t0() + Duration::seconds(secs)
    }

    fn sample(project: &str, sandbox: &str, when: DateTime<Utc>) -> SandboxSample {
        SandboxSample {
            project: project.into(),
            work_item_id: None,
            sandbox: sandbox.into(),
            sampled_at: when,
            cpu_usage_ns: 0,
            memory_bytes: 100,
            storage_bytes: 1000,
            disk_read_bytes: 0,
            disk_write_bytes: 0,
        }
    }

    /// (secs, cpu_ns, memory, storage, read, write)
    type Row = (i64, i64, i64, i64, i64, i64);

    async fn put(pool: &sqlx::PgPool, project: &str, sandbox: &str, rows: &[Row]) {
        for &(secs, cpu, mem, sto, dr, dw) in rows {
            let s = SandboxSample {
                cpu_usage_ns: cpu,
                memory_bytes: mem,
                storage_bytes: sto,
                disk_read_bytes: dr,
                disk_write_bytes: dw,
                ..sample(project, sandbox, at(secs))
            };
            assert!(record_sandbox_sample(pool, &s).await.unwrap());
        }
    }

    async fn series(
        pool: &sqlx::PgPool,
        project: &str,
        from: i64,
        to: i64,
        bucket: u64,
    ) -> Vec<SandboxSeriesPoint> {
        sandbox_series(pool, project, at(from), at(to), Std::from_secs(bucket))
            .await
            .unwrap()
    }

    async fn count(pool: &sqlx::PgPool) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM workflows.sandbox_samples")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    fn close(a: Option<f64>, b: f64) {
        let a = a.expect("value");
        assert!((a - b).abs() < 1e-9, "{a} != {b}");
    }

    #[tokio::test]
    async fn insert_stores_and_dedupes() {
        let db = db!();
        let mut s = sample("p", "sb", at(0));
        s.work_item_id = Some(7);
        assert!(record_sandbox_sample(&db.pool, &s).await.unwrap());
        assert!(!record_sandbox_sample(&db.pool, &s).await.unwrap());
        assert_eq!(count(&db.pool).await, 1);
        let wi: Option<i64> =
            sqlx::query_scalar("SELECT work_item_id FROM workflows.sandbox_samples")
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(wi, Some(7));
    }

    #[tokio::test]
    async fn insert_rejects_invalid_input() {
        let db = db!();
        let ok = sample("p", "sb", at(0));
        for bad in [
            SandboxSample {
                cpu_usage_ns: -1,
                ..ok.clone()
            },
            SandboxSample {
                memory_bytes: -1,
                ..ok.clone()
            },
            SandboxSample {
                storage_bytes: -1,
                ..ok.clone()
            },
            SandboxSample {
                disk_read_bytes: -1,
                ..ok.clone()
            },
            SandboxSample {
                disk_write_bytes: -1,
                ..ok.clone()
            },
            SandboxSample {
                project: String::new(),
                ..ok.clone()
            },
            SandboxSample {
                sandbox: String::new(),
                ..ok.clone()
            },
            SandboxSample {
                sampled_at: Utc::now() + Duration::hours(1),
                ..ok.clone()
            },
        ] {
            assert!(
                record_sandbox_sample(&db.pool, &bad).await.is_err(),
                "{bad:?}"
            );
        }
        let null: Result<bool, _> = sqlx::query_scalar(
            "SELECT workflows_api.record_sandbox_sample('p', NULL, 'sb', now(), NULL, 1, 1, 1, 1)",
        )
        .fetch_one(&db.pool)
        .await;
        assert!(null.is_err());
        assert_eq!(count(&db.pool).await, 0);
    }

    #[tokio::test]
    async fn prune_deletes_only_old_samples_in_batches() {
        let db = db!();
        let now = Utc::now();
        for (i, days) in [31, 29, 0].into_iter().enumerate() {
            let s = sample(
                "p",
                &format!("sb{i}"),
                now - Duration::days(days) - Duration::seconds(1),
            );
            assert!(record_sandbox_sample(&db.pool, &s).await.unwrap());
        }
        let n: i64 = sqlx::query_scalar("SELECT workflows.prune_sandbox_samples()")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(count(&db.pool).await, 2);

        // Batching: a limit smaller than the backlog needs repeated calls.
        for i in 0..5 {
            let s = sample("q", "sb", now - Duration::days(40) - Duration::seconds(i));
            assert!(record_sandbox_sample(&db.pool, &s).await.unwrap());
        }
        let mut gone = Vec::new();
        loop {
            let n: i64 = sqlx::query_scalar("SELECT workflows.prune_sandbox_samples('30 days', 2)")
                .fetch_one(&db.pool)
                .await
                .unwrap();
            gone.push(n);
            if n < 2 {
                break;
            }
        }
        assert_eq!(gone, vec![2, 2, 1]);
        assert_eq!(count(&db.pool).await, 2);
        // Project does not matter: only the two recent rows of "p" remain.
        let projects: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflows.sandbox_samples WHERE project_id = 'p'",
        )
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(projects, 2);
    }

    #[tokio::test]
    async fn ticker_prunes_and_honours_the_throttle() {
        let db = db!();
        let old = sample("p", "sb", Utc::now() - Duration::days(10));
        assert!(record_sandbox_sample(&db.pool, &old).await.unwrap());
        let ticker =
            engine::Ticker::new(db.pool.clone()).with_sample_retention(Std::from_secs(24 * 3600));
        assert_eq!(ticker.prune_sandbox_samples().await.unwrap(), Some(1));
        assert_eq!(count(&db.pool).await, 0);
        let again = sample("p", "sb", Utc::now() - Duration::days(9));
        assert!(record_sandbox_sample(&db.pool, &again).await.unwrap());
        assert_eq!(ticker.prune_sandbox_samples().await.unwrap(), None);
        assert_eq!(count(&db.pool).await, 1);
    }

    #[tokio::test]
    async fn bucketing_aligns_gauges_and_fills_empty_buckets() {
        let db = db!();
        put(
            &db.pool,
            "p",
            "a",
            &[
                (5, 0, 100, 1000, 0, 0),
                (15, 0, 300, 2000, 0, 0),
                (45, 0, 50, 500, 0, 0),
            ],
        )
        .await;
        let s = series(&db.pool, "p", 0, 60, 20).await;
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].bucket, at(0));
        assert_eq!(s[1].bucket, at(20));
        assert_eq!(s[2].bucket, at(40));
        assert_eq!(s[0].samples, 2);
        close(s[0].memory_bytes_avg, 200.0);
        assert_eq!(s[0].memory_bytes_max, Some(300));
        assert_eq!(s[0].storage_bytes_max, Some(2000));
        assert_eq!(s[1].samples, 0);
        assert!(s[1].memory_bytes_avg.is_none() && s[1].cpu_cores.is_none());
        assert_eq!(s[2].memory_bytes_max, Some(50));
    }

    #[tokio::test]
    async fn multiple_sandboxes_are_summed() {
        let db = db!();
        let gb = 1_000_000_000;
        put(
            &db.pool,
            "p",
            "a",
            &[(0, 0, 100, 10, 0, 0), (10, 5 * gb, 100, 10, 1000, 0)],
        )
        .await;
        put(
            &db.pool,
            "p",
            "b",
            &[(0, 0, 200, 20, 0, 0), (10, 10 * gb, 200, 20, 3000, 0)],
        )
        .await;
        let s = series(&db.pool, "p", 0, 20, 20).await;
        assert_eq!(s.len(), 1);
        close(s[0].cpu_cores, 1.5);
        close(s[0].disk_read_bps, 400.0);
        close(s[0].memory_bytes_avg, 300.0);
        assert_eq!(s[0].memory_bytes_max, Some(300));
        assert_eq!(s[0].storage_bytes_max, Some(30));
        assert_eq!(s[0].samples, 4);
    }

    #[tokio::test]
    async fn rates_from_steady_counters() {
        let db = db!();
        let gb = 1_000_000_000;
        // 0.5 cores, 100 B/s read, 50 B/s write, polled every 10 s.
        let rows: Vec<Row> = (0..6)
            .map(|i| (i * 10, i * 5 * gb / 10, 1, 1, i * 1000, i * 500))
            .collect();
        put(&db.pool, "p", "a", &rows).await;
        let s = series(&db.pool, "p", 0, 60, 30).await;
        assert_eq!(s.len(), 2);
        // The first sample has no predecessor: bucket 0 has two intervals (10-20, 20-30 is
        // bucket 1), so only 10 and 20 count.
        close(s[0].cpu_cores, 0.5);
        close(s[0].disk_read_bps, 100.0);
        close(s[0].disk_write_bps, 50.0);
        close(s[1].cpu_cores, 0.5);
        close(s[1].disk_read_bps, 100.0);
    }

    #[tokio::test]
    async fn counter_reset_counts_the_new_value_and_never_goes_negative() {
        let db = db!();
        let gb = 1_000_000_000;
        put(
            &db.pool,
            "p",
            "a",
            &[
                (0, 100 * gb, 1, 1, 5000, 0),
                (10, 101 * gb, 1, 1, 6000, 0),
                (20, 2 * gb, 1, 1, 400, 0),
            ],
        )
        .await;
        let s = series(&db.pool, "p", 0, 30, 30).await;
        // Intervals: +1 core-second-ish gb over 10 s, then reset to 2 gb over 10 s.
        close(s[0].cpu_cores, 3.0 * gb as f64 / 20.0 / 1e9);
        close(s[0].disk_read_bps, (1000.0 + 400.0) / 20.0);
        assert!(s[0].disk_write_bps.unwrap() >= 0.0);
    }

    #[tokio::test]
    async fn first_sample_in_window_uses_the_sample_before_from() {
        let db = db!();
        let gb = 1_000_000_000;
        put(
            &db.pool,
            "p",
            "a",
            &[(-10, 0, 1, 1, 0, 0), (10, 10 * gb, 1, 1, 0, 0)],
        )
        .await;
        // A 20 s interval crosses `from`: it belongs to the bucket of its end sample.
        let s = series(&db.pool, "p", 0, 20, 20).await;
        close(s[0].cpu_cores, 0.5);
        assert_eq!(s[0].samples, 1);
    }

    #[tokio::test]
    async fn long_gaps_yield_no_rate() {
        let db = db!();
        let gb = 1_000_000_000;
        put(
            &db.pool,
            "p",
            "a",
            &[(0, 0, 1, 1, 0, 0), (3600, 100 * gb, 1, 1, 0, 0)],
        )
        .await;
        let s = series(&db.pool, "p", 0, 3601, 3601).await;
        assert_eq!(s[0].samples, 2);
        assert!(s[0].cpu_cores.is_none() && s[0].disk_read_bps.is_none());
    }

    #[tokio::test]
    async fn window_is_half_open() {
        let db = db!();
        put(
            &db.pool,
            "p",
            "a",
            &[
                (0, 0, 10, 1, 0, 0),
                (60, 0, 20, 1, 0, 0),
                (120, 0, 30, 1, 0, 0),
            ],
        )
        .await;
        let s = series(&db.pool, "p", 60, 120, 30).await;
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].samples, 1);
        assert_eq!(s[0].memory_bytes_max, Some(20));
        assert_eq!(s[1].samples, 0);
    }

    #[tokio::test]
    async fn bad_arguments_are_rejected() {
        let db = db!();
        let p = &db.pool;
        let bad = |from: i64, to: i64, b: u64| {
            let p = p.clone();
            async move {
                sandbox_series(&p, "p", at(from), at(to), Std::from_secs(b))
                    .await
                    .is_err()
            }
        };
        assert!(bad(0, 60, 0).await);
        assert!(bad(60, 60, 10).await);
        assert!(bad(60, 0, 10).await);
        assert!(bad(0, 1001, 1).await);
        assert!(!bad(0, 1000, 1).await);
        let sub_second = sqlx::query("SELECT * FROM workflows_api.sandbox_series('p', now() - interval '1 minute', now(), interval '500 ms')")
            .fetch_all(p)
            .await;
        assert!(sub_second.is_err());
    }

    #[tokio::test]
    async fn projects_are_isolated() {
        let db = db!();
        let gb = 1_000_000_000;
        put(
            &db.pool,
            "a",
            "sb",
            &[(0, 0, 100, 1, 0, 0), (10, 10 * gb, 100, 1, 0, 0)],
        )
        .await;
        put(
            &db.pool,
            "b",
            "sb",
            &[(0, 0, 7, 1, 0, 0), (10, gb, 7, 1, 0, 0)],
        )
        .await;
        let a = series(&db.pool, "a", 0, 20, 20).await;
        let b = series(&db.pool, "b", 0, 20, 20).await;
        assert_eq!(a[0].samples, 2);
        assert_eq!(b[0].samples, 2);
        assert_eq!(a[0].memory_bytes_max, Some(100));
        assert_eq!(b[0].memory_bytes_max, Some(7));
        close(a[0].cpu_cores, 1.0);
        close(b[0].cpu_cores, 0.1);
        let none = series(&db.pool, "c", 0, 20, 20).await;
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].samples, 0);
    }

    #[tokio::test]
    async fn function_grants_match_the_roles() {
        let db = db!();
        let can = |role: &'static str, f: &'static str| {
            let p = db.pool.clone();
            async move {
                sqlx::query_scalar::<_, bool>("SELECT has_function_privilege($1, $2, 'EXECUTE')")
                    .bind(role)
                    .bind(f)
                    .fetch_one(&p)
                    .await
                    .unwrap()
            }
        };
        let insert = "workflows_api.record_sandbox_sample(text, bigint, text, timestamptz, bigint, bigint, bigint, bigint, bigint)";
        let read = "workflows_api.sandbox_series(text, timestamptz, timestamptz, interval)";
        let prune = "workflows.prune_sandbox_samples(interval, integer)";
        assert!(can("enzarb_workflows", insert).await);
        assert!(!can("enzarb_ui", insert).await);
        assert!(!can("enzarb_connectors", insert).await);
        assert!(can("enzarb_ui", read).await);
        assert!(!can("enzarb_connectors", read).await);
        assert!(can("enzarb_workflows", prune).await);
        assert!(!can("enzarb_ui", prune).await);
    }
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
