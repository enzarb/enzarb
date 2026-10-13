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

async fn insert_items(pool: &PgPool, items: &[(&str, Option<&str>)]) {
    for (key, project) in items {
        sqlx::query(
            "INSERT INTO workflows.work_items (key, project_id, workflow, state) VALUES ($1, $2, 'w', 's')",
        )
        .bind(key)
        .bind(project)
        .execute(pool)
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn realtime_role_sees_only_work_item_project_view() {
    let Some((pool, _c)) = bare_db().await else {
        return;
    };
    sqlx::query("CREATE ROLE enzarb_realtime LOGIN")
        .execute(&pool)
        .await
        .unwrap();
    migrate(&pool, false).await.unwrap();
    insert_items(&pool, &[("a", Some("p1")), ("b", Some("p2")), ("c", None)]).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET ROLE enzarb_realtime")
        .execute(&mut *conn)
        .await
        .unwrap();
    let rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, project FROM workflows_api.work_item_project_v ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(rows.len(), 3);
    for denied in [
        "SELECT * FROM workflows_api.work_item_v",
        "SELECT * FROM workflows_api.job_v",
        "SELECT * FROM workflows.work_items",
    ] {
        let err = sqlx::query(denied).fetch_all(&mut *conn).await.unwrap_err();
        assert!(
            err.to_string().contains("permission denied"),
            "{denied}: {err}"
        );
    }
    let can_connect: bool = sqlx::query_scalar(
        "SELECT has_database_privilege('enzarb_realtime', current_database(), 'CONNECT')",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert!(can_connect);
}

#[tokio::test]
async fn work_item_project_view_matches_work_item_view() {
    let Some((pool, _c)) = bare_db().await else {
        return;
    };
    migrate(&pool, false).await.unwrap();
    insert_items(
        &pool,
        &[
            ("a", Some("p1")),
            ("b", Some("p2")),
            ("c", Some("p1")),
            ("d", None),
        ],
    )
    .await;
    let narrow: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, project FROM workflows_api.work_item_project_v ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    let full: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, project FROM workflows_api.work_item_v ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(narrow.len(), 4);
    assert_eq!(narrow, full);
}
