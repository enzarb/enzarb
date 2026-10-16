use engine::testing::TestDb;
use enzarb_workflows::migrate::{MigrateError, migrate};
use sqlx::PgPool;

/// An unmigrated database (an empty database on the shared server when `ENZARB_TEST_PG_URL` is
/// set, so the cluster-global runtime roles may already exist). Keep the guard alive.
async fn bare_db() -> Option<(PgPool, TestDb)> {
    let db = TestDb::start_bare().await?;
    Some((db.pool.clone(), db))
}

/// Like `bare_db` but on a dedicated container: no roles exist yet.
async fn roleless_db() -> Option<(PgPool, TestDb)> {
    let db = TestDb::start_bare_container().await?;
    Some((db.pool.clone(), db))
}

async fn migrated(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass('engine.tasks') IS NOT NULL")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn migrate_without_roles_applies_and_is_idempotent() {
    let Some((pool, _c)) = bare_db().await else {
        return;
    };
    migrate(&pool, false).await.unwrap();
    assert!(migrated(&pool).await);
    migrate(&pool, false).await.unwrap();
}

#[tokio::test]
async fn require_roles_fails_when_missing_then_passes() {
    let Some((pool, _c)) = roleless_db().await else {
        return;
    };
    let err = migrate(&pool, true).await.unwrap_err();
    assert!(matches!(err, MigrateError::MissingRoles(_)), "{err}");
    let msg = err.to_string();
    for r in ["enzarb_connectors", "enzarb_ui", "enzarb_workflows"] {
        assert!(msg.contains(r), "{msg}");
    }
    assert!(!migrated(&pool).await, "nothing applied on failure");

    // Only some roles present: still fails, naming the rest.
    sqlx::query("CREATE ROLE enzarb_ui")
        .execute(&pool)
        .await
        .unwrap();
    let msg = migrate(&pool, true).await.unwrap_err().to_string();
    assert!(
        !msg.contains("enzarb_ui") && msg.contains("enzarb_connectors"),
        "{msg}"
    );

    for r in ["enzarb_connectors", "enzarb_workflows"] {
        sqlx::query(&format!("CREATE ROLE {r}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    migrate(&pool, true).await.unwrap();
    assert!(migrated(&pool).await);
}

#[tokio::test]
async fn ui_role_reads_engine_api_but_not_engine_tables() {
    let Some(db) = TestDb::start_container().await else {
        return;
    };
    migrate(&db.pool, false).await.unwrap();
    let ui = db.pool_as("enzarb_ui").await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM engine_api.task_v")
        .fetch_one(&ui)
        .await
        .unwrap();
    assert_eq!(n, 0);
    assert!(
        sqlx::query("SELECT * FROM engine.tasks")
            .execute(&ui)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT engine_api.ingest('x', NULL, NULL, 'issue', '{}')")
            .execute(&ui)
            .await
            .is_err(),
        "only the receiver ingests"
    );
}
