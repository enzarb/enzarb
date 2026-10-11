use std::{
    sync::Arc,
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
use tokio_tungstenite::tungstenite::{Message, protocol::frame::coding::CloseCode};

const SECRET: &[u8] = b"s3cret";

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

async fn serve(buffer: usize, ping: Duration) -> (String, Arc<Hub>) {
    let hub = Arc::new(Hub::new(buffer));
    let app = ws::router(AppState {
        hub: hub.clone(),
        secret: Arc::new(SECRET.to_vec()),
        ping_interval: ping,
        max_token_ttl: Duration::from_secs(300),
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (format!("127.0.0.1:{}", addr.port()), hub)
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
