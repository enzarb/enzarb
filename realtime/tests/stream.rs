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
    let c = json!({"sub":"alice","projects":projects,"aud":AUDIENCE,"exp":now()+ttl,"iat":now()});
    encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap()
}

struct Server {
    addr: String,
    hub: Arc<Hub>,
    limit: Arc<Semaphore>,
    rejected: Arc<AtomicU64>,
    shutdown: watch::Sender<bool>,
}

async fn serve_with(buffer: usize, ping: Duration, cap: usize) -> Server {
    let hub = Arc::new(Hub::new(buffer));
    let limit = Arc::new(Semaphore::new(cap));
    let rejected = Arc::new(AtomicU64::new(0));
    let (shutdown, rx) = watch::channel(false);
    let app = ws::router(AppState {
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
    Frame::WorkItem {
        id,
        project: p.into(),
    }
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
        r#"{"topic":"work_item","id":123,"project":"enzarb"}"#
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
