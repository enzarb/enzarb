#![cfg(feature = "tokio")]
//! The `acp` channel kind, `welcome.features` and `open_error.code`.

use std::time::Duration;

use bytes::Bytes;
use enzarb_proto::session::{Event, ServerOptions, Session, SessionError, handshake, memory};
use enzarb_proto::{
    ChannelKind, ClientKind, Ctrl, Frame, FrameType, MAX_REPO_LEN, MAX_RESUME_LEN, feature,
    open_error_code,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const T: Duration = Duration::from_secs(10);

fn ctrl_json(c: &Ctrl) -> Value {
    serde_json::to_value(c).unwrap()
}

fn decode(s: &str) -> Ctrl {
    Frame::new(FrameType::Ctrl, 0, Bytes::from(s.to_owned()))
        .ctrl_message()
        .unwrap()
}

fn acp_server() -> ServerOptions {
    ServerOptions {
        features: vec![feature::ACP.into()],
        ..Default::default()
    }
}

#[test]
fn features_omitted_when_absent_or_empty() {
    for f in [None, Some(vec![])] {
        let v = ctrl_json(&Ctrl::Welcome {
            version: 1,
            features: f,
        });
        assert_eq!(v, json!({"type": "welcome", "version": 1}));
    }
    let v = ctrl_json(&Ctrl::Welcome {
        version: 1,
        features: Some(vec!["acp".into()]),
    });
    assert_eq!(v["features"], json!(["acp"]));
}

#[test]
fn new_optional_fields_are_omitted_when_absent() {
    let v = ctrl_json(&Ctrl::Open {
        channel: 1,
        kind: ChannelKind::Pty,
        cols: 80,
        rows: 24,
        command: None,
        repo: None,
        resume: None,
    });
    assert_eq!(
        v,
        json!({"type":"open","channel":1,"kind":"pty","cols":80,"rows":24})
    );
    let v = ctrl_json(&Ctrl::OpenError {
        channel: 1,
        message: "m".into(),
        code: None,
    });
    assert_eq!(v, json!({"type":"open_error","channel":1,"message":"m"}));
}

#[test]
fn old_payloads_decode() {
    assert_eq!(
        decode(r#"{"type":"welcome","version":1}"#),
        Ctrl::Welcome {
            version: 1,
            features: None
        }
    );
    assert!(matches!(
        decode(r#"{"type":"open","channel":1,"kind":"pty","cols":80,"rows":24}"#),
        Ctrl::Open {
            kind: ChannelKind::Pty,
            repo: None,
            resume: None,
            ..
        }
    ));
    assert!(matches!(
        decode(r#"{"type":"open_error","channel":1,"message":"x"}"#),
        Ctrl::OpenError { code: None, .. }
    ));
    // explicit nulls and unknown fields on the new shapes
    assert_eq!(
        decode(r#"{"type":"welcome","version":1,"features":null,"future":1}"#),
        Ctrl::Welcome {
            version: 1,
            features: None
        }
    );
}

#[test]
fn unknown_kind_and_code_are_tolerated() {
    assert!(matches!(
        decode(r#"{"type":"open","channel":2,"kind":"quic","cols":0,"rows":0}"#),
        Ctrl::Open {
            kind: ChannelKind::Unknown,
            ..
        }
    ));
    let Ctrl::OpenError { code, .. } =
        decode(r#"{"type":"open_error","channel":2,"message":"m","code":"brand_new"}"#)
    else {
        panic!()
    };
    assert_eq!(code.as_deref(), Some("brand_new"));
}

#[test]
fn acp_open_round_trips() {
    let m = Ctrl::Open {
        channel: 9,
        kind: ChannelKind::Acp,
        cols: 0,
        rows: 0,
        command: None,
        repo: Some("https://github.com/enzarb/x".into()),
        resume: Some("sess_1".into()),
    };
    let back = Frame::from_bytes(Frame::ctrl(&m).unwrap().to_bytes().unwrap())
        .unwrap()
        .ctrl_message()
        .unwrap();
    assert_eq!(back, m);
    assert_eq!(ctrl_json(&m)["kind"], "acp");
}

#[tokio::test]
async fn features_reach_the_client_and_default_to_none() {
    let ((client, _ce), (server, _se)) =
        memory::session_pair_with(ClientKind::Ui, acp_server()).await;
    assert!(client.has_feature(feature::ACP));
    assert_eq!(client.features(), ["acp"]);
    assert_eq!(server.features(), ["acp"]);

    let ((client, _ce), _s) = memory::session_pair(ClientKind::Ui).await;
    assert!(client.features().is_empty());
    assert!(!client.has_feature(feature::ACP));
}

#[tokio::test]
async fn hostile_welcome_features_are_bounded() {
    let (c, mut s) = memory::pair();
    let client = tokio::spawn(Session::connect(c, ClientKind::Ui));
    // swallow hello, answer with 200 features, some empty, some huge
    let _hello = s.next().await.unwrap().unwrap();
    let mut feats: Vec<String> = (0..200).map(|i| format!("f{i}")).collect();
    feats.insert(0, String::new());
    feats.insert(1, "x".repeat(10_000));
    let w = Ctrl::Welcome {
        version: 1,
        features: Some(feats),
    };
    s.send(Message::Binary(
        Frame::ctrl(&w).unwrap().to_bytes().unwrap(),
    ))
    .await
    .unwrap();
    let (client, _ev) = timeout(T, client).await.unwrap().unwrap().unwrap();
    assert_eq!(client.features().len(), feature::MAX_FEATURES);
    assert!(
        client
            .features()
            .iter()
            .all(|f| !f.is_empty() && f.len() <= 64)
    );
}

#[tokio::test]
async fn acp_stream_round_trips_larger_than_the_window_with_backpressure() {
    let ((client, _ce), (_server, mut se)) =
        memory::session_pair_with(ClientKind::Ui, acp_server()).await;
    let c2 = client.clone();
    let opener = tokio::spawn(async move {
        c2.open_acp(
            Some("https://github.com/enzarb/x".into()),
            Some("s1".into()),
        )
        .await
        .unwrap()
    });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    assert_eq!(req.kind, ChannelKind::Acp);
    assert_eq!((req.cols, req.rows), (0, 0));
    assert_eq!(req.repo.as_deref(), Some("https://github.com/enzarb/x"));
    assert_eq!(req.resume.as_deref(), Some("s1"));
    let mut srv = req.accept().unwrap();
    let mut cli = opener.await.unwrap();

    // 2 MiB of NDJSON-ish lines from the client; the server reads slowly.
    let mut payload = Vec::new();
    let mut i = 0u32;
    while payload.len() < 2 * 1024 * 1024 {
        payload.extend_from_slice(format!("{{\"jsonrpc\":\"2.0\",\"id\":{i}}}\n").as_bytes());
        i += 1;
    }
    let want = payload.clone();
    let writer = tokio::spawn(async move {
        cli.write_all(&payload).await.unwrap();
        cli.shutdown().await.unwrap();
        cli
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!writer.is_finished(), "writer must block on credit");
    let mut got = Vec::new();
    timeout(T, srv.read_to_end(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, want);
    let _cli = writer.await.unwrap();
}

#[tokio::test]
async fn open_error_code_surfaces_to_the_client() {
    let ((client, _ce), (_server, mut se)) =
        memory::session_pair_with(ClientKind::Ui, acp_server()).await;
    let c2 = client.clone();
    let t = tokio::spawn(async move { c2.open_acp(None, None).await });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    req.reject_with_code(open_error_code::CHAT_LIMIT, "too many chats");
    let e = t.await.unwrap().err().unwrap();
    assert_eq!(e.open_error_code(), Some("chat_limit"));
    assert!(
        matches!(e, SessionError::OpenRejectedCode { ref message, .. } if message == "too many chats")
    );

    // A plain reject (old server) stays OpenRejected.
    let c2 = client.clone();
    let t = tokio::spawn(async move { c2.open_acp(None, None).await });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    req.reject("nope");
    let e = t.await.unwrap().err().unwrap();
    assert!(matches!(e, SessionError::OpenRejected(_)));
    assert_eq!(e.open_error_code(), None);
}

async fn raw_server() -> (memory::MemoryWs, Session, enzarb_proto::session::Events) {
    let (mut c, s) = memory::pair();
    let server = tokio::spawn(Session::accept(s, ServerOptions::default()));
    handshake::client_handshake(&mut c, ClientKind::Ui)
        .await
        .unwrap();
    let (session, events) = server.await.unwrap().unwrap();
    (c, session, events)
}

async fn send_ctrl(c: &mut memory::MemoryWs, json: &str) {
    let f = Frame::new(FrameType::Ctrl, 0, Bytes::from(json.to_owned()));
    c.send(Message::Binary(f.to_bytes().unwrap()))
        .await
        .unwrap();
}

async fn recv_ctrl(c: &mut memory::MemoryWs) -> Ctrl {
    loop {
        if let Message::Binary(b) = timeout(T, c.next()).await.unwrap().unwrap().unwrap() {
            return Frame::from_bytes(b).unwrap().ctrl_message().unwrap();
        }
    }
}

#[tokio::test]
async fn server_answers_unknown_kind_with_unsupported_kind_and_survives() {
    let (mut c, session, mut events) = raw_server().await;
    send_ctrl(
        &mut c,
        r#"{"type":"open","channel":4,"kind":"quic","cols":0,"rows":0}"#,
    )
    .await;
    let Ctrl::OpenError { channel, code, .. } = recv_ctrl(&mut c).await else {
        panic!()
    };
    assert_eq!(channel, 4);
    assert_eq!(code.as_deref(), Some("unsupported_kind"));
    // The id was not reserved and the connection is still usable.
    send_ctrl(
        &mut c,
        r#"{"type":"open","channel":4,"kind":"acp","cols":0,"rows":0}"#,
    )
    .await;
    let Some(Event::Open(req)) = events.next().await else {
        panic!()
    };
    assert_eq!(req.kind, ChannelKind::Acp);
    req.accept().unwrap();
    assert!(matches!(
        recv_ctrl(&mut c).await,
        Ctrl::Opened { channel: 4 }
    ));
    assert!(!session.is_closed());
}

#[tokio::test]
async fn oversized_repo_or_resume_is_refused_without_reaching_the_app() {
    let (mut c, session, mut events) = raw_server().await;
    let long = "r".repeat(MAX_REPO_LEN + 1);
    send_ctrl(
        &mut c,
        &format!(r#"{{"type":"open","channel":1,"kind":"acp","cols":0,"rows":0,"repo":"{long}"}}"#),
    )
    .await;
    let Ctrl::OpenError { code, .. } = recv_ctrl(&mut c).await else {
        panic!()
    };
    assert_eq!(code.as_deref(), Some("invalid_request"));
    let long = "s".repeat(MAX_RESUME_LEN + 1);
    send_ctrl(
        &mut c,
        &format!(
            r#"{{"type":"open","channel":1,"kind":"acp","cols":0,"rows":0,"resume":"{long}"}}"#
        ),
    )
    .await;
    assert!(matches!(
        recv_ctrl(&mut c).await,
        Ctrl::OpenError { ref code, .. } if code.as_deref() == Some("invalid_request")
    ));
    assert!(
        timeout(Duration::from_millis(100), events.next())
            .await
            .is_err(),
        "no Open event may reach the application"
    );
    assert!(!session.is_closed());

    // Client side refuses locally too.
    let ((client, _ce), _s) = memory::session_pair(ClientKind::Ui).await;
    let e = client
        .open_acp(Some("x".repeat(MAX_REPO_LEN + 1)), None)
        .await
        .err()
        .unwrap();
    assert_eq!(e.open_error_code(), Some("invalid_request"));
}
