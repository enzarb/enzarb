//! `enzarb_realtime` grants reconciler.

use std::time::Duration;

use engine::Ticker;
use engine::grants::reconcile_realtime;
use engine::testing::TestDb;
use sqlx::PgPool;
use tokio::sync::watch;

// `enzarb_realtime` is a cluster-global role that these tests create and expect absent, so they
// need their own server rather than the shared one.
macro_rules! db {
    () => {
        match TestDb::start_container().await {
            Some(db) => db,
            None => return,
        }
    };
}

async fn create_role(pool: &PgPool) {
    sqlx::query("CREATE ROLE enzarb_realtime LOGIN PASSWORD 'enzarb_realtime'")
        .execute(pool)
        .await
        .unwrap();
}

async fn check(pool: &PgPool, sql: &str) -> bool {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn has_all(pool: &PgPool) -> bool {
    check(
        pool,
        "SELECT has_database_privilege('enzarb_realtime', current_database(), 'CONNECT')
         AND has_schema_privilege('enzarb_realtime', 'workflows_api', 'USAGE')
         AND has_table_privilege('enzarb_realtime', 'workflows_api.work_item_project_v', 'SELECT')",
    )
    .await
}

async fn acls(pool: &PgPool) -> String {
    sqlx::query_scalar(
        "SELECT concat_ws('|',
            (SELECT datacl::text FROM pg_database WHERE datname = current_database()),
            (SELECT nspacl::text FROM pg_namespace WHERE nspname = 'workflows_api'),
            (SELECT relacl::text FROM pg_class WHERE oid = 'workflows_api.work_item_project_v'::regclass))",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn role_absent_is_a_noop() {
    let db = db!();
    assert!(!reconcile_realtime(&db.pool).await.unwrap());
    assert!(
        !check(
            &db.pool,
            "SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_realtime')"
        )
        .await
    );
}

#[tokio::test]
async fn role_created_later_gets_exactly_the_grants() {
    let db = db!();
    create_role(&db.pool).await;
    assert!(!has_all(&db.pool).await);
    assert!(reconcile_realtime(&db.pool).await.unwrap());
    assert!(has_all(&db.pool).await);
    for rel in [
        "workflows.work_items",
        "workflows_api.work_item_v",
        "workflows_api.job_v",
    ] {
        let sql = format!("SELECT has_table_privilege('enzarb_realtime', '{rel}', 'SELECT')");
        assert!(!check(&db.pool, &sql).await, "{rel}");
    }
}

#[tokio::test]
async fn second_pass_changes_nothing() {
    let db = db!();
    create_role(&db.pool).await;
    assert!(reconcile_realtime(&db.pool).await.unwrap());
    let before = acls(&db.pool).await;
    assert!(!reconcile_realtime(&db.pool).await.unwrap());
    assert_eq!(acls(&db.pool).await, before);
}

#[tokio::test]
async fn runtime_role_can_reconcile_and_realtime_cannot_call_it() {
    let db = db!();
    create_role(&db.pool).await;
    let runtime = db.pool_as("enzarb_workflows").await;
    assert!(reconcile_realtime(&runtime).await.unwrap());
    assert!(has_all(&db.pool).await);
    assert!(!reconcile_realtime(&runtime).await.unwrap());

    let realtime = db.pool_as("enzarb_realtime").await;
    assert!(reconcile_realtime(&realtime).await.is_err());
}

#[tokio::test]
async fn ticker_applies_grants_when_role_appears() {
    let db = db!();
    let runtime = db.pool_as("enzarb_workflows").await;
    let (tx, rx) = watch::channel(false);
    let task = tokio::spawn(
        Ticker::new(runtime)
            .with_interval(Duration::from_millis(100))
            .run(rx),
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    create_role(&db.pool).await;
    let mut ok = false;
    for _ in 0..30 {
        if has_all(&db.pool).await {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tx.send(true).unwrap();
    task.await.unwrap();
    assert!(ok, "ticker did not grant within 3s");
}
