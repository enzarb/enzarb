#![cfg(feature = "tokio")]
//! Integration tests: client and server sessions over an in-memory websocket.

use std::time::Duration;

use bytes::Bytes;
use enzarb_proto::session::{
    Event, HandshakeError, ServerOptions, Session, SessionEnd, SessionError, handshake, memory,
};
use enzarb_proto::{
    ChannelKind, ClientKind, Ctrl, Frame, FrameType, INITIAL_WINDOW, RunPhase, RunStart, close_code,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

const T: Duration = Duration::from_secs(5);

fn run_start(id: &str, log: Option<u32>) -> RunStart {
    RunStart {
        run_id: id.into(),
        repo_url: "https://example.com/r".into(),
        base_ref: "main".into(),
        branch: "b".into(),
        push: false,
        clone_token: "t".into(),
        prompt: "p".into(),
        timeout_secs: 60,
        callback_url: "http://cb".into(),
        callback_token: "cb".into(),
        log_channel: log,
    }
}

/// Server that accepts every open as an echo pty and answers run_* messages.
fn spawn_echo_server(session: Session, mut events: enzarb_proto::session::Events) {
    tokio::spawn(async move {
        while let Some(ev) = events.next().await {
            match ev {
                Event::Open(req) => {
                    let mut ch = req.accept().unwrap();
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        loop {
                            let n = ch.read(&mut buf).await.unwrap_or(0);
                            if n == 0 || ch.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    });
                }
                Event::Ctrl(Ctrl::RunStart(run)) => {
                    if run.run_id == "dup" {
                        session.send_error("internal", "refusing dup").unwrap();
                        continue;
                    }
                    let log = session
                        .accept_run(&run.run_id, RunPhase::Running, false, run.log_channel)
                        .unwrap();
                    if let Some(mut log) = log {
                        tokio::spawn(async move {
                            for i in 0..3 {
                                log.send.send_json(&json!({"line": i})).await.unwrap();
                            }
                            log.send.close(0, "done").unwrap();
                        });
                    }
                }
                Event::Ctrl(Ctrl::RunStatus { run_id }) => {
                    if run_id == "unimpl" {
                        session
                            .send_error("unimplemented", "no run manager")
                            .unwrap();
                    } else if run_id == "nope" {
                        session.send_error("unknown_run", "no such run").unwrap();
                    } else {
                        session
                            .send_run_state(&enzarb_proto::session::RunInfo {
                                run_id,
                                state: RunPhase::Succeeded,
                                exit_code: Some(0),
                                summary: Some("ok".into()),
                            })
                            .unwrap();
                    }
                }
                _ => {}
            }
        }
    });
}

#[tokio::test]
async fn handshake_roles_and_version() {
    let ((client, _ce), (server, _se)) = memory::session_pair(ClientKind::Proxy).await;
    assert_eq!(client.version(), 1);
    assert_eq!(server.version(), 1);
    assert_eq!(server.peer_kind(), Some(ClientKind::Proxy));
    assert_eq!(client.peer_kind(), None);
}

#[tokio::test]
async fn no_common_version_is_refused() {
    let (c, s) = memory::pair();
    let server = Session::accept(
        s,
        ServerOptions {
            supported_versions: vec![7],
        },
    );
    let client = Session::connect(c, ClientKind::Ui);
    let (client, server) = timeout(T, async { tokio::join!(client, server) })
        .await
        .unwrap();
    assert!(matches!(
        server,
        Err(HandshakeError::UnsupportedVersion { .. })
    ));
    assert!(matches!(
        client,
        Err(HandshakeError::Rejected { ref code, .. }) if code == "unsupported_version"
    ));
}

#[tokio::test]
async fn first_frame_must_be_hello() {
    let (mut c, s) = memory::pair();
    let server = tokio::spawn(Session::accept(s, ServerOptions::default()));
    let f = Frame::window(1, 10).to_bytes().unwrap();
    c.send(Message::Binary(f)).await.unwrap();
    let reply = timeout(T, c.next()).await.unwrap().unwrap().unwrap();
    let Message::Binary(b) = reply else { panic!() };
    let m = Frame::from_bytes(b).unwrap().ctrl_message().unwrap();
    assert!(matches!(m, Ctrl::Error { ref code, .. } if code == "protocol_violation"));
    assert!(matches!(
        server.await.unwrap(),
        Err(HandshakeError::Protocol(_))
    ));
    // The server dropped the connection.
    assert!(timeout(T, c.next()).await.unwrap().is_none());
}

#[tokio::test]
async fn open_several_channels_and_echo() {
    let ((client, _ce), (server, se)) = memory::session_pair(ClientKind::Ui).await;
    spawn_echo_server(server, se);
    let mut chans = Vec::new();
    for _ in 0..4 {
        chans.push(client.open_pty(80, 24, None).await.unwrap());
    }
    let ids: std::collections::HashSet<_> = chans.iter().map(|c| c.id()).collect();
    assert_eq!(ids.len(), 4);
    for (i, ch) in chans.iter_mut().enumerate() {
        let msg = format!("hello {i}");
        ch.write_all(msg.as_bytes()).await.unwrap();
        let mut buf = vec![0u8; msg.len()];
        timeout(T, ch.read_exact(&mut buf)).await.unwrap().unwrap();
        assert_eq!(buf, msg.as_bytes());
    }
}

#[tokio::test]
async fn saturated_channel_does_not_block_others() {
    let ((client, _ce), (server, mut se)) = memory::session_pair(ClientKind::Ui).await;
    let _keep = server.clone();
    let slow_task = tokio::spawn({
        let client = client.clone();
        async move { client.open_pty(80, 24, None).await.unwrap() }
    });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    let mut slow_srv = req.accept().unwrap();
    let mut slow = slow_task.await.unwrap();

    let fast_task = tokio::spawn({
        let client = client.clone();
        async move { client.open_pty(80, 24, None).await.unwrap() }
    });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    let mut fast_srv = req.accept().unwrap();
    let mut fast = fast_task.await.unwrap();

    // Writer on the slow channel: the server never reads, so it must stall at
    // exactly the initial window rather than drop or buffer without bound.
    let total = INITIAL_WINDOW as usize * 3;
    let payload: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();
    let writer = tokio::spawn({
        let payload = payload.clone();
        async move {
            slow.write_all(&payload).await.unwrap();
            slow
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !writer.is_finished(),
        "writer must be blocked by flow control"
    );
    assert_eq!(slow_srv.send.credit(), INITIAL_WINDOW as u64); // our own direction is untouched

    // The other channel still makes progress while the first is saturated.
    for _ in 0..20 {
        fast.write_all(b"ping").await.unwrap();
        let mut b = [0u8; 4];
        timeout(T, fast_srv.read_exact(&mut b))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&b, b"ping");
        fast_srv.write_all(b"pong").await.unwrap();
        timeout(T, fast.read_exact(&mut b)).await.unwrap().unwrap();
        assert_eq!(&b, b"pong");
    }
    assert!(!writer.is_finished());

    // Draining the slow channel releases credit; everything arrives in order.
    let mut got = vec![0u8; total];
    timeout(T, slow_srv.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, payload);
    timeout(T, writer).await.unwrap().unwrap();
}

#[tokio::test]
async fn json_log_channel_via_run_start() {
    let ((client, _ce), (server, se)) = memory::session_pair(ClientKind::Executor).await;
    spawn_echo_server(server, se);

    let started = client
        .run_start_logged(run_start("job-1", None))
        .await
        .unwrap();
    assert_eq!(started.accepted.run_id, "job-1");
    assert_eq!(started.accepted.state, RunPhase::Running);
    assert!(!started.accepted.existing);
    let mut log = started.log.expect("log channel");
    for i in 0..3 {
        let v: serde_json::Value = timeout(T, log.recv.recv_json())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(v, json!({"line": i}));
    }
    assert!(timeout(T, log.recv.recv()).await.unwrap().is_none());
    assert_eq!(log.recv.close_info().unwrap().reason, "done");

    // Client-chosen id, and no log channel at all.
    let s = client
        .run_start(run_start("job-2", Some(900)))
        .await
        .unwrap();
    assert_eq!(s.log.unwrap().id(), 900);
    let s = client.run_start(run_start("job-3", None)).await.unwrap();
    assert!(s.log.is_none());

    let info = client.run_status("job-1").await.unwrap();
    assert_eq!(info.state, RunPhase::Succeeded);
    assert_eq!(info.exit_code, Some(0));
    assert_eq!(info.summary.as_deref(), Some("ok"));
}

#[tokio::test]
async fn error_replies_are_attributed_to_requests() {
    let ((client, _ce), (server, se)) = memory::session_pair(ClientKind::Executor).await;
    spawn_echo_server(server, se);
    let e = client.run_status("nope").await.unwrap_err();
    assert!(matches!(e, SessionError::Remote { ref code, .. } if code == "unknown_run"));
    // A failed run_start releases its reserved log channel id.
    let e = client
        .run_start(run_start("dup", Some(77)))
        .await
        .err()
        .unwrap();
    assert!(matches!(e, SessionError::Remote { ref code, .. } if code == "internal"));
    let held = client.run_start(run_start("ok", Some(77))).await.unwrap();
    // The same id twice is refused locally.
    assert!(matches!(
        client
            .run_start(run_start("again", Some(77)))
            .await
            .err()
            .unwrap(),
        SessionError::ChannelInUse(77)
    ));
    drop(held);
}

#[tokio::test]
async fn unimplemented_error_fails_pending_run_request() {
    let ((client, _ce), (server, se)) = memory::session_pair(ClientKind::Executor).await;
    spawn_echo_server(server, se);
    let e = timeout(T, client.run_status("unimpl"))
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(e, SessionError::Remote { ref code, .. } if code == "unimplemented"));
}

#[tokio::test]
async fn error_run_id_beats_oldest_pending() {
    let ((client, _ce), (server, mut se)) = memory::session_pair(ClientKind::Executor).await;
    tokio::spawn(async move {
        let mut seen = 0;
        while let Some(ev) = se.next().await {
            if let Event::Ctrl(Ctrl::RunStatus { run_id }) = ev {
                seen += 1;
                if seen == 2 {
                    // Fail the *newer* request by run_id, then answer the older.
                    server
                        .send_run_error(&run_id, "weird_code", "nope")
                        .unwrap();
                    server
                        .send_run_state(&enzarb_proto::session::RunInfo {
                            run_id: "a".into(),
                            state: RunPhase::Running,
                            exit_code: None,
                            summary: None,
                        })
                        .unwrap();
                }
            }
        }
    });
    let (a, b) = timeout(T, async {
        tokio::join!(client.run_status("a"), client.run_status("b"))
    })
    .await
    .unwrap();
    assert_eq!(a.unwrap().state, RunPhase::Running);
    assert!(
        matches!(b.unwrap_err(), SessionError::Remote { ref code, .. } if code == "weird_code")
    );
}

#[tokio::test]
async fn open_rejected_and_duplicate_ids() {
    let ((client, _ce), (server, mut se)) = memory::session_pair(ClientKind::Ui).await;
    let _keep = server;
    let c2 = client.clone();
    let t = tokio::spawn(async move { c2.open_pty(80, 24, None).await });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    assert_eq!(req.kind, ChannelKind::Pty);
    assert_eq!((req.cols, req.rows), (80, 24));
    req.reject("no capacity");
    let e = t.await.unwrap().err().unwrap();
    assert!(matches!(e, SessionError::OpenRejected(ref m) if m == "no capacity"));

    // Dropping the request undecided also rejects.
    let c2 = client.clone();
    let t = tokio::spawn(async move { c2.open_pty(80, 24, None).await });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    drop(req);
    assert!(matches!(
        t.await.unwrap().err().unwrap(),
        SessionError::OpenRejected(_)
    ));
}

/// Drive the server with raw frames from a hand-rolled client.
async fn raw_client() -> (memory::MemoryWs, Session, enzarb_proto::session::Events) {
    let (mut c, s) = memory::pair();
    let server = tokio::spawn(Session::accept(s, ServerOptions::default()));
    handshake::client_handshake(&mut c, ClientKind::Ui)
        .await
        .unwrap();
    let (session, events) = server.await.unwrap().unwrap();
    (c, session, events)
}

async fn send(c: &mut memory::MemoryWs, f: Frame) {
    c.send(Message::Binary(f.to_bytes().unwrap()))
        .await
        .unwrap();
}

async fn recv(c: &mut memory::MemoryWs) -> Frame {
    loop {
        match timeout(T, c.next())
            .await
            .unwrap()
            .expect("closed")
            .unwrap()
        {
            Message::Binary(b) => return Frame::from_bytes(b).unwrap(),
            _ => continue,
        }
    }
}

async fn recv_ctrl(c: &mut memory::MemoryWs) -> Ctrl {
    recv(c).await.ctrl_message().unwrap()
}

fn open_msg(channel: u32) -> Frame {
    Frame::ctrl(&Ctrl::Open {
        channel,
        kind: ChannelKind::Pty,
        cols: 80,
        rows: 24,
        command: None,
    })
    .unwrap()
}

#[tokio::test]
async fn server_enforces_channel_table_rules() {
    let (mut c, _session, mut events) = raw_client().await;
    send(&mut c, open_msg(5)).await;
    let Some(Event::Open(req)) = events.next().await else {
        panic!()
    };
    // Id is reserved while the request is pending.
    send(&mut c, open_msg(5)).await;
    assert!(matches!(
        recv_ctrl(&mut c).await,
        Ctrl::OpenError { channel: 5, .. }
    ));
    let _ch = req.accept().unwrap();
    assert!(matches!(
        recv_ctrl(&mut c).await,
        Ctrl::Opened { channel: 5 }
    ));
    // In use after accept; channel 0 is never valid.
    send(&mut c, open_msg(5)).await;
    assert!(matches!(
        recv_ctrl(&mut c).await,
        Ctrl::OpenError { channel: 5, .. }
    ));
    send(&mut c, open_msg(0)).await;
    assert!(matches!(
        recv_ctrl(&mut c).await,
        Ctrl::OpenError { channel: 0, .. }
    ));
    // Data and resize for unknown channels get error replies and the session survives.
    send(&mut c, Frame::data_bin(9, Bytes::from_static(b"x"))).await;
    assert!(
        matches!(recv_ctrl(&mut c).await, Ctrl::Error { ref code, .. } if code == "unknown_channel")
    );
    send(
        &mut c,
        Frame::ctrl(&Ctrl::Resize {
            channel: 9,
            cols: 1,
            rows: 1,
        })
        .unwrap(),
    )
    .await;
    assert!(
        matches!(recv_ctrl(&mut c).await, Ctrl::Error { ref code, .. } if code == "unknown_channel")
    );
    // A second hello is a violation, but not fatal.
    send(
        &mut c,
        Frame::ctrl(&Ctrl::Hello {
            versions: vec![1],
            client: ClientKind::Ui,
        })
        .unwrap(),
    )
    .await;
    assert!(
        matches!(recv_ctrl(&mut c).await, Ctrl::Error { ref code, .. } if code == "protocol_violation")
    );
    // Server-bound message from the wrong side.
    send(&mut c, Frame::ctrl(&Ctrl::Opened { channel: 5 }).unwrap()).await;
    assert!(
        matches!(recv_ctrl(&mut c).await, Ctrl::Error { ref code, .. } if code == "protocol_violation")
    );
    // Resize on an open channel is delivered.
    send(
        &mut c,
        Frame::ctrl(&Ctrl::Resize {
            channel: 5,
            cols: 100,
            rows: 50,
        })
        .unwrap(),
    )
    .await;
    assert!(matches!(
        events.next().await,
        Some(Event::Ctrl(Ctrl::Resize {
            channel: 5,
            cols: 100,
            rows: 50
        }))
    ));
}

#[tokio::test]
async fn unknown_ctrl_type_gets_unknown_message() {
    let (mut c, _session, _events) = raw_client().await;
    let f = Frame::new(
        FrameType::Ctrl,
        0,
        Bytes::from_static(br#"{"type":"teleport"}"#),
    );
    send(&mut c, f).await;
    assert!(
        matches!(recv_ctrl(&mut c).await, Ctrl::Error { ref code, .. } if code == "unknown_message")
    );
}

#[tokio::test]
async fn credit_overrun_aborts_only_that_channel() {
    let (mut c, session, mut events) = raw_client().await;
    for id in [1, 2] {
        send(&mut c, open_msg(id)).await;
        let Some(Event::Open(req)) = events.next().await else {
            panic!()
        };
        std::mem::forget(req.accept().unwrap());
        assert!(matches!(recv_ctrl(&mut c).await, Ctrl::Opened { .. }));
    }
    // 17 frames of 16 KiB = 272 KiB > 256 KiB window.
    for _ in 0..17 {
        send(&mut c, Frame::data_bin(1, vec![0u8; 16 * 1024])).await;
    }
    let mut saw_error = false;
    let mut saw_close = false;
    while !(saw_error && saw_close) {
        let f = recv(&mut c).await;
        match f.frame_type {
            FrameType::Ctrl => {
                saw_error = matches!(f.ctrl_message().unwrap(), Ctrl::Error { ref code, .. } if code == "flow_control");
            }
            FrameType::Close => {
                assert_eq!(f.channel, 1);
                assert_eq!(f.close_info().unwrap().code, close_code::PROTOCOL_ERROR);
                saw_close = true;
            }
            _ => {}
        }
    }
    // Channel 2 and the session are fine.
    send(&mut c, Frame::data_bin(2, Bytes::from_static(b"still ok"))).await;
    assert!(!session.is_closed());
}

#[tokio::test]
async fn data_after_peer_close_is_a_violation() {
    let (mut c, _session, mut events) = raw_client().await;
    send(&mut c, open_msg(3)).await;
    let Some(Event::Open(req)) = events.next().await else {
        panic!()
    };
    let mut ch = req.accept().unwrap();
    recv_ctrl(&mut c).await;
    send(&mut c, Frame::close(3, 0, "")).await;
    send(&mut c, Frame::data_bin(3, Bytes::from_static(b"late"))).await;
    // Reader sees EOF, writer side stays usable (half-close).
    assert_eq!(
        timeout(T, ch.read(&mut [0u8; 4])).await.unwrap().unwrap(),
        0
    );
    loop {
        let f = recv(&mut c).await;
        if f.frame_type == FrameType::Ctrl {
            assert!(
                matches!(f.ctrl_message().unwrap(), Ctrl::Error { ref code, .. } if code == "flow_control")
            );
            break;
        }
    }
}

#[tokio::test]
async fn auto_window_returns_credit_on_read() {
    let (mut c, _session, mut events) = raw_client().await;
    send(&mut c, open_msg(1)).await;
    let Some(Event::Open(req)) = events.next().await else {
        panic!()
    };
    let mut ch = req.accept().unwrap();
    recv_ctrl(&mut c).await;
    // Send 192 KiB; nothing is granted back until the app reads.
    for _ in 0..12 {
        send(&mut c, Frame::data_bin(1, vec![7u8; 16 * 1024])).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        timeout(Duration::from_millis(100), c.next()).await.is_err(),
        "no WINDOW before read"
    );
    let mut buf = vec![0u8; 192 * 1024];
    ch.read_exact(&mut buf).await.unwrap();
    let f = recv(&mut c).await;
    assert_eq!(f.frame_type, FrameType::Window);
    assert_eq!(f.channel, 1);
    // Grants are batched: once half the initial window has been consumed.
    assert_eq!(f.window_credit().unwrap(), INITIAL_WINDOW / 2);
}

#[tokio::test]
async fn text_message_is_fatal() {
    let (mut c, session, mut events) = raw_client().await;
    c.send(Message::Text("hi".into())).await.unwrap();
    assert!(
        matches!(recv_ctrl(&mut c).await, Ctrl::Error { ref code, .. } if code == "protocol_violation")
    );
    assert!(matches!(session.closed().await, SessionEnd::Protocol(_)));
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn bad_frame_is_fatal() {
    let (mut c, session, _events) = raw_client().await;
    c.send(Message::Binary(Bytes::from_static(&[
        9, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ])))
    .await
    .unwrap();
    assert!(matches!(session.closed().await, SessionEnd::Protocol(_)));
}

#[tokio::test]
async fn half_close_and_clean_session_close() {
    let ((client, _ce), (server, se)) = memory::session_pair(ClientKind::Ui).await;
    let server2 = server.clone();
    spawn_echo_server(server, se);
    let mut ch = client.open_pty(80, 24, None).await.unwrap();
    ch.write_all(b"abc").await.unwrap();
    // Half close: the echo task sees EOF, flushes, then closes its side.
    ch.shutdown().await.unwrap();
    let mut out = Vec::new();
    timeout(T, ch.read_to_end(&mut out)).await.unwrap().unwrap();
    assert_eq!(out, b"abc");
    // Writing after our CLOSE fails.
    assert!(ch.write_all(b"x").await.is_err());

    // Closing the session fails open channels and ends the peer.
    let mut ch2 = client.open_pty(80, 24, None).await.unwrap();
    assert_eq!(client.close().await, SessionEnd::Closed);
    assert!(ch2.write_all(b"x").await.is_err() || ch2.read(&mut [0u8; 1]).await.is_err());
    assert_eq!(
        timeout(T, server2.closed()).await.unwrap(),
        SessionEnd::PeerClosed
    );
    assert!(matches!(
        client.open_pty(1, 1, None).await.err().unwrap(),
        SessionError::Ended(_)
    ));
}

#[tokio::test]
async fn dropped_sender_sends_close_and_dropped_receiver_discards() {
    let ((client, _ce), (server, mut se)) = memory::session_pair(ClientKind::Ui).await;
    let _keep = server;
    let c2 = client.clone();
    let t = tokio::spawn(async move { c2.open_pty(80, 24, None).await.unwrap() });
    let Some(Event::Open(req)) = se.next().await else {
        panic!()
    };
    let mut srv = req.accept().unwrap();
    let cli = t.await.unwrap();
    let (cs, cr) = cli.split();
    drop(cr); // client no longer reads: server output is discarded, not stalled
    let big = vec![1u8; INITIAL_WINDOW as usize * 2];
    timeout(T, srv.write_all(&big)).await.unwrap().unwrap();
    drop(cs); // CLOSE
    assert_eq!(
        timeout(T, srv.read(&mut [0u8; 1])).await.unwrap().unwrap(),
        0
    );
    assert_eq!(srv.recv.close_info().unwrap().code, close_code::NORMAL);
}

#[tokio::test]
async fn dropping_last_session_closes_the_connection() {
    let ((client, ce), (server, mut se)) = memory::session_pair(ClientKind::Ui).await;
    drop(ce);
    drop(client);
    assert_eq!(
        timeout(T, server.closed()).await.unwrap(),
        SessionEnd::PeerClosed
    );
    assert!(se.next().await.is_none());
}
