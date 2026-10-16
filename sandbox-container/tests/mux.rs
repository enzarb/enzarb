//! Integration tests: a real sandbox-init server on a random port, driven by
//! the enzarb-proto client.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use enzarb_proto::session::{Client, ConnectError, Event, Events, Session};
use enzarb_proto::{ClientKind, CredentialKind, Ctrl};
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
    start_with(|_| {}).await
}

async fn start_with(tweak: impl FnOnce(&mut Config)) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let token_file = dir.path().join("token");
    std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let mut config = Config {
        listen: ([127, 0, 0, 1], 0).into(),
        auth_token_file: token_file,
        workspace,
        shell: "/bin/sh".into(),
        grace: Duration::from_secs(2),
        ..Config::default()
    };
    tweak(&mut config);
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
    assert!(resp.ends_with(r#"{"status":"ok"}"#), "{resp}");
}

#[tokio::test]
async fn healthz_reports_docker_disk_and_prune_additively() {
    use sandbox_init::docker_prune::{ActivityGate, PruneConfig, Pruner};
    use sandbox_init::dockerd::{Dockerd, DockerdConfig};

    let dir = tempfile::tempdir().unwrap();
    let reaper = Reaper::new(false);
    tokio::spawn(reaper.clone().run());
    let dockerd = Dockerd::new(
        DockerdConfig {
            enabled: false,
            ..DockerdConfig::default()
        },
        reaper.clone(),
    );
    let prune = PruneConfig {
        data_root: dir.path().to_path_buf(),
        ..PruneConfig::default()
    };
    let pruner = Pruner::new(
        prune,
        dockerd.subscribe(),
        ActivityGate::default(),
        reaper.clone(),
    );
    let state = AppState::new(Config::default(), reaper)
        .with_dockerd(dockerd)
        .with_pruner(pruner);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Arc::new(state)));

    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).await.unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body: serde_json::Value =
        serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["dockerd"], serde_json::json!({"state": "disabled"}));
    let disk = &body["docker_disk"];
    assert_eq!(disk["path"], dir.path().display().to_string());
    for k in ["total_bytes", "used_bytes", "free_bytes"] {
        assert!(disk[k].is_u64(), "{k}: {body}");
    }
    assert!(disk["free_percent"].is_number(), "{body}");
    assert!(disk["low_disk"].is_boolean(), "{body}");
    assert_eq!(
        body["docker_prune"],
        serde_json::json!({"enabled": true, "last": null})
    );
}

#[tokio::test]
async fn steps_are_advertised_and_unknown_runs_answered() {
    let srv = start().await;
    let (session, mut events) = srv.connect().await;
    assert!(session.has_feature(enzarb_proto::feature::STEPS));
    session
        .send_ctrl(&Ctrl::StepStatus {
            run_id: "nope".into(),
            after_seq: 0,
        })
        .unwrap();
    match timeout(T, events.next()).await.unwrap() {
        Some(Event::Ctrl(Ctrl::StepState { state, .. })) => {
            assert_eq!(state, enzarb_proto::StepPhase::Unknown)
        }
        _ => panic!("expected step_state"),
    }
    session
        .send_ctrl(&Ctrl::StepStop {
            run_id: "nope".into(),
        })
        .unwrap();
    match timeout(T, events.next()).await.unwrap() {
        Some(Event::Ctrl(Ctrl::Error { code, .. })) => assert_eq!(code, "unknown_run"),
        _ => panic!("expected error"),
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

#[tokio::test]
async fn credential_status_reflects_token_file() {
    let tok = tempfile::tempdir().unwrap();
    let file = tok.path().join("claude-token");
    let f = file.clone();
    let srv = start_with(move |c| c.claude_token_file = f).await;
    let (session, mut events) = srv.connect().await;

    async fn ask(session: &Session, events: &mut Events) -> bool {
        session.send_ctrl(&Ctrl::CredentialStatus).unwrap();
        loop {
            match timeout(T, events.next())
                .await
                .expect("no credential_state")
            {
                Some(Event::Ctrl(Ctrl::CredentialState {
                    kind, configured, ..
                })) => {
                    assert_eq!(kind, CredentialKind::ClaudeOauth);
                    return configured;
                }
                Some(_) => continue,
                None => panic!("session closed"),
            }
        }
    }

    assert!(!ask(&session, &mut events).await, "missing file");
    std::fs::write(&file, "").unwrap();
    assert!(!ask(&session, &mut events).await, "empty file");
    std::fs::write(&file, "not-a-token\n").unwrap();
    assert!(!ask(&session, &mut events).await, "wrong prefix");
    std::fs::write(&file, "sk-ant-oat01-abc\n").unwrap();
    assert!(ask(&session, &mut events).await, "valid token");
}

const LEGACY: &str = "sk-ant-oat01-LEGACYtoken";

async fn next_captured(events: &mut Events) -> (Option<String>, Option<bool>) {
    loop {
        match timeout(T, events.next())
            .await
            .expect("no credential_captured")
        {
            Some(Event::Ctrl(Ctrl::CredentialCaptured { token, legacy, .. })) => {
                return (token.map(|t| t.expose().to_string()), legacy);
            }
            Some(_) => continue,
            None => panic!("session closed"),
        }
    }
}

async fn user_token_client(srv: &Server, kind: ClientKind) -> (Session, Events) {
    timeout(
        T,
        Client::connect_offering(
            &srv.url(),
            TOKEN,
            kind,
            vec![enzarb_proto::feature::CLAUDE_USER_TOKEN.into()],
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

fn ack(session: &Session) {
    session
        .send_ctrl(&Ctrl::CredentialState {
            kind: CredentialKind::ClaudeOauth,
            configured: true,
            scope: Some("user".into()),
        })
        .unwrap();
}

#[tokio::test]
async fn legacy_token_is_promoted_and_deleted_only_after_the_ack() {
    let tok = tempfile::tempdir().unwrap();
    let file = tok.path().join("claude-token");
    std::fs::write(&file, format!("{LEGACY}\n")).unwrap();
    let f = file.clone();
    let srv = start_with(move |c| c.claude_token_file = f).await;

    // Neither an executor nor an old proxy (no feature) ever gets the token,
    // and an ack from them deletes nothing.
    for kind in [ClientKind::Executor, ClientKind::Proxy, ClientKind::Ui] {
        let (s, mut ev) = timeout(T, Client::connect(&srv.url(), TOKEN, kind))
            .await
            .unwrap()
            .unwrap();
        assert!(
            timeout(Duration::from_millis(500), next_captured(&mut ev))
                .await
                .is_err(),
            "token promoted to {kind:?}"
        );
        ack(&s);
    }
    // A non-proxy offering the feature is not trusted with it either.
    let (_s, mut ev) = user_token_client(&srv, ClientKind::Ui).await;
    assert!(
        timeout(Duration::from_millis(500), next_captured(&mut ev))
            .await
            .is_err()
    );
    assert!(file.exists());

    // A new proxy that never acks (proxy down mid-way) leaves the file.
    let (s1, mut e1) = user_token_client(&srv, ClientKind::Proxy).await;
    assert_eq!(
        next_captured(&mut e1).await,
        (Some(LEGACY.into()), Some(true))
    );
    s1.close().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(file.exists(), "deleted without an ack");

    // The next proxy session promotes again; an ack deletes the file.
    let (s2, mut e2) = user_token_client(&srv, ClientKind::Proxy).await;
    assert_eq!(next_captured(&mut e2).await.1, Some(true));
    ack(&s2);
    timeout(T, async {
        while file.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("legacy file not deleted after the ack");
}

#[tokio::test]
async fn legacy_file_changed_after_promotion_is_kept() {
    let tok = tempfile::tempdir().unwrap();
    let file = tok.path().join("claude-token");
    std::fs::write(&file, LEGACY).unwrap();
    let f = file.clone();
    let srv = start_with(move |c| c.claude_token_file = f).await;
    let (s, mut e) = user_token_client(&srv, ClientKind::Proxy).await;
    next_captured(&mut e).await;
    std::fs::write(&file, "sk-ant-oat01-NEWER").unwrap();
    ack(&s);
    // A credential_status round trip orders after the ack.
    s.send_ctrl(&Ctrl::CredentialStatus).unwrap();
    loop {
        match timeout(T, e.next()).await.unwrap() {
            Some(Event::Ctrl(Ctrl::CredentialState { configured, .. })) => {
                assert!(configured);
                break;
            }
            Some(_) => continue,
            None => panic!("closed"),
        }
    }
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "sk-ant-oat01-NEWER"
    );
}

async fn ask_stats(session: &Session, events: &mut Events) -> enzarb_proto::SandboxStats {
    session.send_ctrl(&Ctrl::StatsRequest).unwrap();
    loop {
        match timeout(T, events.next()).await.expect("no stats") {
            Some(Event::Ctrl(Ctrl::Stats(s))) => return s,
            Some(_) => continue,
            None => panic!("closed"),
        }
    }
}

#[tokio::test]
async fn stats_request_reads_cgroup_files_on_demand() {
    let cg = tempfile::tempdir().unwrap();
    std::fs::write(cg.path().join("cpu.stat"), "usage_usec 11\nuser_usec 7\n").unwrap();
    std::fs::write(cg.path().join("memory.current"), "4096\n").unwrap();
    let root = cg.path().to_path_buf();
    let srv = start_with(move |c| c.cgroup_root = root).await;
    let (session, mut events) = srv.connect().await;
    assert!(
        session
            .features()
            .iter()
            .any(|f| f == enzarb_proto::feature::STATS)
    );

    let s = ask_stats(&session, &mut events).await;
    assert!(s.ts_ms > 1_700_000_000_000);
    assert_eq!(s.cpu_usage_usec, Some(11));
    assert_eq!(s.cpu_user_usec, Some(7));
    assert_eq!(s.cpu_system_usec, None);
    assert_eq!(s.memory_bytes, Some(4096));
    assert_eq!(s.io_read_bytes, None);
    assert!(s.storage_total_bytes.is_some_and(|t| t > 0));

    // Counters are read per request, not cached.
    std::fs::write(cg.path().join("memory.current"), "max\n").unwrap();
    assert_eq!(ask_stats(&session, &mut events).await.memory_bytes, None);
    session.close().await;
}

#[tokio::test]
async fn stats_request_without_cgroup_files_still_replies() {
    let srv = start_with(|c| c.cgroup_root = "/nonexistent/cgroup".into()).await;
    let (session, mut events) = srv.connect().await;
    let s = ask_stats(&session, &mut events).await;
    assert!(s.ts_ms > 0);
    assert_eq!(s.cpu_usage_usec, None);
    assert_eq!(s.memory_bytes, None);
    // The session is unaffected: a pty still works.
    let pty = session
        .open_pty(80, 24, argv(&["echo", "ok"]))
        .await
        .unwrap();
    let (_send, mut recv) = pty.split();
    read_until(&mut recv, "ok").await;
    session.close().await;
}
