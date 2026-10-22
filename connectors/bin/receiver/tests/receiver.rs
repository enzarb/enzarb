use axum::body::Body;
use axum::http::{Request, StatusCode};
use enzarb_receiver::signature::sign;
use enzarb_receiver::{AppState, db, router};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tower::ServiceExt;

const SECRET: &[u8] = b"hook-secret";

type Row = (String, Option<String>, Option<String>, Option<String>);

async fn setup() -> (impl Sized, PgPool) {
    let node = Postgres::default().start().await.unwrap();
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 4).await.unwrap();
    db::migrate(&pool).await.unwrap();
    (node, pool)
}

fn delivery(id: &str, event: &str, body: &str, secret: &[u8]) -> Request<Body> {
    Request::post("/hooks/github")
        .header("x-hub-signature-256", sign(secret, body.as_bytes()))
        .header("x-github-event", event)
        .header("x-github-delivery", id)
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn logs_a_delivery_once_and_ignores_redelivery() {
    let (_node, pool) = setup().await;
    let app = router(AppState::new(pool.clone(), SECRET.to_vec()));
    let body = r#"{"action":"opened","repository":{"full_name":"Octo/App"},"sender":{"login":"octocat"},"issue":{"title":"Hi"}}"#;

    for _ in 0..2 {
        let res = app
            .clone()
            .oneshot(delivery("d1", "issues", body, SECRET))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }
    let rows: Vec<Row> = sqlx::query_as("SELECT event, action, repo, sender FROM events")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![(
            "issues".into(),
            Some("opened".into()),
            Some("octo/app".into()),
            Some("octocat".into())
        )]
    );
}

#[tokio::test]
async fn logs_deliveries_without_a_repository() {
    let (_node, pool) = setup().await;
    let app = router(AppState::new(pool.clone(), SECRET.to_vec()));
    let res = app
        .oneshot(delivery("d2", "ping", r#"{"zen":"x"}"#, SECRET))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    let repo: Option<String> =
        sqlx::query_scalar("SELECT repo FROM events WHERE delivery_id = 'd2'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(repo, None);
}

#[tokio::test]
async fn rejects_bad_signatures_and_bad_requests() {
    let (_node, pool) = setup().await;
    let app = router(AppState::new(pool.clone(), SECRET.to_vec()));

    let res = app
        .clone()
        .oneshot(delivery("d3", "push", "{}", b"other"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let res = app
        .clone()
        .oneshot(delivery("d4", "push", "not json", SECRET))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let missing = Request::post("/hooks/github")
        .header("x-hub-signature-256", sign(SECRET, b"{}"))
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        app.oneshot(missing).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn event_view_and_legacy_drop() {
    let (_node, pool) = setup().await;
    sqlx::query(
        "INSERT INTO events (delivery_id, event, repo, payload) VALUES ('d5','push','o/r','{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM connectors.event_v")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);

    sqlx::query("CREATE SCHEMA engine_api")
        .execute(&pool)
        .await
        .unwrap();
    db::drop_legacy(&pool).await.unwrap();
    db::drop_legacy(&pool).await.unwrap(); // idempotent
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_namespace WHERE nspname = 'engine_api'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(left, 0);
}
