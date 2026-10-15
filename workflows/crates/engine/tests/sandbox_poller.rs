use engine::sandbox_poller::{PgPollLock, PgSampleSink, PollLock, SampleSink};
use engine::testing::TestDb;
use enzarb_proto::SandboxStats;

macro_rules! db {
    () => {
        match TestDb::start().await {
            Some(db) => db,
            None => return,
        }
    };
}

async fn work_item(pool: &sqlx::PgPool, key: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflows.work_items (key, project_id, workflow, state)
         VALUES ($1, 'p', 'issue', 'Triaging') RETURNING id",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn job(pool: &sqlx::PgPool, wi: i64, project: &str, kind: &str, status: &str) {
    sqlx::query(
        "INSERT INTO workflows.jobs (work_item_id, project_id, kind, status, idempotency_key)
         VALUES ($1, $2, $3, $4, gen_random_uuid()::text)",
    )
    .bind(wi)
    .bind(project)
    .bind(kind)
    .bind(status)
    .execute(pool)
    .await
    .unwrap();
}

fn frame(ts_ms: u64) -> SandboxStats {
    SandboxStats {
        ts_ms,
        memory_bytes: Some(7),
        ..Default::default()
    }
}

#[tokio::test]
async fn samples_are_attributed_to_the_lowest_work_item_with_a_running_agent_job() {
    let db = db!();
    let sink = PgSampleSink::new(db.pool.clone());
    let (a, b, c) = (
        work_item(&db.pool, "github:o/r#1").await,
        work_item(&db.pool, "github:o/r#2").await,
        work_item(&db.pool, "github:o/r#3").await,
    );
    // Nothing runs: NULL.
    assert!(sink.store("p", "sb", &frame(1_000)).await.unwrap());
    // Not agent jobs, not running, or another Project: ignored.
    job(&db.pool, a, "p", "set_progress", "running").await;
    job(&db.pool, a, "p", "agent_implement", "queued").await;
    job(&db.pool, a, "other", "agent_implement", "running").await;
    assert!(sink.store("p", "sb", &frame(2_000)).await.unwrap());
    job(&db.pool, c, "p", "agent_implement", "running").await;
    job(&db.pool, b, "p", "agent_review", "running").await;
    assert!(sink.store("p", "sb", &frame(3_000)).await.unwrap());
    // A retried insert is a no-op.
    assert!(!sink.store("p", "sb", &frame(3_000)).await.unwrap());

    let rows: Vec<(Option<i64>,)> =
        sqlx::query_as("SELECT work_item_id FROM workflows.sandbox_stats ORDER BY at")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    assert_eq!(rows, [(None,), (None,), (Some(b),)]);
}

#[tokio::test]
async fn prune_removes_rows_past_retention() {
    let db = db!();
    let sink = PgSampleSink::new(db.pool.clone());
    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    let old_ms = now_ms - 40 * 86_400_000;
    sink.store("p", "sb", &frame(old_ms)).await.unwrap();
    sink.store("p", "sb", &frame(now_ms)).await.unwrap();
    assert_eq!(sink.prune(30).await.unwrap(), 1);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM workflows.sandbox_stats")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 1);
}

#[tokio::test]
async fn only_one_poller_holds_the_lock_and_it_moves_on_release() {
    let db = db!();
    let first = PgPollLock::new(db.pool.clone());
    let second = PgPollLock::new(db.pool.clone());
    assert!(first.try_acquire().await.unwrap());
    // Holding it again is cheap and still true; the other replica is shut out.
    assert!(first.try_acquire().await.unwrap());
    assert!(!second.try_acquire().await.unwrap());
    assert!(!second.try_acquire().await.unwrap());
    // The holder goes away (its connection closes): the other takes over.
    drop(first);
    let mut taken = false;
    for _ in 0..50 {
        if second.try_acquire().await.unwrap() {
            taken = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(taken, "the lock was never released");
}
