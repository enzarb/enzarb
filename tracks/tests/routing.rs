//! Routing against a real Postgres (testcontainers): decisions, pending runs, at-most-once,
//! catch-up past the floor, and `/v1/decisions`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use enzarb_tracks::github::{RepoSource, Snapshot, SourceError};
use enzarb_tracks::projects::{ProjectLookup, ProjectRef};
use enzarb_tracks::router::Router;
use enzarb_tracks::{api, db};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tokio::sync::Semaphore;
use tower::ServiceExt;

const WORKFLOWS: &str = r#"
version = 1
[workflows.fix]
on = ["issues.labeled"]
if = "event.label.name == 'ez'"
start = "build"
[workflows.fix.nodes.build]
type = "command"
task = "build"
next = "done"
[workflows.fix.nodes.done]
type = "end"
status = "done"
"#;

struct Projects;
#[async_trait]
impl ProjectLookup for Projects {
    async fn owner(&self, repo: &str) -> Result<Option<ProjectRef>, String> {
        Ok((repo == "o/r").then(|| ProjectRef {
            name: "p".into(),
            default_branch: "main".into(),
        }))
    }
}

struct Source;
#[async_trait]
impl RepoSource for Source {
    async fn snapshot(&self, _: &str, _: &str) -> Result<Snapshot, SourceError> {
        Ok(Snapshot {
            sha: "abc1234".into(),
            workflows: Some(WORKFLOWS.into()),
            modes: None,
        })
    }
}

async fn setup() -> (impl Sized, PgPool, Arc<Router>) {
    let node = Postgres::default().with_tag("18").start().await.unwrap();
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 4).await.unwrap();
    // The connectors schema as the receiver creates it (only what routing reads).
    for q in [
        "CREATE SCHEMA connectors",
        "CREATE TABLE connectors.events (id bigserial PRIMARY KEY, source text NOT NULL DEFAULT 'github',
           delivery_id text NOT NULL, event text NOT NULL, action text, repo text, sender text,
           payload jsonb NOT NULL, received_at timestamptz NOT NULL DEFAULT now())",
        "CREATE VIEW connectors.event_v AS SELECT id, source, delivery_id, event, action, repo,
           sender, payload, received_at FROM connectors.events",
    ] {
        sqlx::query(q).execute(&pool).await.unwrap();
    }
    db::migrate(&pool).await.unwrap();
    let router = Arc::new(Router::new(
        pool.clone(),
        Arc::new(Source),
        Arc::new(Projects),
        2,
        Duration::from_secs(1),
    ));
    (node, pool, router)
}

async fn insert(
    pool: &PgPool,
    repo: Option<&str>,
    event: &str,
    action: &str,
    label: &str,
    issue: u32,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO connectors.events (delivery_id, event, action, repo, payload)
         VALUES (gen_random_uuid()::text, $1, $2, $3, $4) RETURNING id",
    )
    .bind(event)
    .bind(action)
    .bind(repo)
    .bind(json!({"action": action, "label": {"name": label},
        "issue": {"number": issue, "title": "T", "user": {"login": "a"}, "html_url": "u"}}))
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn decision(pool: &PgPool, id: i64) -> (String, Value, Value, Value, Option<String>) {
    sqlx::query_as::<_, (String, Value, Value, Value, Option<String>)>(
        "SELECT outcome, matched, reasons, errors, project FROM tracks.event_decisions WHERE event_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn drain(r: &Arc<Router>) {
    let sem = Arc::new(Semaphore::new(2));
    r.sweep_once(&sem).await.unwrap();
    // Wait for the spawned tasks.
    let _ = sem.acquire_many(2).await.unwrap();
}

#[tokio::test]
async fn routes_events_and_records_decisions() {
    let (_node, pool, router) = setup().await;
    // History from before tracks existed is never routed.
    let old = insert(&pool, Some("o/r"), "issues", "labeled", "ez", 1).await;
    router.init_floor().await.unwrap();

    let hit = insert(&pool, Some("o/r"), "issues", "labeled", "ez", 2).await;
    let miss = insert(&pool, Some("o/r"), "issues", "labeled", "bug", 3).await;
    let nobody = insert(&pool, Some("x/y"), "issues", "labeled", "ez", 4).await;
    let no_repo = insert(&pool, None, "ping", "", "", 5).await;
    drain(&router).await;

    assert!(
        sqlx::query("SELECT 1 FROM tracks.event_decisions WHERE event_id = $1")
            .bind(old)
            .fetch_optional(&pool)
            .await
            .unwrap()
            .is_none()
    );

    let (outcome, matched, reasons, _, project) = decision(&pool, hit).await;
    assert_eq!(outcome, "started");
    assert_eq!(matched, json!(["fix"]));
    assert_eq!(project.as_deref(), Some("p"));
    assert_eq!(reasons[0]["workflow"], "fix");
    let (status, node, sha): (String, Option<String>, String) =
        sqlx::query_as("SELECT status, current_node, sha FROM tracks.runs WHERE event_id = $1")
            .bind(hit)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (status.as_str(), node.as_deref(), sha.as_str()),
        ("pending", Some("build"), "abc1234")
    );

    let (outcome, matched, reasons, ..) = decision(&pool, miss).await;
    assert_eq!((outcome.as_str(), matched), ("no_match", json!([])));
    assert!(reasons[0]["reason"].as_str().unwrap().contains("is false"));

    let (outcome, _, _, errors, project) = decision(&pool, nobody).await;
    assert_eq!(outcome, "error");
    assert!(errors[0].as_str().unwrap().contains("no Project owns x/y"));
    assert!(project.is_none());

    assert_eq!(decision(&pool, no_repo).await.0, "skipped");

    // At most once: a second pass changes nothing and creates no second run.
    assert!(router.undecided(10).await.unwrap().is_empty());
    router.process(hit).await;
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks.runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);
}

#[tokio::test]
async fn a_subject_with_a_live_run_does_not_start_another() {
    let (_node, pool, router) = setup().await;
    router.init_floor().await.unwrap();
    let first = insert(&pool, Some("o/r"), "issues", "labeled", "ez", 9).await;
    drain(&router).await;
    let again = insert(&pool, Some("o/r"), "issues", "labeled", "ez", 9).await;
    drain(&router).await;
    assert_eq!(decision(&pool, first).await.0, "started");
    let (outcome, matched, reasons, ..) = decision(&pool, again).await;
    assert_eq!((outcome.as_str(), matched), ("no_match", json!([])));
    assert!(reasons[0]["reason"].as_str().unwrap().contains("live run"));
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks.runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1);
}

#[tokio::test]
async fn run_loop_catches_up_and_follows_notifications() {
    let (_node, pool, router) = setup().await;
    router.init_floor().await.unwrap();
    // Missed while tracks was down: picked up by the catch-up after connect.
    let missed = insert(&pool, Some("o/r"), "issues", "labeled", "ez", 20).await;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(router.clone().run(stop_rx));
    wait_decided(&pool, missed).await;
    // Delivered by NOTIFY (the receiver sends it in the insert transaction).
    let live = insert(&pool, Some("o/r"), "issues", "labeled", "bug", 21).await;
    sqlx::query("SELECT pg_notify('connector_event', $1)")
        .bind(live.to_string())
        .execute(&pool)
        .await
        .unwrap();
    wait_decided(&pool, live).await;
    stop_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .unwrap()
        .unwrap();
}

async fn wait_decided(pool: &PgPool, id: i64) {
    for _ in 0..100 {
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM tracks.event_decisions WHERE event_id = $1")
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap();
        if n == 1 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("event {id} was not decided");
}

#[tokio::test]
async fn decisions_endpoint_and_dry_run_by_event_ids() {
    let (_node, pool, router) = setup().await;
    router.init_floor().await.unwrap();
    let hit = insert(&pool, Some("o/r"), "issues", "labeled", "ez", 30).await;
    let undecided = insert(&pool, Some("z/z"), "push", "", "", 31).await;
    drain(&router).await;
    sqlx::query("DELETE FROM tracks.event_decisions WHERE event_id = $1")
        .bind(undecided)
        .execute(&pool)
        .await
        .unwrap();

    let token = "0123456789abcdef0123456789abcdef";
    let app = api::router(
        api::AppState::new(token.as_bytes().to_vec()).with_pool(pool.clone()),
        1 << 20,
    );
    let get = |uri: String| {
        let app = app.clone();
        async move {
            let res = app
                .oneshot(
                    Request::get(uri)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let st = res.status();
            let b = res.into_body().collect().await.unwrap().to_bytes();
            (st, serde_json::from_slice::<Value>(&b).unwrap())
        }
    };
    let (st, v) = get(format!("/v1/decisions?event_ids={hit},{undecided}")).await;
    assert_eq!(st, StatusCode::OK);
    let d = v["decisions"].as_array().unwrap();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0]["event_id"], hit);
    assert_eq!(d[0]["outcome"], "started");
    assert_eq!(d[0]["matched"], json!(["fix"]));
    assert_eq!(d[0]["sha"], "abc1234");
    assert!(d[0]["decided_at"].as_str().unwrap().ends_with('Z'));

    assert_eq!(
        get("/v1/decisions?event_ids=x".into()).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get("/v1/decisions?event_ids=".into()).await.0,
        StatusCode::BAD_REQUEST
    );

    let res = app
        .oneshot(
            Request::post("/v1/dry-run")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"workflows_toml": WORKFLOWS, "event_ids": [hit]}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let b = res.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&b).unwrap();
    assert_eq!(v["results"][0]["event_id"], hit);
    assert_eq!(v["results"][0]["triggers"][0]["matched"], true);
}
