//! `NOTIFY work_item_changed` is sent by DB triggers (docs/contracts.md §6).

use engine::testing::TestDb;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use std::time::Duration;

macro_rules! db {
    () => {
        match TestDb::start().await {
            Some(db) => db,
            None => return,
        }
    };
}

async fn listener(db: &TestDb) -> PgListener {
    let mut l = PgListener::connect(&db.url).await.unwrap();
    l.listen("work_item_changed").await.unwrap();
    l
}

/// Payloads received until the channel is quiet for a short while.
async fn drain(l: &mut PgListener) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(n) = tokio::time::timeout(Duration::from_millis(500), l.recv()).await {
        out.push(n.unwrap().payload().to_string());
    }
    out
}

async fn work_item(pool: &PgPool, key: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO workflows.work_items (key, workflow, state) VALUES ($1, 'issue', 'Triaging')
         RETURNING id",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn job(pool: &PgPool, wi: i64, key: &str) {
    sqlx::query(
        "INSERT INTO workflows.jobs (work_item_id, kind, idempotency_key) VALUES ($1, 'k', $2)",
    )
    .bind(wi)
    .bind(key)
    .execute(pool)
    .await
    .unwrap();
}

async fn exec(pool: &PgPool, sql: &str) {
    sqlx::query(sql).execute(pool).await.unwrap();
}

#[tokio::test]
async fn job_changes_notify_once_with_work_item_id() {
    let db = db!();
    let wi = work_item(&db.pool, "k1").await;
    let mut l = listener(&db).await;
    let want = vec![wi.to_string()];

    job(&db.pool, wi, "j1").await;
    assert_eq!(drain(&mut l).await, want, "insert");

    let steps = [
        "status = 'running', attempt = attempt + 1, lease_owner = 'w', lease_until = now()",
        "status = 'queued', run_after = now() + interval '1 minute'",
        "status = 'succeeded'",
        "status = 'failed'",
        "status = 'cancelled'",
        "output = '{\"a\": 1}'::jsonb",
    ];
    for set in steps {
        exec(
            &db.pool,
            &format!("UPDATE workflows.jobs SET {set} WHERE idempotency_key = 'j1'"),
        )
        .await;
        assert_eq!(drain(&mut l).await, want, "{set}");
    }
}

#[tokio::test]
async fn lease_and_deadline_only_updates_do_not_notify() {
    let db = db!();
    let wi = work_item(&db.pool, "k2").await;
    job(&db.pool, wi, "j1").await;
    let mut l = listener(&db).await;
    exec(
        &db.pool,
        "UPDATE workflows.jobs SET lease_owner = 'w', lease_until = now(), deadline = now(),
         updated_at = now()",
    )
    .await;
    assert!(drain(&mut l).await.is_empty());
}

#[tokio::test]
async fn work_item_and_transition_changes_notify() {
    let db = db!();
    let mut l = listener(&db).await;
    let wi = work_item(&db.pool, "k3").await;
    assert_eq!(drain(&mut l).await, vec![wi.to_string()]);
    exec(
        &db.pool,
        "UPDATE workflows.work_items SET state = 'Planning'",
    )
    .await;
    assert_eq!(drain(&mut l).await, vec![wi.to_string()]);
    exec(&db.pool, "UPDATE workflows.work_items SET wake_at = now()").await;
    assert!(drain(&mut l).await.is_empty(), "wake_at is not shown");
    exec(
        &db.pool,
        "INSERT INTO workflows.transitions (work_item_id, to_state, event_type)
         SELECT id, 'Planning', 'X' FROM workflows.work_items",
    )
    .await;
    assert_eq!(drain(&mut l).await, vec![wi.to_string()]);
}

#[tokio::test]
async fn transition_without_work_item_does_not_notify() {
    let db = db!();
    let mut l = listener(&db).await;
    exec(
        &db.pool,
        "INSERT INTO workflows.transitions (work_item_id, to_state, event_type)
         VALUES (NULL, 'Planning', 'X')",
    )
    .await;
    assert!(drain(&mut l).await.is_empty());
}

#[tokio::test]
async fn rollback_sends_nothing_and_transaction_dedupes() {
    let db = db!();
    let wi = work_item(&db.pool, "k4").await;
    job(&db.pool, wi, "j1").await;
    job(&db.pool, wi, "j2").await;
    let mut l = listener(&db).await;

    let mut tx = db.pool.begin().await.unwrap();
    sqlx::query("UPDATE workflows.jobs SET status = 'failed'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(drain(&mut l).await.is_empty());

    let mut tx = db.pool.begin().await.unwrap();
    for sql in [
        "UPDATE workflows.jobs SET status = 'running'",
        "UPDATE workflows.jobs SET status = 'succeeded'",
        "INSERT INTO workflows.transitions (work_item_id, to_state, event_type)
         SELECT id, 'Done', 'X' FROM workflows.work_items",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(drain(&mut l).await, vec![wi.to_string()]);
}

#[tokio::test]
async fn notify_migration_is_safe_to_rerun() {
    let db = db!();
    let sql = include_str!("../../../migrations/20261011000001_notify_work_item_changed.sql");
    sqlx::raw_sql(sql).execute(&db.pool).await.unwrap();
    let wi = work_item(&db.pool, "k5").await;
    let mut l = listener(&db).await;
    job(&db.pool, wi, "j1").await;
    assert_eq!(drain(&mut l).await, vec![wi.to_string()]);
}

#[tokio::test]
async fn runtime_role_can_write_jobs_with_triggers() {
    let db = db!();
    let wi = work_item(&db.pool, "k6").await;
    let pool = db.pool_as("enzarb_workflows").await;
    let mut l = listener(&db).await;
    job(&pool, wi, "j1").await;
    assert_eq!(drain(&mut l).await, vec![wi.to_string()]);
}
