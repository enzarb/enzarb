//! `GithubApp` against a fake GitHub: rate limits (403 + `retry-after`) are waited out or reported
//! as transient, a plain 403 is permanent, and snapshots are cached and revalidated.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::SeqCst};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use enzarb_tracks::github::{GithubApp, RATE_LIMITED, RepoSource, SourceError};

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Default)]
struct Fake {
    /// Head requests answered with the rate limit before it clears.
    limited_heads: AtomicU32,
    /// Plain 403 (no rate-limit signals) for the head.
    forbidden: AtomicBool,
    heads: AtomicU32,
    not_modified: AtomicU32,
    contents: AtomicU32,
}

async fn installation() -> &'static str {
    r#"{"id": 7}"#
}
async fn token() -> (StatusCode, &'static str) {
    (StatusCode::CREATED, r#"{"token": "t"}"#)
}
async fn head(State(f): State<Arc<Fake>>, h: HeaderMap) -> Response {
    f.heads.fetch_add(1, SeqCst);
    if f.forbidden.load(SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            "Resource not accessible by integration",
        )
            .into_response();
    }
    if f.limited_heads.load(SeqCst) > 0 {
        f.limited_heads.fetch_sub(1, SeqCst);
        return (
            StatusCode::FORBIDDEN,
            [("retry-after", "1"), ("x-ratelimit-remaining", "0")],
            "You have exceeded a secondary rate limit.",
        )
            .into_response();
    }
    if h.get("if-none-match").is_some_and(|v| v == "\"e1\"") {
        f.not_modified.fetch_add(1, SeqCst);
        return StatusCode::NOT_MODIFIED.into_response();
    }
    ([(header::ETAG, "\"e1\"")], SHA).into_response()
}
async fn content(State(f): State<Arc<Fake>>) -> Response {
    f.contents.fetch_add(1, SeqCst);
    StatusCode::NOT_FOUND.into_response()
}

async fn serve() -> (Arc<Fake>, String) {
    let fake = Arc::new(Fake::default());
    let app = Router::new()
        .route("/repos/o/r/installation", get(installation))
        .route("/app/installations/7/access_tokens", post(token))
        .route("/repos/o/r/commits/main", get(head))
        .route("/repos/o/r/contents/{*path}", get(content))
        .with_state(fake.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (fake, base)
}

fn app(base: &str, ttl: Duration, max_wait: Duration) -> GithubApp {
    let key = include_str!("fixtures/test-app-key.pem");
    GithubApp::with_base("1", key, base)
        .unwrap()
        .with_limits(ttl, max_wait)
}

#[tokio::test]
async fn rate_limit_with_retry_after_is_waited_out() {
    let (fake, base) = serve().await;
    fake.limited_heads.store(1, SeqCst);
    let a = app(&base, Duration::from_secs(30), Duration::from_secs(30));
    let t = std::time::Instant::now();
    let snap = a.snapshot("o/r", "main").await.unwrap();
    assert_eq!(snap.sha, SHA);
    assert!(
        t.elapsed() >= Duration::from_millis(900),
        "honoured retry-after"
    );
    assert_eq!(fake.heads.load(SeqCst), 2);
}

#[tokio::test]
async fn too_long_a_limit_is_transient_and_stops_sending() {
    let (fake, base) = serve().await;
    fake.limited_heads.store(100, SeqCst);
    // retry-after (1s) exceeds the budget (0): fail at once, marked as a rate limit.
    let a = app(&base, Duration::from_secs(30), Duration::ZERO);
    let e = a.snapshot("o/r", "main").await.unwrap_err();
    let SourceError::Transient(m) = e else {
        panic!("{e:?}")
    };
    assert!(m.contains(RATE_LIMITED), "{m}");
    // While the limit is active no further request reaches GitHub.
    let again = a.snapshot("o/r", "main").await.unwrap_err();
    assert!(matches!(again, SourceError::Transient(_)));
    assert_eq!(fake.heads.load(SeqCst), 1);
}

#[tokio::test]
async fn plain_403_is_permanent() {
    let (fake, base) = serve().await;
    fake.forbidden.store(true, SeqCst);
    let a = app(&base, Duration::from_secs(30), Duration::from_secs(30));
    let e = a.snapshot("o/r", "main").await.unwrap_err();
    assert!(matches!(e, SourceError::Permanent(_)), "{e:?}");
    assert_eq!(fake.heads.load(SeqCst), 1);
}

#[tokio::test]
async fn snapshots_are_cached_shared_and_revalidated() {
    let (fake, base) = serve().await;
    let a = Arc::new(app(
        &base,
        Duration::from_millis(300),
        Duration::from_secs(30),
    ));
    let mut hs = Vec::new();
    for _ in 0..8 {
        let a = a.clone();
        hs.push(tokio::spawn(async move { a.snapshot("o/r", "main").await }));
    }
    for h in hs {
        assert_eq!(h.await.unwrap().unwrap().sha, SHA);
    }
    assert_eq!(
        fake.heads.load(SeqCst),
        1,
        "one fetch for concurrent events"
    );
    assert_eq!(fake.contents.load(SeqCst), 2, "workflows.toml, modes.toml");
    a.snapshot("o/r", "main").await.unwrap();
    assert_eq!(fake.heads.load(SeqCst), 1, "cache hit");
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(a.snapshot("o/r", "main").await.unwrap().sha, SHA);
    assert_eq!(fake.heads.load(SeqCst), 2, "revalidated after the TTL");
    assert_eq!(fake.not_modified.load(SeqCst), 1, "with If-None-Match");
    assert_eq!(
        fake.contents.load(SeqCst),
        2,
        "files kept for an unchanged head"
    );
}
