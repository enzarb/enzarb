//! End-to-end: real axum server, fake resolver, fake sandbox-init websocket server.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use enzarb_proto::{
    ChannelKind, ClientKind, CredentialKind, Ctrl, Frame, FrameType, StepStart,
    session::handshake::{
        client_handshake, client_handshake_with_features, server_handshake_with_features,
    },
};
use enzarb_sandbox_proxy::{
    app::{AppState, router},
    auth::{AUDIENCE, now_secs},
    config::Config,
    credential::Keyring,
    resolver::{FakeResolver, ProjectInfo, SandboxRef},
    store::{Credentials, MemStore},
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
const OWNER_TOKEN: &str = "sk-ant-oat01-OWNER-SECRET";
const CAPTURED: &str = "sk-ant-oat01-CAPTURED-SECRET";

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
async fn fake_sandbox(features: Vec<String>) -> (SocketAddr, mpsc::UnboundedReceiver<Vec<u8>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (s, _) = l.accept().await.unwrap();
            let tx = tx.clone();
            let features = features.clone();
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
                let hello = server_handshake_with_features(&mut ws, &[1], &features)
                    .await
                    .unwrap();
                assert_eq!(hello.client, ClientKind::Proxy);
                assert_eq!(
                    hello.features,
                    vec![enzarb_proto::feature::CLAUDE_USER_TOKEN.to_string()]
                );
                let pushed = Frame::data_bin(9, &b"from-sandbox"[..]).to_bytes().unwrap();
                ws.send(Message::Binary(pushed)).await.unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if let Message::Binary(b) = m {
                        let _ = tx.send(b.to_vec());
                        let f = Frame::from_bytes(b).unwrap();
                        // Scripted sandbox behaviour for the chat tests.
                        if let Ok(Ctrl::Open { channel, repo, .. }) = f.ctrl_message()
                            && repo.as_deref() == Some("refuse")
                        {
                            let e = Frame::ctrl(&Ctrl::OpenError {
                                channel,
                                message: "disabled".into(),
                                code: Some("chat_disabled".into()),
                            })
                            .unwrap();
                            ws.send(Message::Binary(e.to_bytes().unwrap()))
                                .await
                                .unwrap();
                        }
                        if f.frame_type == FrameType::DataBin && f.payload.starts_with(b"capture") {
                            let c = Frame::ctrl(&Ctrl::CredentialCaptured {
                                kind: CredentialKind::ClaudeOauth,
                                token: Some(enzarb_proto::Secret::new(CAPTURED)),
                                legacy: (&f.payload[..] == b"capture-legacy").then_some(true),
                            })
                            .unwrap();
                            ws.send(Message::Binary(c.to_bytes().unwrap()))
                                .await
                                .unwrap();
                        } else if f.frame_type == FrameType::DataBin && &f.payload[..] == b"bye" {
                            let c = Frame::close(f.channel, 0, "superseded");
                            ws.send(Message::Binary(c.to_bytes().unwrap()))
                                .await
                                .unwrap();
                        } else if f.frame_type == FrameType::DataBin {
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
    resolver: Arc<FakeResolver>,
    sandbox: SocketAddr,
    store: Arc<MemStore>,
    keyring: Arc<Keyring>,
}

impl Harness {
    fn demo(&self, owner: &str, ready: bool) -> ProjectInfo {
        ProjectInfo {
            owner: owner.into(),
            sandbox: ready.then(|| SandboxRef {
                endpoint: format!("ws://{}/v1/mux", self.sandbox),
                secret_namespace: "enzarb-sandboxes".into(),
                secret_name: "sb-demo-auth".into(),
                secret_key: "token".into(),
            }),
        }
    }
}

async fn harness() -> Harness {
    harness_with(vec![], |_| {}).await
}

async fn harness_with(features: Vec<String>, tweak: impl FnOnce(&mut Config)) -> Harness {
    let (sb, sandbox_rx) = fake_sandbox(features).await;
    let r = FakeResolver::default();
    let sref = |ep: String| SandboxRef {
        endpoint: ep,
        secret_namespace: "enzarb-sandboxes".into(),
        secret_name: "sb-demo-auth".into(),
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
    let r = Arc::new(r);
    let mut cfg = Config::for_tests(SECRET);
    tweak(&mut cfg);
    let keyring = Arc::new(Keyring::from_keys("k1", [("k1".to_string(), [5; 32])]).unwrap());
    let store = Arc::new(MemStore::default());
    let creds = Arc::new(Credentials::new(store.clone(), keyring.clone()));
    let (key_id, ciphertext) = keyring
        .seal("ruckc", &enzarb_proto::Secret::new(OWNER_TOKEN))
        .unwrap();
    store.set_row(
        "ruckc",
        enzarb_sandbox_proxy::store::Stored { key_id, ciphertext },
    );
    let h_store = store.clone();
    let state = AppState::new(cfg, r.clone(), creds, CancellationToken::new());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, router(state)).await.unwrap() });
    Harness {
        proxy,
        sandbox_rx,
        resolver: r,
        sandbox: sb,
        store: h_store,
        keyring,
    }
}

impl Harness {
    fn seed(&self, owner: &str, token: &str) {
        let (key_id, ciphertext) = self
            .keyring
            .seal(owner, &enzarb_proto::Secret::new(token))
            .unwrap();
        self.store.set_row(
            owner,
            enzarb_sandbox_proxy::store::Stored { key_id, ciphertext },
        );
    }

    fn stored(&self, owner: &str) -> Option<String> {
        self.store.row(owner).map(|r| {
            self.keyring
                .open(owner, &r.key_id, &r.ciphertext)
                .unwrap()
                .expose()
                .to_string()
        })
    }
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
        repo: None,
        resume: None,
        claude_token: None,
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

fn step_start() -> StepStart {
    StepStart {
        run_id: "evil".into(),
        attempt: 1,
        task_id: "t".into(),
        workspaces: vec![],
        exec: enzarb_proto::StepExec::Command {
            run: "rm -rf".into(),
            tools: vec![],
        },
        outputs: Default::default(),
        env: Default::default(),
        github_token: None,
    }
}

#[tokio::test]
async fn run_control_from_browser_is_rejected() {
    for msg in [
        Ctrl::StepStart(Box::new(step_start())),
        Ctrl::StepStop { run_id: "j".into() },
        Ctrl::StepStatus {
            run_id: "j".into(),
            after_seq: 0,
        },
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

#[tokio::test]
async fn credential_status_reaches_sandbox_but_credential_state_is_rejected() {
    let mut h = harness().await;
    let (mut ws, _) = connect_async(url(&h, "demo", &jwt("ruckc", "demo")))
        .await
        .unwrap();
    client_handshake(&mut ws, ClientKind::Ui).await.unwrap();
    let status = Frame::ctrl(&Ctrl::CredentialStatus)
        .unwrap()
        .to_bytes()
        .unwrap();
    ws.send(Message::Binary(status.clone())).await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), h.sandbox_rx.recv())
        .await
        .expect("timeout")
        .unwrap();
    assert_eq!(got, status.to_vec());

    // A browser-sent credential_state is rejected and never forwarded.
    let state = Frame::ctrl(&Ctrl::CredentialState {
        kind: CredentialKind::ClaudeOauth,
        configured: true,
        scope: None,
    })
    .unwrap()
    .to_bytes()
    .unwrap();
    ws.send(Message::Binary(state)).await.unwrap();
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
            Message::Close(_) => break,
            _ => {}
        }
    }
    assert!(saw_error);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.sandbox_rx.try_recv().is_err());
}

type Browser =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Connect, handshake and prove the relay is live by echoing a frame.
async fn open_relay(h: &Harness) -> Browser {
    let (mut ws, _) = connect_async(url(h, "demo", &jwt("ruckc", "demo")))
        .await
        .unwrap();
    client_handshake(&mut ws, ClientKind::Ui).await.unwrap();
    let f = next_bin(&mut ws).await;
    assert_eq!(f.channel, 9);
    send_echo(&mut ws).await;
    ws
}

async fn send_echo(ws: &mut Browser) {
    let data = Frame::data_bin(1, &b"ls\n"[..]);
    ws.send(Message::Binary(data.to_bytes().unwrap()))
        .await
        .unwrap();
    let echo = next_bin(ws).await;
    assert_eq!((echo.channel, &echo.payload[..]), (1, &b"ls\n"[..]));
}

/// The close frame the browser receives next, skipping any data.
async fn close_of(ws: &mut Browser) -> (u16, String) {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("timeout")
        {
            Some(Ok(Message::Close(Some(c)))) => return (c.code.into(), c.reason.to_string()),
            Some(Ok(Message::Close(None))) | None => panic!("closed without a reason"),
            Some(Ok(_)) => {}
            Some(Err(e)) => panic!("{e}"),
        }
    }
}

#[tokio::test]
async fn owner_change_closes_relay() {
    let h = harness().await;
    let mut ws = open_relay(&h).await;
    h.resolver
        .update("demo", h.demo("mallory", true), SANDBOX_TOKEN);
    assert_eq!(
        close_of(&mut ws).await,
        (1008, "project access changed".into())
    );
}

#[tokio::test]
async fn cleared_sandbox_status_closes_relay() {
    let h = harness().await;
    let mut ws = open_relay(&h).await;
    h.resolver.update("demo", h.demo("ruckc", false), "");
    assert_eq!(
        close_of(&mut ws).await,
        (1008, "project access changed".into())
    );
}

#[tokio::test]
async fn deleted_project_closes_relay() {
    let h = harness().await;
    let mut ws = open_relay(&h).await;
    h.resolver.remove("demo");
    assert_eq!(
        close_of(&mut ws).await,
        (1008, "project access changed".into())
    );
}

#[tokio::test]
async fn unrelated_updates_keep_relay_open() {
    let h = harness().await;
    let mut ws = open_relay(&h).await;
    // Another Project changes owner.
    h.resolver.update("booting", h.demo("mallory", false), "");
    // Same owner, different secret ref and endpoint query.
    let mut same = h.demo("ruckc", true);
    same.sandbox.as_mut().unwrap().secret_name = "sb-demo-auth-2".into();
    h.resolver.update("demo", same, SANDBOX_TOKEN);
    tokio::time::sleep(Duration::from_millis(200)).await;
    send_echo(&mut ws).await;
}

// ---- chat tab (acp) ----

fn acp_features() -> Vec<String> {
    vec!["acp".into()]
}

fn open_frame(channel: u32, kind: ChannelKind, repo: Option<&str>) -> Vec<u8> {
    Frame::ctrl(&Ctrl::Open {
        channel,
        kind,
        cols: 0,
        rows: 0,
        command: None,
        repo: repo.map(Into::into),
        resume: None,
        claude_token: None,
    })
    .unwrap()
    .to_bytes()
    .unwrap()
    .to_vec()
}

/// `open` as the proxy forwards it: with the owner's token injected.
fn injected(open: Vec<u8>) -> Vec<u8> {
    let mut m = Frame::from_bytes(open.into())
        .unwrap()
        .ctrl_message()
        .unwrap();
    if let Ctrl::Open { claude_token, .. } = &mut m {
        *claude_token = Some(enzarb_proto::Secret::new(OWNER_TOKEN));
    }
    ctrl_bytes(&m)
}

async fn connect_ui(h: &Harness) -> (Browser, Vec<String>) {
    let (mut ws, _) = connect_async(url(h, "demo", &jwt("ruckc", "demo")))
        .await
        .unwrap();
    let w = client_handshake_with_features(&mut ws, ClientKind::Ui)
        .await
        .unwrap();
    let f = next_bin(&mut ws).await;
    assert_eq!(f.channel, 9);
    (ws, w.features)
}

async fn next_ctrl(ws: &mut Browser) -> Ctrl {
    loop {
        let f = next_bin(ws).await;
        if f.frame_type == FrameType::Ctrl {
            return f.ctrl_message().unwrap();
        }
    }
}

async fn expect_sandbox(h: &mut Harness) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(5), h.sandbox_rx.recv())
        .await
        .expect("timeout")
        .unwrap()
}

#[tokio::test]
async fn welcome_features_are_the_intersection() {
    for (sandbox, expected) in [
        (vec![], vec![]),
        (acp_features(), acp_features()),
        (
            vec!["acp".to_string(), "future".to_string()],
            acp_features(),
        ),
        (vec!["future".to_string()], vec![]),
    ] {
        let h = harness_with(sandbox, |_| {}).await;
        let (_ws, features) = connect_ui(&h).await;
        assert_eq!(features, expected);
    }
}

#[tokio::test]
async fn acp_open_gets_owner_token_and_data_relays_both_ways() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    let (mut ws, features) = connect_ui(&h).await;
    assert_eq!(features, acp_features());
    // A browser-supplied claude_token is dropped; the owner's is injected.
    let open = Frame::ctrl(&Ctrl::Open {
        channel: 3,
        kind: ChannelKind::Acp,
        cols: 0,
        rows: 0,
        command: None,
        repo: Some("https://github.com/a/b".into()),
        resume: None,
        claude_token: Some(enzarb_proto::Secret::new("browser-supplied")),
    })
    .unwrap()
    .to_bytes()
    .unwrap();
    ws.send(Message::Binary(open)).await.unwrap();
    let got = Frame::from_bytes(expect_sandbox(&mut h).await.into())
        .unwrap()
        .ctrl_message()
        .unwrap();
    match got {
        Ctrl::Open {
            channel: 3,
            kind: ChannelKind::Acp,
            repo: Some(repo),
            claude_token: Some(t),
            ..
        } => {
            assert_eq!(repo, "https://github.com/a/b");
            assert_eq!(t.expose(), OWNER_TOKEN);
        }
        m => panic!("{m:?}"),
    }
    // A stream larger than one frame, split by the sender, echoes back intact.
    let chunk = vec![b'x'; 200 * 1024];
    for _ in 0..4 {
        let d = Frame::data_bin(3, chunk.clone()).to_bytes().unwrap();
        ws.send(Message::Binary(d.clone())).await.unwrap();
        assert_eq!(expect_sandbox(&mut h).await, d.to_vec());
        let echo = next_bin(&mut ws).await;
        assert_eq!((echo.channel, echo.payload.len()), (3, chunk.len()));
    }
}

#[tokio::test]
async fn acp_open_without_advertised_feature_never_reaches_sandbox() {
    let mut h = harness().await;
    let (mut ws, features) = connect_ui(&h).await;
    assert!(features.is_empty());
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { channel, code, .. } => {
            assert_eq!((channel, code.as_deref()), (3, Some("unsupported_kind")));
        }
        m => panic!("{m:?}"),
    }
    // The socket survives: the pty still works.
    ws.send(Message::Binary(
        open_frame(1, ChannelKind::Pty, None).into(),
    ))
    .await
    .unwrap();
    send_echo(&mut ws).await;
    let first = expect_sandbox(&mut h).await;
    assert_eq!(first, open_frame(1, ChannelKind::Pty, None));
}

#[tokio::test]
async fn unknown_kind_gets_coded_open_error_and_socket_survives() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    let (mut ws, _) = connect_ui(&h).await;
    let raw = br#"{"type":"open","channel":4,"kind":"vnc","cols":0,"rows":0}"#;
    let f = Frame::new(FrameType::Ctrl, 0, &raw[..]).to_bytes().unwrap();
    ws.send(Message::Binary(f)).await.unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { channel, code, .. } => {
            assert_eq!((channel, code.as_deref()), (4, Some("unsupported_kind")));
        }
        m => panic!("{m:?}"),
    }
    send_echo(&mut ws).await;
    // Only the echo data reached the sandbox, not the open.
    let got = Frame::from_bytes(expect_sandbox(&mut h).await.into()).unwrap();
    assert_eq!(got.frame_type, FrameType::DataBin);
}

#[tokio::test]
async fn second_acp_open_on_a_socket_is_chat_limit() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    expect_sandbox(&mut h).await;
    ws.send(Message::Binary(
        open_frame(5, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { channel, code, .. } => {
            assert_eq!((channel, code.as_deref()), (5, Some("chat_limit")));
        }
        m => panic!("{m:?}"),
    }
    assert!(h.sandbox_rx.try_recv().is_err(), "second open leaked");

    // The sandbox closes the first (superseded): the close reason passes
    // through and a new open is accepted again.
    let bye = Frame::data_bin(3, &b"bye"[..]).to_bytes().unwrap();
    ws.send(Message::Binary(bye)).await.unwrap();
    let c = next_bin(&mut ws).await;
    assert_eq!(c.frame_type, FrameType::Close);
    assert_eq!(c.channel, 3);
    assert!(c.payload.ends_with(b"superseded"));
    expect_sandbox(&mut h).await; // the "bye" data
    ws.send(Message::Binary(
        open_frame(5, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    assert_eq!(
        expect_sandbox(&mut h).await,
        injected(open_frame(5, ChannelKind::Acp, None))
    );
}

#[tokio::test]
async fn sandbox_open_error_codes_pass_through_and_free_the_slot() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, Some("refuse")).into(),
    ))
    .await
    .unwrap();
    expect_sandbox(&mut h).await;
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { channel, code, .. } => {
            assert_eq!((channel, code.as_deref()), (3, Some("chat_disabled")));
        }
        m => panic!("{m:?}"),
    }
    let retry = open_frame(4, ChannelKind::Acp, None);
    ws.send(Message::Binary(retry.clone().into()))
        .await
        .unwrap();
    assert_eq!(expect_sandbox(&mut h).await, injected(retry));
}

#[tokio::test]
async fn oversize_repo_and_resume_are_refused_locally() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    let (mut ws, _) = connect_ui(&h).await;
    let long = "r".repeat(enzarb_proto::MAX_REPO_LEN + 1);
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, Some(&long)).into(),
    ))
    .await
    .unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { code, .. } => assert_eq!(code.as_deref(), Some("invalid_request")),
        m => panic!("{m:?}"),
    }
    assert!(h.sandbox_rx.try_recv().is_err());
}

#[tokio::test]
async fn run_control_still_rejected_with_acp_enabled() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    let (mut ws, _) = connect_ui(&h).await;
    let bytes = Frame::ctrl(&Ctrl::StepStart(Box::new(step_start())))
        .unwrap()
        .to_bytes()
        .unwrap();
    ws.send(Message::Binary(bytes)).await.unwrap();
    assert_eq!(close_of(&mut ws).await.0, 1008);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.sandbox_rx.try_recv().is_err());
}

#[tokio::test]
async fn pings_flow_and_do_not_defeat_the_idle_timeout() {
    let h = harness_with(acp_features(), |c| {
        c.idle_timeout = Duration::from_millis(800);
        c.ping_interval = Duration::from_millis(100);
    })
    .await;
    let (mut ws, _) = connect_ui(&h).await;
    let mut pings = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    let closed = loop {
        assert!(Instant::now() < deadline, "never idled out");
        match ws.next().await {
            Some(Ok(Message::Ping(_))) => pings += 1,
            Some(Ok(Message::Close(Some(c)))) => break (u16::from(c.code), c.reason.to_string()),
            Some(Ok(_)) => {}
            other => panic!("{other:?}"),
        }
    };
    // The client answers pings automatically while reading; those pongs must
    // not count as activity.
    assert!(pings >= 3, "pings: {pings}");
    assert_eq!(closed, (1001, "idle timeout".into()));
}

#[tokio::test]
async fn data_traffic_keeps_the_relay_alive_past_idle() {
    let h = harness_with(acp_features(), |c| {
        c.idle_timeout = Duration::from_millis(800);
        c.ping_interval = Duration::from_millis(100);
    })
    .await;
    let (mut ws, _) = connect_ui(&h).await;
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        send_echo(&mut ws).await;
    }
}

fn ctrl_bytes(m: &Ctrl) -> Vec<u8> {
    Frame::ctrl(m).unwrap().to_bytes().unwrap().to_vec()
}

fn data(payload: &'static [u8]) -> Message {
    Message::Binary(Frame::data_bin(1, payload).to_bytes().unwrap())
}

/// Drain the sandbox's received frames until `pred` matches one.
async fn sandbox_until(h: &mut Harness, pred: impl Fn(&Frame) -> bool) -> Vec<Frame> {
    let mut seen = Vec::new();
    loop {
        let f = Frame::from_bytes(expect_sandbox(h).await.into()).unwrap();
        let done = pred(&f);
        seen.push(f);
        if done {
            return seen;
        }
    }
}

#[tokio::test]
async fn acp_open_without_stored_token_is_no_credential_and_frees_slot() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    h.store.delete_row("ruckc");
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { channel, code, .. } => {
            assert_eq!((channel, code.as_deref()), (3, Some("no_credential")));
        }
        m => panic!("{m:?}"),
    }
    // Store a token: the retry on another channel goes through (slot freed).
    h.seed("ruckc", "later-token");
    ws.send(Message::Binary(
        open_frame(5, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    let f = Frame::from_bytes(expect_sandbox(&mut h).await.into()).unwrap();
    assert!(matches!(
        f.ctrl_message().unwrap(),
        Ctrl::Open { channel: 5, claude_token: Some(t), .. } if t.expose() == "later-token"
    ));
}

#[tokio::test]
async fn store_failure_on_open_is_answered_and_nothing_forwarded() {
    let mut h = harness_with(acp_features(), |_| {}).await;
    h.store
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::OpenError { code, .. } => {
            assert_eq!(code.as_deref(), Some("credential_unavailable"))
        }
        m => panic!("{m:?}"),
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.sandbox_rx.try_recv().is_err());
}

#[tokio::test]
async fn capture_is_stored_for_owner_and_never_reaches_browser() {
    let logs = LogBuf::default();
    let _log = tracing::subscriber::set_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(logs.clone())
            .finish(),
    );
    let mut h = harness_with(acp_features(), |_| {}).await;
    h.store.delete_row("ruckc");
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(data(b"capture")).await.unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::CredentialState {
            configured: true,
            scope: Some(s),
            ..
        } => assert_eq!(s, "user"),
        m => panic!("{m:?}"),
    }
    assert_eq!(h.stored("ruckc").as_deref(), Some(CAPTURED));
    assert_eq!(
        h.store.puts.lock().unwrap().clone(),
        vec![("ruckc".to_string(), false)]
    );
    // The sandbox got no ack for a non-legacy capture.
    sandbox_until(&mut h, |f| f.frame_type == FrameType::DataBin).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.sandbox_rx.try_recv().is_err());
    // Clear: deleted, answered, never forwarded.
    ws.send(Message::Binary(
        ctrl_bytes(&Ctrl::CredentialClear {
            kind: CredentialKind::ClaudeOauth,
        })
        .into(),
    ))
    .await
    .unwrap();
    match next_ctrl(&mut ws).await {
        Ctrl::CredentialState {
            configured: false,
            scope: Some(s),
            ..
        } => assert_eq!(s, "user"),
        m => panic!("{m:?}"),
    }
    assert!(h.stored("ruckc").is_none());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.sandbox_rx.try_recv().is_err());
    // Relay still works; no frame carried the captured token to the browser
    // (next_ctrl would have surfaced a credential_captured above).
    send_echo(&mut ws).await;
    // An acp open after clear gets no_credential (uncached read).
    ws.send(Message::Binary(
        open_frame(3, ChannelKind::Acp, None).into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        next_ctrl(&mut ws).await,
        Ctrl::OpenError { code: Some(c), .. } if c == "no_credential"
    ));
    let text = logs.text();
    assert!(text.contains("claude credential stored"), "{text}");
    for secret in [CAPTURED, OWNER_TOKEN, "CAPTURED-SECRET"] {
        assert!(!text.contains(secret), "token leaked into logs");
    }
}

#[derive(Clone, Default)]
struct LogBuf(Arc<std::sync::Mutex<Vec<u8>>>);

impl LogBuf {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for LogBuf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuf {
    type Writer = LogBuf;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn is_ack(f: &Frame) -> bool {
    f.frame_type == FrameType::Ctrl
        && matches!(
            f.ctrl_message(),
            Ok(Ctrl::CredentialState { configured: true, scope: Some(ref s), .. }) if s == "user"
        )
}

#[tokio::test]
async fn legacy_capture_is_insert_only_and_acked_to_sandbox_after_write() {
    let mut h = harness().await;
    // ruckc already has a token: a legacy capture must not overwrite it.
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(data(b"capture-legacy")).await.unwrap();
    let seen = sandbox_until(&mut h, is_ack).await;
    assert_eq!(seen.len(), 2);
    assert_eq!(h.stored("ruckc").as_deref(), Some(OWNER_TOKEN));
    assert_eq!(
        h.store.puts.lock().unwrap().clone(),
        vec![("ruckc".to_string(), true)]
    );
    // Store down: no ack, so the sandbox keeps its legacy file.
    h.store
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    ws.send(data(b"capture-legacy")).await.unwrap();
    sandbox_until(&mut h, |f| f.frame_type == FrameType::DataBin).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.sandbox_rx.try_recv().is_err());
    // The browser never sees an ack or the capture for the legacy path.
    h.store
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    send_echo(&mut ws).await;
}

#[tokio::test]
async fn browser_cannot_send_credential_captured() {
    let mut h = harness().await;
    let (mut ws, _) = connect_ui(&h).await;
    ws.send(Message::Binary(
        ctrl_bytes(&Ctrl::CredentialCaptured {
            kind: CredentialKind::ClaudeOauth,
            token: Some(enzarb_proto::Secret::new("evil")),
            legacy: None,
        })
        .into(),
    ))
    .await
    .unwrap();
    let (code, _) = close_of(&mut ws).await;
    assert_eq!(code, 1008);
    assert_eq!(h.stored("ruckc").as_deref(), Some(OWNER_TOKEN));
    assert!(h.sandbox_rx.try_recv().is_err());
}
