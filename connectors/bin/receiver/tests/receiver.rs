//! Integration tests against a throwaway Postgres (testcontainers).
//! `workflows_api.ingest_event` is stubbed here; the real one is owned by
//! enzarb-workflows (same signature, returns NULL for a duplicate dedupe key).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use enzarb_connector_github::{BotIdentity, sign};
use enzarb_receiver::{AppState, db, router};
use sqlx::PgPool;
use std::sync::atomic::{AtomicU64, Ordering};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tower::ServiceExt;

const SECRET: &[u8] = b"hook-secret";

const STUB: &str = r#"
CREATE SCHEMA workflows;
CREATE TABLE workflows.events (
    id bigserial PRIMARY KEY,
    source text NOT NULL,
    dedupe_key text NOT NULL UNIQUE,
    work_item_key text NOT NULL,
    type text NOT NULL,
    payload jsonb NOT NULL
);
CREATE SCHEMA workflows_api;
CREATE FUNCTION workflows_api.ingest_event(
    p_source text, p_dedupe_key text, p_work_item_key text, p_type text, p_payload jsonb
) RETURNS bigint LANGUAGE plpgsql AS $$
DECLARE v_id bigint;
BEGIN
    INSERT INTO workflows.events (source, dedupe_key, work_item_key, type, payload)
    VALUES (p_source, p_dedupe_key, p_work_item_key, p_type, COALESCE(p_payload, '{}'::jsonb))
    ON CONFLICT (dedupe_key) DO NOTHING
    RETURNING id INTO v_id;
    RETURN v_id;
END $$;
"#;

/// Env var naming one shared Postgres server for the whole run (see docs/dev.md). When set, each
/// test gets its own empty database on it instead of its own container.
const PG_URL_ENV: &str = "ENZARB_TEST_PG_URL";

/// A Postgres to test against: a throwaway container, or a fresh database on the shared server
/// (dropped, best effort, on `Drop`).
struct Server {
    /// Admin URL of the server (`.../postgres`).
    admin: String,
    /// Name of the database created for this test (shared mode only).
    db: Option<String>,
    _c: Option<ContainerAsync<Postgres>>,
}

impl Server {
    async fn start() -> Server {
        if let Some(admin) = std::env::var(PG_URL_ENV).ok().filter(|u| !u.is_empty()) {
            static N: AtomicU64 = AtomicU64::new(0);
            let db = format!(
                "recv_{}_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                N.fetch_add(1, Ordering::Relaxed)
            );
            let pool = sqlx::PgPool::connect(&admin).await.unwrap();
            sqlx::query(&format!("CREATE DATABASE {db}"))
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
            return Server {
                admin,
                db: Some(db),
                _c: None,
            };
        }
        let c = Postgres::default()
            .with_tag("18")
            .start()
            .await
            .expect("start postgres (Docker required)");
        let host = c.get_host().await.unwrap();
        let port = c.get_host_port_ipv4(5432).await.unwrap();
        Server {
            admin: format!("postgres://postgres:postgres@{host}:{port}/postgres"),
            db: None,
            _c: Some(c),
        }
    }

    /// URL of `database` on this server, as `user:password` (superuser by default).
    fn url(&self, creds: &str, database: &str) -> String {
        let (scheme, rest) = self.admin.split_once("://").unwrap();
        let host = rest.rsplit_once('@').unwrap().1.split('/').next().unwrap();
        format!("{scheme}://{creds}@{host}/{database}")
    }

    /// The database a test should use: its own on a shared server, else `postgres`.
    fn default_db(&self) -> &str {
        self.db.as_deref().unwrap_or("postgres")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let Some(db) = self.db.take() else { return };
        let admin = self.admin.clone();
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                if let Ok(pool) = sqlx::PgPool::connect(&admin).await {
                    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)"))
                        .execute(&pool)
                        .await;
                    pool.close().await;
                }
            });
        })
        .join();
    }
}

struct Env {
    pool: PgPool,
    app: Router,
    _s: Server,
}

async fn setup() -> Env {
    let s = Server::start().await;
    let url = s.url("postgres:postgres", s.default_db());
    let pool = db::connect(&url, 5).await.unwrap();
    sqlx::raw_sql(STUB).execute(&pool).await.unwrap();
    db::migrate(&pool).await.unwrap();
    let app = router(AppState::new(
        pool.clone(),
        SECRET.to_vec(),
        BotIdentity::default(),
    ));
    Env { pool, app, _s: s }
}

/// Regression test for enzarb-connectors#15: on a pgop-provisioned database the schema exists, the
/// migration role owns it, and another role owns the database. `CREATE SCHEMA IF NOT EXISTS` then
/// fails with "permission denied for database"; migrations must succeed anyway, and twice in a row.
#[tokio::test]
async fn migrates_as_schema_owner_without_database_create() {
    let s = Server::start().await;
    let admin_pool = sqlx::PgPool::connect(&s.admin).await.unwrap();
    // Roles are cluster-global: tolerate them existing on a shared server (and racing creators).
    for role in [
        "wf_owner LOGIN PASSWORD 'x'",
        "c_owner LOGIN PASSWORD 'x'",
        "enzarb_connectors",
    ] {
        let _ = sqlx::query(&format!("CREATE ROLE {role}"))
            .execute(&admin_pool)
            .await;
    }
    // Shared server: the database already exists (unique name), so just hand it over.
    let database = match s.db.as_deref() {
        Some(db) => {
            sqlx::query(&format!("ALTER DATABASE {db} OWNER TO wf_owner"))
                .execute(&admin_pool)
                .await
                .unwrap();
            db
        }
        None => {
            sqlx::query("CREATE DATABASE enzarb OWNER wf_owner")
                .execute(&admin_pool)
                .await
                .unwrap();
            "enzarb"
        }
    };
    let in_db = sqlx::PgPool::connect(&s.url("postgres:postgres", database))
        .await
        .unwrap();
    sqlx::query("CREATE SCHEMA connectors AUTHORIZATION c_owner")
        .execute(&in_db)
        .await
        .unwrap();

    let pool = db::connect(&s.url("c_owner:x", database), 2).await.unwrap();
    db::migrate(&pool)
        .await
        .expect("first migrate as schema owner");
    db::migrate(&pool).await.expect("second migrate is a no-op");
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connectors.installations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

fn fixture(name: &str) -> Vec<u8> {
    let p = format!(
        "{}/../../crates/connector-github/tests/fixtures/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(p).unwrap()
}

async fn post(
    app: &Router,
    event: &str,
    delivery: &str,
    body: &[u8],
    sig: Option<&str>,
) -> StatusCode {
    let mut req = Request::post("/hooks/github")
        .header("x-github-event", event)
        .header("x-github-delivery", delivery);
    if let Some(s) = sig {
        req = req.header("x-hub-signature-256", s);
    }
    app.clone()
        .oneshot(req.body(Body::from(body.to_vec())).unwrap())
        .await
        .unwrap()
        .status()
}

async fn post_signed(app: &Router, event: &str, delivery: &str, body: &[u8]) -> StatusCode {
    post(app, event, delivery, body, Some(&sign(SECRET, body))).await
}

async fn get(app: &Router, path: &str) -> StatusCode {
    app.clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

async fn count_events(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM workflows.events")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn migrations_history_is_schema_scoped_and_idempotent() {
    let env = setup().await;
    db::migrate(&env.pool).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connectors._sqlx_migrations")
        .fetch_one(&env.pool)
        .await
        .unwrap();
    assert_eq!(n, db::MIGRATOR.migrations.len() as i64);
    let public: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('public._sqlx_migrations')::text")
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(public, None);
}

#[tokio::test]
async fn health_and_ready() {
    let env = setup().await;
    assert_eq!(get(&env.app, "/healthz").await, StatusCode::OK);
    assert_eq!(get(&env.app, "/readyz").await, StatusCode::OK);
    env.pool.close().await;
    assert_eq!(
        get(&env.app, "/readyz").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(get(&env.app, "/healthz").await, StatusCode::OK);
}

#[tokio::test]
async fn signatures_are_enforced() {
    let env = setup().await;
    let body = fixture("issues_opened");
    assert_eq!(
        post(&env.app, "issues", "d1", &body, None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&env.app, "issues", "d1", &body, Some("sha256=deadbeef")).await,
        StatusCode::UNAUTHORIZED
    );
    let wrong = sign(b"other", &body);
    assert_eq!(
        post(&env.app, "issues", "d1", &body, Some(&wrong)).await,
        StatusCode::UNAUTHORIZED
    );
    // Signature over a different body.
    let other = sign(SECRET, b"{}");
    assert_eq!(
        post(&env.app, "issues", "d1", &body, Some(&other)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(count_events(&env.pool).await, 0);
}

#[tokio::test]
async fn bad_requests() {
    let env = setup().await;
    let body = fixture("issues_opened");
    // Missing delivery header.
    let req = Request::post("/hooks/github")
        .header("x-github-event", "issues")
        .header("x-hub-signature-256", sign(SECRET, &body))
        .body(Body::from(body.clone()))
        .unwrap();
    assert_eq!(
        env.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    // Signed but malformed JSON.
    assert_eq!(
        post_signed(&env.app, "issues", "d2", b"not json").await,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn ingests_events_dedupes_and_lowercases() {
    let env = setup().await;

    // Unmapped / irrelevant deliveries: 204, nothing stored.
    assert_eq!(
        post_signed(&env.app, "ping", "p1", &fixture("ping")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_signed(&env.app, "issues", "e1", &fixture("issues_edited")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        post_signed(
            &env.app,
            "issue_comment",
            "b1",
            &fixture("issue_comment_by_bot")
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(count_events(&env.pool).await, 0);

    // One delivery, three events (comment + mention + approval).
    let body = fixture("issue_comment_mention_approve_on_pr");
    assert_eq!(
        post_signed(&env.app, "issue_comment", "dlv-1", &body).await,
        StatusCode::ACCEPTED
    );
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT source, dedupe_key, work_item_key, type FROM workflows.events ORDER BY id",
    )
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "github".into(),
                "github:dlv-1".into(),
                "github:enzarb/enzarb-ui#57".into(),
                "CommentAdded".into()
            ),
            (
                "github".into(),
                "github:dlv-1#mentioned".into(),
                "github:enzarb/enzarb-ui#57".into(),
                "Mentioned".into()
            ),
            (
                "github".into(),
                "github:dlv-1#approved".into(),
                "github:enzarb/enzarb-ui#57".into(),
                "ApprovalGranted".into()
            ),
        ]
    );
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM workflows.events WHERE dedupe_key = 'github:dlv-1'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(payload["subject"], "github:enzarb/enzarb-ui#57");
    assert_eq!(payload["type"], "CommentAdded");
    assert_eq!(payload["id"], "dlv-1");

    // Redelivery is a no-op but still acknowledged.
    assert_eq!(
        post_signed(&env.app, "issue_comment", "dlv-1", &body).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(count_events(&env.pool).await, 3);

    // Mixed-case repo names are lowercased in key, subject and payload.
    let mixed = String::from_utf8(fixture("issues_opened"))
        .unwrap()
        .replace("enzarb/enzarb-ui", "EnZarb/Enzarb-UI")
        .into_bytes();
    assert_eq!(
        post_signed(&env.app, "issues", "dlv-2", &mixed).await,
        StatusCode::ACCEPTED
    );
    let (key, payload): (String, serde_json::Value) = sqlx::query_as(
        "SELECT work_item_key, payload FROM workflows.events WHERE dedupe_key = 'github:dlv-2'",
    )
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert!(key.starts_with("github:enzarb/enzarb-ui#"), "{key}");
    assert_eq!(payload["subject"], key);
    assert_eq!(payload["data"]["repo"], "github:enzarb/enzarb-ui");
}

#[tokio::test]
async fn database_failure_returns_500() {
    let env = setup().await;
    sqlx::query("DROP FUNCTION workflows_api.ingest_event(text, text, text, text, jsonb)")
        .execute(&env.pool)
        .await
        .unwrap();
    assert_eq!(
        post_signed(&env.app, "issues", "x1", &fixture("issues_opened")).await,
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

async fn installs(pool: &PgPool) -> Vec<(i64, String, serde_json::Value)> {
    sqlx::query_as(
        "SELECT installation_id, account, repos FROM connectors.installations ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn installation_upserts() {
    let env = setup().await;

    let created = fixture("installation_created");
    assert_eq!(
        post_signed(&env.app, "installation", "i1", &created).await,
        StatusCode::ACCEPTED
    );
    let rows = installs(&env.pool).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 55001122);
    assert_eq!(rows[0].1, "enzarb");
    assert_eq!(
        rows[0].2,
        serde_json::json!(["github:enzarb/enzarb-ui", "github:enzarb/enzarb-workflows"])
    );
    // Redelivery is idempotent.
    assert_eq!(
        post_signed(&env.app, "installation", "i1", &created).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(installs(&env.pool).await.len(), 1);
    // Installation events never reach the workflow engine.
    assert_eq!(count_events(&env.pool).await, 0);

    // Repositories added then removed, per the fixtures.
    let added = fixture("installation_repositories_added");
    assert_eq!(
        post_signed(&env.app, "installation_repositories", "i2", &added).await,
        StatusCode::ACCEPTED
    );
    let after_add = installs(&env.pool).await;
    assert!(after_add[0].2.as_array().unwrap().len() >= 2);
    let removed = fixture("installation_repositories_removed");
    assert_eq!(
        post_signed(&env.app, "installation_repositories", "i3", &removed).await,
        StatusCode::ACCEPTED
    );
    let after_rm = installs(&env.pool).await;
    assert_eq!(after_rm.len(), 1);
    for r in removed_repos(&removed) {
        assert!(
            !after_rm[0]
                .2
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v == r.as_str()),
            "{r} still present"
        );
    }

    // Deleting the installation removes the row.
    let deleted = fixture("installation_deleted");
    assert_eq!(
        post_signed(&env.app, "installation", "i4", &deleted).await,
        StatusCode::ACCEPTED
    );
    assert!(installs(&env.pool).await.is_empty());
}

fn removed_repos(body: &[u8]) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(body).unwrap();
    v["repositories_removed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| format!("github:{}", r["full_name"].as_str().unwrap().to_lowercase()))
        .collect()
}
