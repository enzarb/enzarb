use enzarb_workflows::migrate::{MigrateError, migrate};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

async fn bare_db() -> Option<(PgPool, impl Sized)> {
    let c = match Postgres::default().with_tag("18").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP: Docker unavailable ({e})");
            return None;
        }
    };
    let host = c.get_host().await.unwrap();
    let port = c.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    Some((engine::db::connect(&url, 2).await.unwrap(), c))
}

async fn migrated(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT to_regclass('workflows.work_items') IS NOT NULL")
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
    let Some((pool, _c)) = bare_db().await else {
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
