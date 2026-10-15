//! The shared-server mode of `TestDb` (`ENZARB_TEST_PG_URL`). Skipped when it is not set; the
//! per-test container fallback is exercised by every other integration test.

use engine::testing::{PG_URL_ENV, TEMPLATE_DB, TestDb, migrations_checksum};
use sqlx::postgres::PgPoolOptions;

fn admin_url() -> Option<String> {
    let url = std::env::var(PG_URL_ENV).ok().filter(|u| !u.is_empty());
    if url.is_none() {
        eprintln!("SKIP: {PG_URL_ENV} not set; shared-server test skipped");
    }
    url
}

async fn current_db(db: &TestDb) -> String {
    sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn databases_are_isolated_and_pool_as_uses_the_test_database() {
    if admin_url().is_none() {
        return;
    }
    let a = TestDb::start().await.unwrap();
    let b = TestDb::start().await.unwrap();
    let (na, nb) = (current_db(&a).await, current_db(&b).await);
    assert_ne!(na, nb);
    assert!(na.starts_with("test_"), "{na}");

    sqlx::query("INSERT INTO workflows.work_items (key, workflow, state) VALUES ('k', 'w', 's')")
        .execute(&a.pool)
        .await
        .unwrap();
    let count = |db: &TestDb| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM workflows.work_items")
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(count(&a).await, 1);
    assert_eq!(count(&b).await, 0);

    let as_role = a.pool_as("enzarb_workflows").await;
    let (who, dbname): (String, String) = sqlx::query_as("SELECT current_user, current_database()")
        .fetch_one(&as_role)
        .await
        .unwrap();
    assert_eq!(
        (who.as_str(), dbname.as_str()),
        ("enzarb_workflows", na.as_str())
    );
}

#[tokio::test]
async fn dropping_a_testdb_drops_its_database() {
    let Some(admin) = admin_url() else { return };
    let db = TestDb::start().await.unwrap();
    let name = current_db(&db).await;
    drop(db);
    let admin = PgPoolOptions::new().connect(&admin).await.unwrap();
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&name)
            .fetch_one(&admin)
            .await
            .unwrap();
    assert!(!exists, "{name} should have been dropped");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_starts_do_not_race() {
    if admin_url().is_none() {
        return;
    }
    // join_all rather than tokio::spawn: the sqlx futures hit rustc's higher-ranked `Send`
    // limitation when spawned. Each start() still uses its own connection, so the advisory lock
    // is contended exactly as between processes.
    let dbs =
        futures::future::join_all((0..8).map(|_| async { TestDb::start().await.unwrap() })).await;
    let mut names = Vec::new();
    for db in &dbs {
        names.push(current_db(db).await);
    }
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 8);
}

#[tokio::test]
async fn stale_template_checksum_triggers_a_rebuild() {
    let Some(admin) = admin_url() else { return };
    // Make sure a template exists, then corrupt its marker.
    drop(TestDb::start().await.unwrap());
    let pool = PgPoolOptions::new().connect(&admin).await.unwrap();
    sqlx::query(&format!("COMMENT ON DATABASE {TEMPLATE_DB} IS 'stale'"))
        .execute(&pool)
        .await
        .unwrap();
    let oid = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, sqlx::postgres::types::Oid>(
            "SELECT oid FROM pg_database WHERE datname = $1",
        )
        .bind(TEMPLATE_DB)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let before = oid(pool.clone()).await;

    let db = TestDb::start().await.unwrap();
    let after = oid(pool.clone()).await;
    assert_ne!(before, after, "template should have been recreated");
    let marker: Option<String> = sqlx::query_scalar(
        "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1",
    )
    .bind(TEMPLATE_DB)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(marker.as_deref(), Some(migrations_checksum().as_str()));
    // And the clone is migrated.
    let migrated: bool =
        sqlx::query_scalar("SELECT to_regclass('workflows.work_items') IS NOT NULL")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(migrated);
}
