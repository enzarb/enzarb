//! Integration tests: a real sandbox-init server on a random port, driven by
//! the enzarb-proto client.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use enzarb_proto::session::{Client, ConnectError, Event, Events, Session};
use enzarb_proto::{ClientKind, Ctrl, RunStart};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use sandbox_init::reaper::Reaper;
use sandbox_init::{AppState, Config, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::{Error as WsError, client::IntoClientRequest};

const TOKEN: &str = "test-token-123";
const T: Duration = Duration::from_secs(20);

struct Server {
    addr: SocketAddr,
    _dir: tempfile::TempDir,
}

impl Server {
    fn url(&self) -> String {
        format!("ws://{}/v1/mux", self.addr)
    }

    async fn connect(&self) -> (Session, Events) {
        timeout(T, Client::connect(&self.url(), TOKEN, ClientKind::Executor))
            .await
            .expect("connect timed out")
            .expect("connect failed")
    }
}

async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let token_file = dir.path().join("token");
    std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let config = Config {
        listen: ([127, 0, 0, 1], 0).into(),
        auth_token_file: token_file,
        workspace,
        shell: "/bin/sh".into(),
        grace: Duration::from_secs(2),
    };
    let reaper = Reaper::new(false);
    tokio::spawn(reaper.clone().run());
    let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Arc::new(AppState::new(config, reaper))));
    Server { addr, _dir: dir }
}

async fn read_until(r: &mut (impl AsyncReadExt + Unpin), needle: &str) -> String {
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        let n = timeout(T, r.read(&mut buf))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "timeout waiting for {needle:?}; got {:?}",
                    String::from_utf8_lossy(&out)
                )
            })
            .unwrap();
        if n == 0 {
            panic!(
                "eof waiting for {needle:?}; got {:?}",
                String::from_utf8_lossy(&out)
            );
        }
        out.extend_from_slice(&buf[..n]);
        let s = String::from_utf8_lossy(&out);
        if s.contains(needle) {
            return s.into_owned();
        }
    }
}

fn argv(a: &[&str]) -> Option<Vec<String>> {
    Some(a.iter().map(|s| s.to_string()).collect())
}

#[tokio::test]
async fn pty_echo_hi_then_close() {
    let srv = start().await;
    let (session, _events) = srv.connect().await;
    let pty = session
        .open_pty(80, 24, argv(&["echo", "hi"]))
        .await
        .unwrap();
    let (_send, mut recv) = pty.split();
    let out = read_until(&mut recv, "hi").await;
    assert!(out.starts_with("hi"), "{out:?}");
    // The process exited: the server closes the channel.
    let mut rest = Vec::new();
    timeout(T, recv.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    let info = recv.close_info().expect("channel closed");
    assert_eq!(info.reason, "exit:0");
    session.close().await;
}

#[tokio::test]
async fn interactive_shell_runs_in_workspace() {
    let srv = start().await;
    let (session, _events) = srv.connect().await;
    let mut pty = session.open_pty(80, 24, None).await.unwrap();
    pty.write_all(b"pwd; echo done-$((20+22))\n").await.unwrap();
    let out = read_until(&mut pty, "done-42\r\n").await;
    assert!(out.contains("/workspace"), "{out:?}");
    pty.write_all(b"exit 3\n").await.unwrap();
    let mut rest = Vec::new();
    timeout(T, pty.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pty.recv.close_info().unwrap().reason, "exit:3");
}

#[tokio::test]
async fn resize_reaches_the_pty() {
    let srv = start().await;
    let (session, _events) = srv.connect().await;
    let mut pty = session
        .open_pty(80, 24, argv(&["sh", "-c", "read x; stty size"]))
        .await
        .unwrap();
    session.resize(pty.id(), 100, 40).unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    pty.write_all(b"\n").await.unwrap();
    read_until(&mut pty, "40 100").await;
}

#[tokio::test]
async fn bad_spawn_is_open_error() {
    let srv = start().await;
    let (session, _events) = srv.connect().await;
    let err = session
        .open_pty(80, 24, argv(&["/definitely/not/a/program"]))
        .await;
    assert!(err.is_err());
    // The session is still usable.
    let pty = session
        .open_pty(80, 24, argv(&["echo", "ok"]))
        .await
        .unwrap();
    let (_s, mut r) = pty.split();
    read_until(&mut r, "ok").await;
}

#[tokio::test]
async fn concurrent_sessions() {
    let srv = start().await;
    let mut tasks = Vec::new();
    for i in 0..3 {
        let (session, events) = srv.connect().await;
        tasks.push(tokio::spawn(async move {
            let _events = events;
            let marker = format!("marker{i}");
            let pty = session
                .open_pty(80, 24, argv(&["echo", &marker]))
                .await
                .unwrap();
            let (_s, mut r) = pty.split();
            read_until(&mut r, &marker).await;
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
}

fn http_status(e: ConnectError) -> u16 {
    match e {
        ConnectError::Connect(WsError::Http(resp)) => resp.status().as_u16(),
        other => panic!("expected an http error, got {other:?}"),
    }
}

#[tokio::test]
async fn bad_or_missing_token_is_401() {
    let srv = start().await;
    let err = match Client::connect(&srv.url(), "wrong", ClientKind::Executor).await {
        Err(e) => e,
        Ok(_) => panic!("bad token was accepted"),
    };
    assert_eq!(http_status(err), 401);

    let req = srv.url().into_client_request().unwrap();
    match tokio_tungstenite::connect_async(req).await {
        Err(WsError::Http(resp)) => assert_eq!(resp.status().as_u16(), 401),
        other => panic!("expected 401, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn healthz_needs_no_auth() {
    let srv = start().await;
    let mut s = tokio::net::TcpStream::connect(srv.addr).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).await.unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.ends_with("ok"), "{resp}");
}

#[tokio::test]
async fn run_messages_are_unimplemented() {
    let srv = start().await;
    let (session, mut events) = srv.connect().await;
    let run: RunStart = serde_json::from_value(serde_json::json!({
        "run_id": "job-1", "repo_url": "https://github.com/enzarb/x", "base_ref": "main",
        "branch": "b", "push": false, "clone_token": "t", "prompt": "p", "timeout_secs": 1,
        "callback_url": "http://x", "callback_token": "t", "log_channel": null
    }))
    .unwrap();
    let msgs = [
        Ctrl::RunStart(run),
        Ctrl::RunStatus {
            run_id: "job-1".into(),
        },
        Ctrl::RunCancel {
            run_id: "job-1".into(),
        },
    ];
    for m in msgs {
        session.send_ctrl(&m).unwrap();
        match timeout(T, events.next()).await.unwrap() {
            Some(Event::Ctrl(Ctrl::Error { code, .. })) => assert_eq!(code, "unimplemented"),
            _ => panic!("expected error unimplemented for {m:?}"),
        }
    }
}

#[tokio::test]
async fn disconnect_kills_that_sessions_ptys_only() {
    let srv = start().await;
    let (s1, _e1) = srv.connect().await;
    let (s2, _e2) = srv.connect().await;
    let script = "echo pid=$$; exec sleep 300";
    let mut pids = Vec::new();
    let mut handles = Vec::new();
    for s in [&s1, &s2] {
        let mut pty = s
            .open_pty(80, 24, argv(&["sh", "-c", script]))
            .await
            .unwrap();
        let out = read_until(&mut pty, "\n").await;
        let pid: i32 = out.trim().strip_prefix("pid=").unwrap().parse().unwrap();
        pids.push(pid);
        handles.push(pty);
    }
    s1.close().await;
    drop(handles.remove(0));
    let gone = |pid: i32| matches!(kill(Pid::from_raw(pid), None), Err(Errno::ESRCH));
    timeout(T, async {
        while !gone(pids[0]) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("session 1's pty process still alive");
    assert!(!gone(pids[1]), "session 2's pty was killed");
    s2.close().await;
    timeout(T, async {
        while !gone(pids[1]) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("session 2's pty process still alive");
}
