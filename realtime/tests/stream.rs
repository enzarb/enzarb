use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use enzarb_realtime::{
    auth::AUDIENCE,
    hub::{Frame, Hub},
    ws::{self, AppState, CLOSE_TOKEN_EXPIRED},
};
use futures_util::StreamExt;
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::json;
use tokio::sync::{Semaphore, watch};
use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};

const SECRET: &[u8] = b"test-secret-0123456789-abcdefghijklmnop";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn token(projects: &[&str], ttl: u64) -> String {
    let c = json!({"sub":"alice","projects":["p"],"repos":projects,"aud":AUDIENCE,"exp":now()+ttl,"iat":now()});
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

struct Server {
    addr: String,
    hub: Arc<Hub>,
    limit: Arc<Semaphore>,
    rejected: Arc<AtomicU64>,
    deprecated: Arc<AtomicU64>,
    shutdown: watch::Sender<bool>,
}

async fn serve_with(buffer: usize, ping: Duration, cap: usize) -> Server {
    serve_cfg(buffer, ping, cap, vec![]).await
}

async fn serve_cfg(buffer: usize, ping: Duration, cap: usize, allowed: Vec<String>) -> Server {
    let hub = Arc::new(Hub::new(buffer));
    let limit = Arc::new(Semaphore::new(cap));
    let rejected = Arc::new(AtomicU64::new(0));
    let deprecated = Arc::new(AtomicU64::new(0));
    let (shutdown, rx) = watch::channel(false);
    let app = ws::router(AppState {
        allowed_origins: Arc::new(allowed),
        deprecated_query: deprecated.clone(),
        hub: hub.clone(),
        secret: Arc::new(SECRET.to_vec()),
        ping_interval: ping,
        max_token_ttl: Duration::from_secs(300),
        limit: limit.clone(),
        rejected: rejected.clone(),
        shutdown: rx,
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Server {
        addr: format!("127.0.0.1:{}", addr.port()),
        hub,
        limit,
        rejected,
        deprecated,
        shutdown,
    }
}

async fn serve(buffer: usize, ping: Duration) -> (String, Arc<Hub>) {
    let s = serve_with(buffer, ping, 100).await;
    // Leak the sender so the shutdown channel stays open for the test's lifetime.
    std::mem::forget(s.shutdown);
    (s.addr, s.hub)
}

fn url(addr: &str) -> String {
    format!(
        "ws://{addr}/live/v1/stream?token={}",
        token(&["enzarb"], 60)
    )
}

async fn expect_close(
    ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
    code: u16,
) {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
            .expect("stream ended without close")
            .expect("err");
        match m {
            Message::Close(Some(cf)) => {
                assert_eq!(cf.code, CloseCode::from(code));
                return;
            }
            Message::Close(None) => panic!("close without code"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn connection_cap_rejects_then_frees_slot() {
    let s = serve_with(16, Duration::from_secs(15), 2).await;
    let (mut a, _) = tokio_tungstenite::connect_async(url(&s.addr))
        .await
        .unwrap();
    let (_b, _) = tokio_tungstenite::connect_async(url(&s.addr))
        .await
        .unwrap();
    match tokio_tungstenite::connect_async(url(&s.addr)).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 503);
            assert_eq!(resp.headers().get("retry-after").unwrap(), "1");
        }
        other => panic!("expected 503, got {other:?}"),
    }
    assert_eq!(s.rejected.load(Ordering::Relaxed), 1);
    a.close(None).await.unwrap();
    drop(a);
    for _ in 0..100 {
        if s.limit.available_permits() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio_tungstenite::connect_async(url(&s.addr))
        .await
        .expect("slot should be free");
}

#[tokio::test]
async fn shutdown_sends_going_away() {
    let s = serve_with(16, Duration::from_secs(15), 10).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(url(&s.addr))
        .await
        .unwrap();
    s.shutdown.send(true).unwrap();
    expect_close(&mut ws, ws::CLOSE_GOING_AWAY).await;
}

#[tokio::test]
async fn silent_client_dropped_at_one_and_a_half_intervals() {
    let ping = Duration::from_millis(400);
    let s = serve_with(16, ping, 1).await;
    // Never read, so tungstenite never answers the server's pings.
    let (_ws, _) = tokio_tungstenite::connect_async(url(&s.addr))
        .await
        .unwrap();
    assert_eq!(s.limit.available_permits(), 0);
    let start = std::time::Instant::now();
    while s.limit.available_permits() == 0 {
        assert!(start.elapsed() < Duration::from_secs(5), "never dropped");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let took = start.elapsed();
    // Deadline is 1.5 intervals (600 ms); allow up to one tick of slack, and not before.
    assert!(took >= ping * 3 / 2 - Duration::from_millis(50), "{took:?}");
    assert!(took <= ping * 3 / 2 + ping, "{took:?}");
}

async fn text(
    ws: &mut (impl StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin),
) -> String {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        if let Message::Text(t) = m {
            return t.to_string();
        }
    }
}

fn wi(id: i64, p: &str) -> Frame {
    Frame::Event { id, repo: p.into() }
}

#[tokio::test]
async fn filters_by_project_and_formats_frames() {
    let (addr, hub) = serve(16, Duration::from_secs(15)).await;
    let url = format!(
        "ws://{addr}/live/v1/stream?token={}",
        token(&["enzarb"], 60)
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    hub.publish(wi(1, "other"));
    hub.publish(wi(123, "enzarb"));
    hub.publish(Frame::Resync);
    assert_eq!(
        text(&mut ws).await,
        r#"{"topic":"event","id":123,"repo":"enzarb"}"#
    );
    assert_eq!(text(&mut ws).await, r#"{"topic":"resync"}"#);
}

#[tokio::test]
async fn rejects_bad_tokens_before_upgrade() {
    let (addr, _) = serve(16, Duration::from_secs(15)).await;
    for q in ["", "?token=garbage"] {
        let r = tokio_tungstenite::connect_async(format!("ws://{addr}/live/v1/stream{q}")).await;
        match r {
            Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
                assert_eq!(resp.status(), 401)
            }
            other => panic!("expected 401, got {other:?}"),
        }
    }
}

// current_thread: the server task cannot drain the channel during the synchronous burst below.
#[tokio::test(flavor = "current_thread")]
async fn lagging_client_gets_resync() {
    let (addr, hub) = serve(2, Duration::from_secs(15)).await;
    let url = format!(
        "ws://{addr}/live/v1/stream?token={}",
        token(&["enzarb"], 60)
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Overflow the 2-slot buffer in one burst, before the server task can drain it.
    for i in 0..50 {
        hub.publish(wi(i, "enzarb"));
    }
    let mut saw_resync = false;
    for _ in 0..60 {
        if text(&mut ws).await == r#"{"topic":"resync"}"# {
            saw_resync = true;
            break;
        }
    }
    assert!(saw_resync);
}

#[tokio::test]
async fn closes_at_exp_with_reconnect_code() {
    let (addr, _) = serve(16, Duration::from_secs(15)).await;
    let url = format!("ws://{addr}/live/v1/stream?token={}", token(&["enzarb"], 1));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match m {
        Message::Close(Some(cf)) => assert_eq!(cf.code, CloseCode::from(CLOSE_TOKEN_EXPIRED)),
        other => panic!("expected close, got {other:?}"),
    }
}

#[tokio::test]
async fn readyz_follows_hub_and_healthz_ok() {
    let (addr, hub) = serve(16, Duration::from_secs(15)).await;
    let get = |p: &'static str| {
        let addr = addr.clone();
        async move {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            s.write_all(
                format!("GET {p} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            out.lines().next().unwrap().to_string()
        }
    };
    assert!(get("/healthz").await.contains("200"));
    assert!(get("/readyz").await.contains("503"));
    hub.set_ready(true);
    assert!(get("/readyz").await.contains("200"));
}

// ---- auth sources ----

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connects with the given query and headers; `Err(status)` when the server refuses the upgrade.
async fn connect(addr: &str, query: &str, headers: &[(&str, &str)]) -> Result<Ws, u16> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{addr}/live/v1/stream{query}")
        .into_client_request()
        .unwrap();
    for (k, v) in headers {
        req.headers_mut().append(
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((ws, _)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => Err(resp.status().as_u16()),
        Err(e) => panic!("unexpected error {e:?}"),
    }
}

fn good() -> String {
    token(&["enzarb"], 60)
}

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

fn cookie(t: &str) -> String {
    format!("enzarb_live={t}")
}

fn origin_of(addr: &str) -> String {
    format!("http://{addr}")
}

#[tokio::test]
async fn each_source_works_on_its_own() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let t = good();
    assert!(
        connect(&s.addr, "", &[("authorization", &bearer(&t))])
            .await
            .is_ok()
    );
    let o = origin_of(&s.addr);
    assert!(
        connect(&s.addr, "", &[("cookie", &cookie(&t)), ("origin", &o)])
            .await
            .is_ok()
    );
    assert!(connect(&s.addr, &format!("?token={t}"), &[]).await.is_ok());
}

#[tokio::test]
async fn precedence_bearer_then_cookie_then_query() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let (t, bad) = (good(), "garbage".to_string());
    let o = origin_of(&s.addr);
    // bad bearer, valid cookie + query
    let r = connect(
        &s.addr,
        &format!("?token={t}"),
        &[
            ("authorization", &bearer(&bad)),
            ("cookie", &cookie(&t)),
            ("origin", &o),
        ],
    )
    .await;
    assert_eq!(r.err(), Some(401));
    // valid bearer wins over bad cookie and bad query
    let r = connect(
        &s.addr,
        "?token=garbage",
        &[("authorization", &bearer(&t)), ("cookie", &cookie(&bad))],
    )
    .await;
    assert!(r.is_ok());
    // bad cookie, valid query
    let r = connect(
        &s.addr,
        &format!("?token={t}"),
        &[("cookie", &cookie(&bad)), ("origin", &o)],
    )
    .await;
    assert_eq!(r.err(), Some(401));
    assert_eq!(s.deprecated.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn malformed_authorization_does_not_fall_through() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let t = good();
    let o = origin_of(&s.addr);
    for auth in ["Basic x", "Bearer ", "Bearer"] {
        let r = connect(
            &s.addr,
            &format!("?token={t}"),
            &[
                ("authorization", auth),
                ("cookie", &cookie(&t)),
                ("origin", &o),
            ],
        )
        .await;
        assert_eq!(r.err(), Some(401), "{auth}");
    }
}

#[tokio::test]
async fn cookie_requires_allowed_origin() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let c = cookie(&good());
    let evil = "http://evil.example";
    let r = connect(&s.addr, "", &[("cookie", &c), ("origin", evil)]).await;
    assert_eq!(r.err(), Some(403));
    let r = connect(&s.addr, "", &[("cookie", &c)]).await;
    assert_eq!(r.err(), Some(403));
    let r = connect(&s.addr, "", &[("cookie", &c), ("origin", "null")]).await;
    assert_eq!(r.err(), Some(403));
    // bad origin wins over a bad token
    let r = connect(
        &s.addr,
        "",
        &[("cookie", &cookie("garbage")), ("origin", evil)],
    )
    .await;
    assert_eq!(r.err(), Some(403));
    // right origin, bad token: 401
    let o = origin_of(&s.addr);
    let r = connect(
        &s.addr,
        "",
        &[("cookie", &cookie("garbage")), ("origin", &o)],
    )
    .await;
    assert_eq!(r.err(), Some(401));
}

#[tokio::test]
async fn bearer_and_query_ignore_origin() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let t = good();
    let evil = ("origin", "http://evil.example");
    let r = connect(&s.addr, "", &[("authorization", &bearer(&t)), evil]).await;
    assert!(r.is_ok());
    let r = connect(&s.addr, &format!("?token={t}"), &[evil]).await;
    assert!(r.is_ok());
}

#[tokio::test]
async fn explicit_allowlist_replaces_host_default() {
    let s = serve_cfg(
        16,
        Duration::from_secs(15),
        100,
        vec!["https://app.example".into()],
    )
    .await;
    let c = cookie(&good());
    let host_origin = origin_of(&s.addr);
    let r = connect(&s.addr, "", &[("cookie", &c), ("origin", &host_origin)]).await;
    assert_eq!(r.err(), Some(403));
    let r = connect(
        &s.addr,
        "",
        &[("cookie", &c), ("origin", "https://app.example")],
    )
    .await;
    assert!(r.is_ok());
}

#[tokio::test]
async fn host_default_ignores_forwarded_host() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let c = cookie(&good());
    let r = connect(
        &s.addr,
        "",
        &[
            ("cookie", &c),
            ("origin", "http://evil.example"),
            ("x-forwarded-host", "evil.example"),
        ],
    )
    .await;
    assert_eq!(r.err(), Some(403));
}

#[tokio::test]
async fn deprecated_counter_only_on_successful_query() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let t = good();
    let o = origin_of(&s.addr);
    connect(&s.addr, "", &[("authorization", &bearer(&t))])
        .await
        .unwrap();
    connect(&s.addr, "", &[("cookie", &cookie(&t)), ("origin", &o)])
        .await
        .unwrap();
    assert!(connect(&s.addr, "?token=garbage", &[]).await.is_err());
    assert_eq!(s.deprecated.load(Ordering::Relaxed), 0);
    connect(&s.addr, &format!("?token={t}"), &[]).await.unwrap();
    assert_eq!(s.deprecated.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn unauthenticated_request_at_cap_gets_401_not_503() {
    let s = serve_with(16, Duration::from_secs(15), 1).await;
    let _held = connect(&s.addr, &format!("?token={}", good()), &[])
        .await
        .unwrap();
    assert_eq!(connect(&s.addr, "", &[]).await.err(), Some(401));
    assert_eq!(
        connect(&s.addr, "?token=garbage", &[]).await.err(),
        Some(401)
    );
    assert_eq!(s.rejected.load(Ordering::Relaxed), 0);
    let r = connect(&s.addr, "", &[("authorization", &bearer(&good()))]).await;
    assert_eq!(r.err(), Some(503));
}

#[tokio::test]
async fn expired_token_via_bearer_and_cookie_gets_4001() {
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let t = token(&["enzarb"], 1);
    let mut a = connect(&s.addr, "", &[("authorization", &bearer(&t))])
        .await
        .unwrap();
    expect_close(&mut a, CLOSE_TOKEN_EXPIRED).await;
    let o = origin_of(&s.addr);
    let mut b = connect(&s.addr, "", &[("cookie", &cookie(&t)), ("origin", &o)])
        .await
        .unwrap();
    expect_close(&mut b, CLOSE_TOKEN_EXPIRED).await;
}

#[derive(Clone, Default)]
struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buf {
    type Writer = Buf;
    fn make_writer(&'a self) -> Buf {
        self.clone()
    }
}

// current_thread (the default): the server task runs on this thread, so `set_default` sees it.
#[tokio::test]
async fn logs_carry_source_but_never_credentials() {
    // A process-global subscriber: a scoped one races with tracing's callsite-interest cache
    // when other tests run in parallel. Other tests' lines land here too, which is harmless.
    static BUF: std::sync::OnceLock<Buf> = std::sync::OnceLock::new();
    let buf = BUF.get_or_init(Buf::default).clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish(),
    )
    .expect("only this test installs a global subscriber");
    let s = serve_with(16, Duration::from_secs(15), 100).await;
    let t = good();
    let o = origin_of(&s.addr);
    let bad = "secretbadtoken.abc.def";
    let evil = "http://evil.example";
    // successes
    connect(&s.addr, "", &[("authorization", &bearer(&t))])
        .await
        .unwrap();
    connect(&s.addr, "", &[("cookie", &cookie(&t)), ("origin", &o)])
        .await
        .unwrap();
    connect(&s.addr, &format!("?token={t}"), &[]).await.unwrap();
    // rejections
    let h = [("authorization", bearer(bad))];
    assert!(connect(&s.addr, "", &[(h[0].0, &h[0].1)]).await.is_err());
    assert!(
        connect(&s.addr, "", &[("authorization", "Basic c2VjcmV0")])
            .await
            .is_err()
    );
    assert!(
        connect(&s.addr, "", &[("cookie", &cookie(bad)), ("origin", &o)])
            .await
            .is_err()
    );
    assert!(
        connect(&s.addr, "", &[("cookie", &cookie(&t)), ("origin", evil)])
            .await
            .is_err()
    );
    assert!(
        connect(&s.addr, &format!("?token={bad}"), &[])
            .await
            .is_err()
    );
    let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
    for secret in [t.as_str(), bad, "c2VjcmV0", "evil.example", "enzarb_live"] {
        assert!(!out.contains(secret), "log leaked {secret}: {out}");
    }
    for label in ["bearer", "cookie", "query"] {
        assert!(
            out.contains(&format!("source=\"{label}\"")),
            "{label}: {out}"
        );
    }
    assert!(out.contains("deprecated"), "{out}");
}
