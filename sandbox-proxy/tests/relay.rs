//! End-to-end: real axum server, fake resolver, fake sandbox-init websocket server.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use enzarb_proto::{
    ChannelKind, ClientKind, Ctrl, Frame, FrameType, RunStart,
    session::handshake::{client_handshake, server_handshake},
};
use enzarb_sandbox_proxy::{
    app::{AppState, router},
    auth::{AUDIENCE, now_secs},
    config::Config,
    resolver::{FakeResolver, ProjectInfo, SandboxRef},
};
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;
use tokio::{net::TcpListener, sync::mpsc};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};
use tokio_util::sync::CancellationToken;

const SECRET: &[u8] = b"integration-secret-integration-secret";
const SANDBOX_TOKEN: &str = "sandbox-bearer";

#[derive(Serialize)]
struct Claims<'a> {
    sub: &'a str,
    project: &'a str,
    aud: &'a str,
    exp: u64,
}

fn jwt(sub: &str, project: &str) -> String {
    encode(
        &Header::new(Algorithm::HS256),
        &Claims {
            sub,
            project,
            aud: AUDIENCE,
            exp: now_secs() + 120,
        },
        &EncodingKey::from_secret(SECRET),
    )
    .unwrap()
}

/// Fake sandbox-init: checks the bearer and `client: "proxy"`, echoes DATA_BIN,
/// records every binary message it receives, and pushes one CTRL frame at start.
#[allow(clippy::result_large_err)]
async fn fake_sandbox() -> (SocketAddr, mpsc::UnboundedReceiver<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let tx = tx.clone();
            tokio::spawn(async move {
                let cb = |req: &Request, resp: Response| {
                    let ok = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        == Some(&format!("Bearer {SANDBOX_TOKEN}"));
                    if ok {
                        Ok(resp)
                    } else {
                        Err(tokio_tungstenite::tungstenite::http::Response::builder()
                            .status(401)
                            .body(None)
                            .unwrap())
                    }
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(s, cb).await else {
                    return;
                };
                let hello = server_handshake(&mut ws, &[1]).await.unwrap();
                assert_eq!(hello.client, ClientKind::Proxy);
                let pushed = Frame::data_bin(9, &b"from-sandbox"[..]).to_bytes().unwrap();
                ws.send(Message::Binary(pushed)).await.unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if let Message::Binary(b) = m {
                        let _ = tx.send(b.to_vec());
                        let f = Frame::from_bytes(b).unwrap();
                        if f.frame_type == FrameType::DataBin {
                            let echo = Frame::data_bin(f.channel, f.payload.clone());
                            ws.send(Message::Binary(echo.to_bytes().unwrap()))
                                .await
                                .unwrap();
                        }
                    }
                }
            });
        }
    });
    (addr, rx)
}

struct Harness {
    proxy: SocketAddr,
    sandbox_rx: mpsc::UnboundedReceiver<Vec<u8>>,
}

async fn harness() -> Harness {
    let (sb, sandbox_rx) = fake_sandbox().await;
    let r = FakeResolver::default();
    let sref = |ep: String| SandboxRef {
        endpoint: ep,
        secret_namespace: "enzarb-p-demo".into(),
        secret_name: "sandbox-auth".into(),
        secret_key: "token".into(),
    };
    r.insert(
        "demo",
        ProjectInfo {
            owner: "ruckc".into(),
            sandbox: Some(sref(format!("ws://{sb}/v1/mux"))),
        },
        SANDBOX_TOKEN,
    );
    r.insert(
        "badtoken",
        ProjectInfo {
            owner: "ruckc".into(),
            sandbox: Some(SandboxRef {
                secret_name: "other".into(),
                ..sref(format!("ws://{sb}/v1/mux"))
            }),
        },
        "wrong-bearer",
    );
    r.insert(
        "booting",
        ProjectInfo {
            owner: "ruckc".into(),
            sandbox: None,
        },
        "",
    );
    let state = AppState::new(
        Config::for_tests(SECRET),
        Arc::new(r),
        CancellationToken::new(),
    );
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
    Harness { proxy, sandbox_rx }
}

fn url(h: &Harness, project: &str, token: &str) -> String {
    format!(
        "ws://{}/sandbox/v1/projects/{project}/mux?token={token}",
        h.proxy
    )
}

async fn status_of(url: String) -> u16 {
    match connect_async(url).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => r.status().as_u16(),
        Ok(_) => 101,
        Err(e) => panic!("unexpected {e}"),
    }
}

async fn next_bin<S>(ws: &mut S) -> Frame
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
        {
            Some(Ok(Message::Binary(b))) => return Frame::from_bytes(b).unwrap(),
            Some(Ok(Message::Close(_))) | None => panic!("closed"),
            Some(Ok(_)) => {}
            Some(Err(e)) => panic!("{e}"),
        }
    }
}

#[tokio::test]
async fn forwards_frames_both_ways() {
    let mut h = harness().await;
    let (mut ws, _) = connect_async(url(&h, "demo", &jwt("ruckc", "demo")))
        .await
        .unwrap();
    assert_eq!(client_handshake(&mut ws, ClientKind::Ui).await.unwrap(), 1);

    // Unsolicited sandbox -> browser frame.
    let f = next_bin(&mut ws).await;
    assert_eq!((f.channel, &f.payload[..]), (9, &b"from-sandbox"[..]));

    // browser -> sandbox: CTRL open, then data that is echoed back.
    let open = Frame::ctrl(&Ctrl::Open {
        channel: 1,
        kind: ChannelKind::Pty,
        cols: 80,
        rows: 24,
        command: None,
    })
    .unwrap();
    ws.send(Message::Binary(open.to_bytes().unwrap()))
        .await
        .unwrap();
    let data = Frame::data_bin(1, &b"ls\n"[..]);
    ws.send(Message::Binary(data.to_bytes().unwrap()))
        .await
        .unwrap();
    let echo = next_bin(&mut ws).await;
    assert_eq!((echo.channel, &echo.payload[..]), (1, &b"ls\n"[..]));

    // The sandbox received the frames byte for byte (the proxy's hello was consumed
    // by the fake's handshake, which also asserted `client: "proxy"`).
    let mut got = vec![];
    while got.len() < 2 {
        got.push(h.sandbox_rx.recv().await.unwrap());
    }
    assert_eq!(got[0], open.to_bytes().unwrap().to_vec());
    assert_eq!(got[1], data.to_bytes().unwrap().to_vec());

    ws.close(None).await.unwrap();
}

fn run_start() -> RunStart {
    RunStart {
        run_id: "evil".into(),
        repo_url: "https://github.com/a/b".into(),
        base_ref: "main".into(),
        branch: "x".into(),
        push: true,
        clone_token: "t".into(),
        prompt: "rm -rf".into(),
        timeout_secs: 1,
        callback_url: "http://attacker".into(),
        callback_token: "t".into(),
        log_channel: None,
    }
}

#[tokio::test]
async fn run_control_from_browser_is_rejected() {
    for msg in [
        Ctrl::RunStart(run_start()),
        Ctrl::RunCancel { run_id: "j".into() },
        Ctrl::RunStatus { run_id: "j".into() },
    ] {
        let mut h = harness().await;
        let (mut ws, _) = connect_async(url(&h, "demo", &jwt("ruckc", "demo")))
            .await
            .unwrap();
        client_handshake(&mut ws, ClientKind::Ui).await.unwrap();
        let bytes = Frame::ctrl(&msg).unwrap().to_bytes().unwrap();
        ws.send(Message::Binary(bytes)).await.unwrap();

        // The browser gets an error and the socket is closed.
        let mut saw_close = false;
        let mut saw_error = false;
        while let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            match m {
                Message::Binary(b) => {
                    let f = Frame::from_bytes(b).unwrap();
                    if f.frame_type == FrameType::Ctrl
                        && matches!(f.ctrl_message(), Ok(Ctrl::Error { .. }))
                    {
                        saw_error = true;
                    }
                }
                Message::Close(_) => {
                    saw_close = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_error && saw_close, "{msg:?}");

        // The sandbox never saw the run message (its hello was consumed by the handshake).
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut seen = vec![];
        while let Ok(b) = h.sandbox_rx.try_recv() {
            seen.push(Frame::from_bytes(b.into()).unwrap());
        }
        assert!(seen.is_empty(), "leaked {seen:?}");
    }
}

#[tokio::test]
async fn browser_cannot_impersonate_executor_in_hello() {
    let h = harness().await;
    let (mut ws, _) = connect_async(url(&h, "demo", &jwt("ruckc", "demo")))
        .await
        .unwrap();
    assert!(
        client_handshake(&mut ws, ClientKind::Executor)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn http_errors() {
    let h = harness().await;
    assert_eq!(status_of(url(&h, "demo", "garbage")).await, 401);
    assert_eq!(
        status_of(format!("ws://{}/sandbox/v1/projects/demo/mux", h.proxy)).await,
        401
    );
    // Valid token for a different project than the path.
    assert_eq!(
        status_of(url(&h, "demo", &jwt("ruckc", "other"))).await,
        403
    );
    // Not the owner.
    assert_eq!(
        status_of(url(&h, "demo", &jwt("mallory", "demo"))).await,
        403
    );
    assert_eq!(status_of(url(&h, "nope", &jwt("ruckc", "nope"))).await, 404);
    assert_eq!(
        status_of(url(&h, "booting", &jwt("ruckc", "booting"))).await,
        503
    );
    // Sandbox refuses the bearer.
    assert_eq!(
        status_of(url(&h, "badtoken", &jwt("ruckc", "badtoken"))).await,
        502
    );
    assert_eq!(status_of(url(&h, "demo", &jwt("ruckc", "demo"))).await, 101);
}

#[tokio::test]
async fn healthz() {
    let h = harness().await;
    let body = tokio::net::TcpStream::connect(h.proxy).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut body = body;
    body.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut s = String::new();
    body.read_to_string(&mut s).await.unwrap();
    assert!(s.starts_with("HTTP/1.1 200"));
}
