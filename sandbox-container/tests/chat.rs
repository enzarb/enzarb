//! End-to-end tests of the chat relay: a real sandbox-init server, a fake ACP
//! agent (`fixtures/fake-chat-agent.mjs`, run with node) and the enzarb-proto
//! client's `open_acp` over a real websocket.

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use enzarb_proto::session::{ChannelHandle, Client, Events, Session, SessionError};
use enzarb_proto::{ClientKind, open_error_code};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use sandbox_init::chat::{ChatManager, PASS_ENV};
use sandbox_init::config::ChatAgent;
use sandbox_init::reaper::Reaper;
use sandbox_init::repos::repo_dir_name;
use sandbox_init::{AppState, ChatMode, Config, serve};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::timeout;

const AUTH: &str = "test-auth-token-123456";
const CLAUDE: &str = "sk-ant-oat01-FAKEclaudeTOKEN_abcdefghijklmnop";
const T: Duration = Duration::from_secs(20);
const FAKE_AGENT: &str = include_str!("fixtures/fake-chat-agent.mjs");

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_out(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    )
}

struct Server {
    addr: SocketAddr,
    chats: Arc<ChatManager>,
    ws: PathBuf,
    repo_url: String,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn connect(&self) -> (Session, Events) {
        timeout(
            T,
            Client::connect(&format!("ws://{}/v1/mux", self.addr), AUTH, ClientKind::Ui),
        )
        .await
        .expect("connect timed out")
        .expect("connect failed")
    }

    fn clone_dir(&self) -> PathBuf {
        self.ws.join("repos").join(repo_dir_name(&self.repo_url))
    }

    fn chat_worktrees(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.ws.join("worktrees"))
            .map(|r| {
                r.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .unwrap()
                            .to_string_lossy()
                            .starts_with("chat-")
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

async fn start_with(tweak: impl FnOnce(&mut Config)) -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let auth = root.join("auth-token");
    std::fs::write(&auth, AUTH).unwrap();
    let ws = root.join("workspace");
    std::fs::create_dir_all(ws.join(".enzarb")).unwrap();
    std::fs::write(ws.join(".enzarb/claude-token"), format!("{CLAUDE}\n")).unwrap();
    let agent = root.join("fake-chat-agent.mjs");
    std::fs::write(&agent, FAKE_AGENT).unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    // origin repo with one commit, cloned the way runs clone (`--no-checkout`)
    let origin = root.join("origin");
    std::fs::create_dir(&origin).unwrap();
    git(&origin, &["init", "-q", "-b", "main"]);
    std::fs::write(origin.join("README"), "hi\n").unwrap();
    git(&origin, &["add", "README"]);
    git(&origin, &["commit", "-qm", "init"]);
    let repo_url = format!("file://{}", origin.display());
    std::fs::create_dir_all(ws.join("repos")).unwrap();
    let clone = ws.join("repos").join(repo_dir_name(&repo_url));
    git(
        root,
        &[
            "clone",
            "-q",
            "--no-checkout",
            &repo_url,
            clone.to_str().unwrap(),
        ],
    );

    let mut config = Config {
        listen: ([127, 0, 0, 1], 0).into(),
        auth_token_file: auth,
        workspace: ws.clone(),
        claude_token_file: ws.join(".enzarb/claude-token"),
        shell: "/bin/sh".into(),
        grace: Duration::from_secs(2),
        acp_bin: agent.to_string_lossy().into_owned(),
        cancel_grace: Duration::from_secs(2),
        // The fake agent behaves like claude-agent-acp; ez has its own tests.
        chat_agent: ChatAgent::Claude,
        // The permission tests below are for `default`; bypass has its own.
        chat_permission_mode: ChatMode::Default,
        chat_allow_bypass: false,
        ..Config::default()
    };
    tweak(&mut config);
    let reaper = Reaper::new(false);
    tokio::spawn(reaper.clone().run());
    let state = AppState::new(config, reaper);
    let chats = state.chats.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Arc::new(state)));
    Server {
        addr,
        chats,
        ws,
        repo_url,
        _dir: dir,
    }
}

async fn start() -> Server {
    start_with(|_| {}).await
}

/// NDJSON over an `acp` channel; remembers every byte received.
struct Acp {
    ch: ChannelHandle,
    buf: Vec<u8>,
    all: String,
    next_id: u64,
}

impl Acp {
    fn new(ch: ChannelHandle) -> Self {
        Self {
            ch,
            buf: Vec::new(),
            all: String::new(),
            next_id: 1,
        }
    }

    async fn send(&mut self, v: Value) {
        let mut s = v.to_string();
        s.push('\n');
        self.ch.write_all(s.as_bytes()).await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        id
    }

    /// Next message, or `None` at end of stream.
    async fn recv(&mut self) -> Option<Value> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return Some(serde_json::from_slice(&line).expect("browser got invalid JSON"));
            }
            let mut b = vec![0u8; 64 * 1024];
            let n = timeout(T, self.ch.read(&mut b))
                .await
                .expect("timed out waiting for the agent")
                .unwrap();
            if n == 0 {
                return None;
            }
            self.all.push_str(&String::from_utf8_lossy(&b[..n]));
            self.buf.extend_from_slice(&b[..n]);
        }
    }

    /// Messages until one matches; returns (it, the ones before).
    async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> (Value, Vec<Value>) {
        let mut seen = Vec::new();
        loop {
            let m = self
                .recv()
                .await
                .unwrap_or_else(|| panic!("stream ended; seen {seen:?}"));
            if pred(&m) {
                return (m, seen);
            }
            seen.push(m);
        }
    }

    async fn response(&mut self, id: u64) -> (Value, Vec<Value>) {
        self.until(|m| m.get("id") == Some(&json!(id)) && m.get("method").is_none())
            .await
    }

    /// Read to the end; returns the CLOSE reason.
    async fn closed(&mut self) -> String {
        while self.recv().await.is_some() {}
        self.ch
            .recv
            .close_info()
            .map(|c| c.reason)
            .unwrap_or_default()
    }

    /// initialize + session/new; returns the session id.
    async fn start_session(&mut self) -> String {
        let id = self.request("initialize", json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true}})).await;
        let (init, _) = self.response(id).await;
        assert_eq!(init["result"]["agentCapabilities"]["loadSession"], true);
        let id = self
            .request("session/new", json!({"cwd": "/", "mcpServers": [{"name": "evil", "command": "/bin/sh", "args": []}]}))
            .await;
        let (new, _) = self.response(id).await;
        new["result"]["sessionId"]
            .as_str()
            .expect("session id")
            .to_string()
    }

    /// session/prompt with `text`; returns (response, the chunk texts of the turn).
    async fn prompt(&mut self, sid: &str, text: &str) -> (Value, String) {
        let id = self
            .request(
                "session/prompt",
                json!({"sessionId": sid, "prompt": [{"type": "text", "text": text}]}),
            )
            .await;
        let (r, seen) = self.response(id).await;
        (r, chunks(&seen))
    }
}

fn chunks(msgs: &[Value]) -> String {
    msgs.iter()
        .filter(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
        .collect()
}

async fn open(s: &Server, session: &Session) -> Acp {
    assert!(session.has_feature("acp"));
    let ch = timeout(T, session.open_acp(Some(s.repo_url.clone()), None))
        .await
        .unwrap()
        .expect("open_acp");
    Acp::new(ch)
}

async fn gone(pid: i32) -> bool {
    for _ in 0..100 {
        if kill(Pid::from_raw(pid), None) == Err(Errno::ESRCH) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn info(acp: &mut Acp, sid: &str) -> Value {
    let (r, text) = acp.prompt(sid, "INFO").await;
    assert_eq!(r["result"]["stopReason"], "end_turn");
    serde_json::from_str(&text).unwrap()
}

/// The ez agent: `acp` argument, ez's config and session directories outside
/// the worktree, the question capability, and the agent's own modes.
#[tokio::test]
async fn ez_chat_runs_ez_acp_with_its_dirs_and_modes() {
    let s = start_with(|c| {
        c.chat_agent = ChatAgent::Ez;
        c.chat_model = "claude-test".into();
    })
    .await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let wt = s.chat_worktrees()[0].clone();
    let i = info(&mut acp, &sid).await;
    assert_eq!(i["argv"], json!(["acp"]));
    assert_eq!(
        i["init"]["clientCapabilities"]["_meta"],
        json!({"ez": {"askUserQuestion": true}})
    );
    assert!(i["newParams"].get("_meta").is_none(), "{}", i["newParams"]);
    let conf = s.ws.join(".enzarb/ez-config");
    assert_eq!(i["ezConfigDir"], json!(conf));
    let sessions = PathBuf::from(i["ezSessionsDir"].as_str().unwrap());
    assert!(sessions.starts_with(s.ws.join(".enzarb/chats")));
    assert!(sessions.ends_with("ez-sessions") && sessions.is_dir());
    assert!(!sessions.starts_with(&wt));
    let toml = std::fs::read_to_string(conf.join("config.toml")).unwrap();
    assert!(toml.contains("default_profile = \"claude\""), "{toml}");
    assert!(toml.contains("provider = \"claude-code\""), "{toml}");
    assert!(toml.contains("model = \"claude-test\""), "{toml}");

    // Modes are the agent's: `acceptEdits` is offered by the fake agent, so it is
    // allowed for ez (claude chats refuse it); an unknown id is refused.
    let id = acp
        .request(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "acceptEdits"}),
        )
        .await;
    let (r, _) = acp.response(id).await;
    assert!(r.get("result").is_some(), "{r}");
    let id = acp
        .request(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "nope"}),
        )
        .await;
    let (r, _) = acp.response(id).await;
    assert_eq!(r["error"]["code"], -32602, "{r}");
    drop(acp);
}

#[tokio::test]
async fn chat_end_to_end() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let wts = s.chat_worktrees();
    assert_eq!(wts.len(), 1);
    let wt = wts[0].clone();
    assert!(
        wt.join("README").exists(),
        "worktree checked out at origin/main"
    );

    // forced capabilities, cwd, mcpServers; scrubbed env
    let i = info(&mut acp, &sid).await;
    assert_eq!(
        i["init"]["clientCapabilities"],
        json!({"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false})
    );
    assert_eq!(i["newParams"]["cwd"], json!(wt));
    assert_eq!(i["newParams"]["mcpServers"], json!([]));
    assert_eq!(i["cwd"], json!(wt));
    assert_eq!(
        i["configDir"],
        json!(s.ws.join(".enzarb/chat-home/.claude"))
    );
    let pid = i["pid"].as_i64().unwrap() as i32;
    let allowed: Vec<&str> = PASS_ENV
        .iter()
        .copied()
        .chain([
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CONFIG_DIR",
            "CARGO_TARGET_DIR",
            "GIT_TERMINAL_PROMPT",
            "GIT_AUTHOR_NAME",
            "GIT_AUTHOR_EMAIL",
            "GIT_COMMITTER_NAME",
            "GIT_COMMITTER_EMAIL",
            // node adds these itself
            "PWD",
            "OLDPWD",
            "SHLVL",
            "_",
        ])
        .collect();
    for k in i["env"].as_array().unwrap() {
        let k = k.as_str().unwrap();
        assert!(allowed.contains(&k), "unexpected env var {k}");
        assert!(
            !k.starts_with("ENZARB_") && !k.starts_with("GIT_CONFIG") && k != "NODE_AUTH_TOKEN"
        );
    }
    assert!(
        i["env"]
            .as_array()
            .unwrap()
            .contains(&json!("CLAUDE_CODE_OAUTH_TOKEN"))
    );

    // update stream
    let (r, text) = acp.prompt(&sid, "HELLO").await;
    assert_eq!(r["result"]["stopReason"], "end_turn");
    assert_eq!(text, "Hello world");

    // modes: plan ok, bypass refused
    let id = acp
        .request(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "plan"}),
        )
        .await;
    assert!(acp.response(id).await.0.get("result").is_some());
    let id = acp
        .request(
            "session/set_mode",
            json!({"sessionId": sid, "modeId": "bypassPermissions"}),
        )
        .await;
    assert_eq!(acp.response(id).await.0["error"]["code"], -32602);
    let id = acp.request("authenticate", json!({"methodId": "x"})).await;
    assert_eq!(acp.response(id).await.0["error"]["code"], -32601);

    // permissions: a and b are allowed in the sandbox (allow_always), only
    // leaving plan mode (c) reaches the browser; allow_always becomes reject there
    let pid_req = acp
        .request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "PERM"}]}),
        )
        .await;
    let (c, before) = acp
        .until(|m| m["method"] == "session/request_permission")
        .await;
    assert!(
        before
            .iter()
            .all(|m| m["method"] != "session/request_permission")
    );
    assert_eq!(c["params"]["toolCall"]["toolCallId"], "c");
    // an answer to an id never issued is ignored, then allow_always is replaced
    acp.send(json!({"jsonrpc": "2.0", "id": 424242, "result": {"outcome": {"outcome": "selected", "optionId": "yes"}}}))
        .await;
    acp.send(json!({"jsonrpc": "2.0", "id": c["id"], "result": {"outcome": {"outcome": "selected", "optionId": "always"}}}))
        .await;
    let (r, seen) = acp.response(pid_req).await;
    assert_eq!(r["result"]["stopReason"], "end_turn");
    assert_eq!(chunks(&seen), "perm: a=always,b=always,c=no");

    // agent requests to the client are refused; spoofed notifications dropped
    let (_, text) = acp.prompt(&sid, "FSREQ").await;
    assert_eq!(text, "fs=-32601");
    assert!(!acp.all.contains("spoof"));
    assert!(!acp.all.contains("session/other_notice"));

    // the Claude token never reaches the browser
    let (_, text) = acp.prompt(&sid, "LEAK").await;
    assert!(text.contains("token=[redacted]"), "{text}");
    assert!(!acp.all.contains("FAKEclaudeTOKEN"));
    assert!(!acp.all.contains(&AUTH[..12]));

    // close: agent stopped, worktree removed, branch and registry kept
    drop(acp);
    assert!(gone(pid).await, "agent still running");
    for _ in 0..100 {
        if !wt.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!wt.exists(), "chat worktree not removed");
    let chat_id = wt.file_name().unwrap().to_string_lossy()["chat-".len()..].to_string();
    let (ok, _) = git_out(
        &s.clone_dir(),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/enzarb/chat-{chat_id}"),
        ],
    );
    assert!(ok, "chat branch kept");
    let meta_dir = s.ws.join(".enzarb/chats").join(&chat_id);
    let meta: Value =
        serde_json::from_slice(&std::fs::read(meta_dir.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["acp_session_id"], json!(sid));
    let tr = meta_dir.join("transcript.jsonl");
    let mode = std::fs::metadata(&tr).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let transcript = std::fs::read_to_string(&tr).unwrap();
    assert!(transcript.contains("\"permission\""));
    assert!(transcript.contains("\"sandbox\""));
    assert!(
        !transcript.contains("FAKEclaudeTOKEN"),
        "token in transcript"
    );
    // the registry slot is released after the worktree is removed
    for _ in 0..100 {
        if s.chats.live() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(s.chats.live(), 0);
}

#[tokio::test]
async fn chat_cancel_resolves_pending_permission() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let id = acp
        .request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "SLEEP"}]}),
        )
        .await;
    let (req, _) = acp
        .until(|m| m["method"] == "session/request_permission")
        .await;
    acp.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}}))
        .await;
    let (r, seen) = acp.response(id).await;
    assert_eq!(r["result"]["stopReason"], "cancelled");
    assert!(chunks(&seen).contains("cancelled perm=cancelled"));
    // a late click is dropped (the agent already got `cancelled`)
    acp.send(json!({"jsonrpc": "2.0", "id": req["id"], "result": {"outcome": {"outcome": "selected", "optionId": "yes"}}}))
        .await;
    let (_, text) = acp.prompt(&sid, "after").await;
    assert_eq!(text, "echo: after");
}

#[tokio::test]
async fn chat_idle_timeout_closes_and_stops_the_agent() {
    let s = start_with(|c| c.chat_idle = Duration::from_secs(1)).await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let pid = info(&mut acp, &sid).await["pid"].as_i64().unwrap() as i32;
    // a pending permission does not keep the chat alive
    acp.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "SLEEP"}]}),
    )
    .await;
    assert_eq!(acp.closed().await, "idle");
    assert!(gone(pid).await);
}

#[tokio::test]
async fn chat_hard_lifetime_limit() {
    let s = start_with(|c| c.chat_max = Duration::from_secs(1)).await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    acp.start_session().await;
    assert_eq!(acp.closed().await, "limit");
}

#[tokio::test]
async fn chat_line_cap() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    // agent -> browser
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    acp.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "BIG"}]}),
    )
    .await;
    assert_eq!(acp.closed().await, "limit");
    drop(acp);
    // browser -> agent: 5 MiB without a newline
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let junk = vec![b'x'; 5 * 1024 * 1024];
    let _ = acp.ch.write_all(&junk).await;
    assert_eq!(acp.closed().await, "limit");
}

#[tokio::test]
async fn chat_agent_exit_is_reported() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    acp.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "EXIT"}]}),
    )
    .await;
    assert_eq!(acp.closed().await, "exit:3");
}

#[tokio::test]
async fn chat_limit_per_socket_and_supersede() {
    let s = start().await;
    let (a_sess, _a_ev) = s.connect().await;
    let mut a = open(&s, &a_sess).await;
    let a_sid = a.start_session().await;
    let a_pid = info(&mut a, &a_sid).await["pid"].as_i64().unwrap() as i32;
    // a second acp channel on the same socket
    let err = a_sess
        .open_acp(Some(s.repo_url.clone()), None)
        .await
        .err()
        .unwrap();
    assert_eq!(err.open_error_code(), Some(open_error_code::CHAT_LIMIT));
    // a new socket takes over (ENZARB_MAX_CHATS=1)
    let (b_sess, _b_ev) = s.connect().await;
    let mut b = open(&s, &b_sess).await;
    assert_eq!(a.closed().await, "superseded");
    assert!(gone(a_pid).await);
    let b_sid = b.start_session().await;
    let (_, text) = b.prompt(&b_sid, "hi").await;
    assert_eq!(text, "echo: hi");
    assert_eq!(s.chats.live(), 1);
}

#[tokio::test]
async fn chat_shutdown_closes_with_shutdown() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let pid = info(&mut acp, &sid).await["pid"].as_i64().unwrap() as i32;
    let chats = s.chats.clone();
    let sd = tokio::spawn(async move { chats.shutdown(Duration::from_secs(15)).await });
    assert_eq!(acp.closed().await, "shutdown");
    sd.await.unwrap();
    assert!(gone(pid).await);
    assert!(s.chat_worktrees().is_empty());
    // no new chats while shutting down
    let (session, _ev) = s.connect().await;
    assert!(
        session
            .open_acp(Some(s.repo_url.clone()), None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn chat_resume_loads_only_registered_sessions() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let wt = s.chat_worktrees()[0].clone();
    // the agent commits on the chat branch
    std::fs::write(wt.join("chat.txt"), "x").unwrap();
    git(&wt, &["add", "chat.txt"]);
    git(&wt, &["commit", "-qm", "chat"]);
    drop(acp);
    drop(session);
    for _ in 0..100 {
        if s.chats.live() == 0 && !wt.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!wt.exists());

    // resume: same worktree path, the committed work is back, load works
    let (session, _ev) = s.connect().await;
    let ch = session
        .open_acp(Some(s.repo_url.clone()), Some(sid.clone()))
        .await
        .unwrap();
    let mut acp = Acp::new(ch);
    assert!(wt.join("chat.txt").exists(), "resumed from the chat branch");
    let id = acp
        .request("initialize", json!({"protocolVersion": 1}))
        .await;
    acp.response(id).await;
    let id = acp
        .request(
            "session/load",
            json!({"sessionId": "sess-not-mine", "cwd": "/"}),
        )
        .await;
    assert_eq!(acp.response(id).await.0["error"]["code"], -32602);
    let id = acp
        .request(
            "session/load",
            json!({"sessionId": sid, "cwd": "/", "mcpServers": [{"name": "x"}]}),
        )
        .await;
    let (r, seen) = acp.response(id).await;
    assert!(r.get("result").is_some(), "{r}");
    assert_eq!(chunks(&seen), format!("replayed {sid}"));
    let i = info(&mut acp, &sid).await;
    assert_eq!(i["loadParams"]["cwd"], json!(wt));
    assert_eq!(i["loadParams"]["mcpServers"], json!([]));
    drop(acp);

    // an unknown resume id opens a fresh chat, whose load is refused
    let (session, _ev) = s.connect().await;
    let ch = session
        .open_acp(Some(s.repo_url.clone()), Some("unknown-session".into()))
        .await
        .unwrap();
    let mut acp = Acp::new(ch);
    let id = acp
        .request("initialize", json!({"protocolVersion": 1}))
        .await;
    acp.response(id).await;
    let id = acp
        .request("session/load", json!({"sessionId": "unknown-session"}))
        .await;
    assert_eq!(acp.response(id).await.0["error"]["code"], -32602);
}

#[tokio::test]
async fn chat_open_errors() {
    // disabled
    let s = start_with(|c| c.max_chats = 0).await;
    let (session, _ev) = s.connect().await;
    assert!(!session.has_feature("acp"));
    let err = session.open_acp(None, None).await.err().unwrap();
    assert_eq!(err.open_error_code(), Some(open_error_code::CHAT_DISABLED));

    // adapter missing
    let s = start_with(|c| c.acp_bin = "/nonexistent/claude-agent-acp".into()).await;
    let (session, _ev) = s.connect().await;
    assert!(!session.has_feature("acp"));
    let err = session.open_acp(None, None).await.err().unwrap();
    assert_eq!(err.open_error_code(), Some(open_error_code::CHAT_DISABLED));

    let s = start().await;
    let (session, _ev) = s.connect().await;
    let code = |r: Result<ChannelHandle, SessionError>| {
        r.err().unwrap().open_error_code().map(String::from)
    };
    // repo not cloned
    assert_eq!(
        code(
            session
                .open_acp(Some("https://github.com/enzarb/not-cloned".into()), None)
                .await
        )
        .as_deref(),
        Some(open_error_code::REPO_NOT_CLONED)
    );
    // invalid repo / resume
    assert_eq!(
        code(session.open_acp(Some("https://x/ y".into()), None).await).as_deref(),
        Some(open_error_code::INVALID_REQUEST)
    );
    assert_eq!(
        code(
            session
                .open_acp(None, Some("../../etc/passwd".into()))
                .await
        )
        .as_deref(),
        Some(open_error_code::INVALID_REQUEST)
    );
    // missing credential
    std::fs::remove_file(s.ws.join(".enzarb/claude-token")).unwrap();
    assert_eq!(
        code(session.open_acp(Some(s.repo_url.clone()), None).await).as_deref(),
        Some(open_error_code::NO_CREDENTIAL)
    );
    // none of these left a chat or a worktree behind
    assert_eq!(s.chats.live(), 0);
    assert!(s.chat_worktrees().is_empty());
    // a per-open token works without the legacy file and is redacted
    let per_open = "sk-ant-oat01-PEROPENtoken_zyxwvut";
    let mut acp = Acp::new(
        session
            .open_acp_with_token(Some(s.repo_url.clone()), None, Some(per_open.into()))
            .await
            .unwrap(),
    );
    let sid = acp.start_session().await;
    let (_, text) = acp.prompt(&sid, "LEAK").await;
    assert!(text.contains("token=[redacted]"), "{text}");
    assert!(!acp.all.contains("PEROPENtoken"));
    drop(acp);
    // an empty per-open token counts as absent
    assert_eq!(
        code(
            session
                .open_acp_with_token(Some(s.repo_url.clone()), None, Some("".into()))
                .await
        )
        .as_deref(),
        Some(open_error_code::NO_CREDENTIAL)
    );
    // without repo: the first clone is used
    std::fs::write(s.ws.join(".enzarb/claude-token"), CLAUDE).unwrap();
    let mut acp = Acp::new(session.open_acp(None, None).await.unwrap());
    let sid = acp.start_session().await;
    let (_, text) = acp.prompt(&sid, "x").await;
    assert_eq!(text, "echo: x");
}

#[tokio::test]
async fn stale_chat_worktrees_are_pruned_at_startup() {
    let s = start().await;
    let stale = s.ws.join("worktrees/chat-00000000deadbeef");
    std::fs::create_dir_all(&stale).unwrap();
    s.chats.recover().await;
    for _ in 0..50 {
        if !stale.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!stale.exists());
}

/// A browser looping close -> open on one socket (the proxy frees its slot on
/// the browser's CLOSE, before the agent is gone): each new agent starts only
/// after the previous one is dead, and the cap never counts more than one.
#[tokio::test]
async fn chat_close_open_loop_counts_terminating_agents() {
    let s = start_with(|c| c.cancel_grace = Duration::from_secs(1)).await;
    let (session, _ev) = s.connect().await;
    let mut prev: Option<i32> = None;
    for _ in 0..4 {
        let mut acp = open(&s, &session).await;
        if let Some(p) = prev {
            // the old agent was gone before this channel was opened
            assert_eq!(
                kill(Pid::from_raw(p), None),
                Err(Errno::ESRCH),
                "agent {p} still alive"
            );
        }
        assert!(s.chats.live() <= 1, "cap exceeded: {}", s.chats.live());
        let sid = acp.start_session().await;
        let pid = info(&mut acp, &sid).await["pid"].as_i64().unwrap() as i32;
        // leave a turn running so teardown has to cancel and kill
        acp.request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "SLEEP"}]}),
        )
        .await;
        acp.until(|m| m["method"] == "session/request_permission")
            .await;
        drop(acp); // browser CLOSE; reopen immediately
        prev = Some(pid);
    }
    assert!(gone(prev.unwrap()).await);
}

/// The sandbox, not the proxy, is the authority on the cap: an extra `acp`
/// open that slips through (e.g. a stale CLOSE freed the proxy's slot) is
/// refused on the same socket while the chat is live, and concurrent opens
/// yield exactly one chat.
#[tokio::test]
async fn chat_cap_is_enforced_in_the_sandbox() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let (a, b, c) = tokio::join!(
        session.open_acp(Some(s.repo_url.clone()), None),
        session.open_acp(Some(s.repo_url.clone()), None),
        session.open_acp(Some(s.repo_url.clone()), None),
    );
    let ok: Vec<_> = [a, b, c]
        .into_iter()
        .filter_map(|r| match r {
            Ok(h) => Some(h),
            Err(e) => {
                assert_eq!(e.open_error_code(), Some(open_error_code::CHAT_LIMIT));
                None
            }
        })
        .collect();
    assert_eq!(ok.len(), 1);
    let mut acp = Acp::new(ok.into_iter().next().unwrap());
    let sid = acp.start_session().await;
    let err = session
        .open_acp(Some(s.repo_url.clone()), None)
        .await
        .err()
        .unwrap();
    assert_eq!(err.open_error_code(), Some(open_error_code::CHAT_LIMIT));
    let (_, text) = acp.prompt(&sid, "still").await;
    assert_eq!(text, "echo: still");
    assert_eq!(s.chats.live(), 1);
}

/// A browser that stops granting credit (never reads) must not stall
/// teardown: the agent is killed and the cap slot freed anyway.
#[tokio::test]
async fn chat_teardown_survives_a_browser_that_never_reads() {
    let s = start_with(|c| c.chat_idle = Duration::from_secs(1)).await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let sid = acp.start_session().await;
    let pid = info(&mut acp, &sid).await["pid"].as_i64().unwrap() as i32;
    acp.request(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "FLOOD"}]}),
    )
    .await;
    // never read again: 4 MiB of output against a 256 KiB window
    assert!(gone(pid).await, "agent not stopped");
    for _ in 0..150 {
        if s.chats.live() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(s.chats.live(), 0, "cap slot still held");
    assert!(s.chat_worktrees().is_empty());
    drop(acp);
}

async fn start_bypass(allow: bool, mode: ChatMode) -> Server {
    start_with(|c| {
        c.chat_permission_mode = mode;
        c.chat_allow_bypass = allow;
    })
    .await
}

#[tokio::test]
async fn chat_bypass_is_the_default_mode() {
    assert_eq!(Config::default().chat_permission_mode, ChatMode::Bypass);
    let s = start_bypass(true, ChatMode::Bypass).await;
    // the adapter would start in the settings' mode; sandbox-init overrides it
    let home = s.ws.join(".enzarb/chat-home/.claude");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("settings.json"),
        r#"{"permissions":{"defaultMode":"acceptEdits"}}"#,
    )
    .unwrap();
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let id = acp
        .request("initialize", json!({"protocolVersion": 1}))
        .await;
    acp.response(id).await;
    let id = acp
        .request(
            "session/new",
            json!({"cwd": "/", "_meta": {"claudeCode": {"options": {"permissionMode": "default", "allowDangerouslySkipPermissions": false}}}}),
        )
        .await;
    let (new, seen) = acp.response(id).await;
    assert!(seen.is_empty(), "{seen:?}");
    let sid = new["result"]["sessionId"].as_str().unwrap().to_string();
    assert_eq!(new["result"]["modes"]["currentModeId"], "bypassPermissions");
    let ids: Vec<&str> = new["result"]["modes"]["availableModes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, ["default", "acceptEdits", "plan", "bypassPermissions"]);
    let (note, _) = acp.until(|m| m["method"] == "_enzarb/mode").await;
    assert_eq!(
        note["params"],
        json!({"sessionId": sid, "modeId": "bypassPermissions", "bypassAllowed": true})
    );

    // the fixed _meta, whatever the browser sent
    let i = info(&mut acp, &sid).await;
    assert_eq!(
        i["newParams"]["_meta"],
        json!({"claudeCode": {"options": {"allowDangerouslySkipPermissions": true, "permissionMode": "bypassPermissions", "settingSources": ["user"]}}})
    );
    assert_eq!(i["modes"], json!(["bypassPermissions"]));
    assert_eq!(i["currentMode"], "bypassPermissions");

    // no permission requests reach the browser
    let (r, text) = acp.prompt(&sid, "PERM").await;
    assert_eq!(r["result"]["stopReason"], "end_turn");
    assert_eq!(text, "perm: a=bypassed,b=bypassed,c=bypassed");
    assert!(!acp.all.contains("session/request_permission"));

    // the token is still redacted
    let (_, text) = acp.prompt(&sid, "LEAK").await;
    assert!(text.contains("token=[redacted]"), "{text}");
    assert!(!acp.all.contains("FAKEclaudeTOKEN"));

    // switching down and back up is allowed; others are not
    for (m, ok) in [
        ("plan", true),
        ("acceptEdits", false),
        ("bypassPermissions", true),
        ("default", true),
    ] {
        let id = acp
            .request("session/set_mode", json!({"sessionId": sid, "modeId": m}))
            .await;
        let (r, _) = acp.response(id).await;
        assert_eq!(r.get("result").is_some(), ok, "{m}: {r}");
        if ok {
            let (note, _) = acp.until(|v| v["method"] == "_enzarb/mode").await;
            assert_eq!(note["params"]["modeId"], m);
        }
    }
    // back in default: sandbox-init answers, leaving plan mode goes to the user
    let id = acp
        .request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": "PERM"}]}),
        )
        .await;
    let (c, _) = acp
        .until(|m| m["method"] == "session/request_permission")
        .await;
    acp.send(json!({"jsonrpc": "2.0", "id": c["id"], "result": {"outcome": {"outcome": "selected", "optionId": "no"}}}))
        .await;
    let (_, seen) = acp.response(id).await;
    assert_eq!(chunks(&seen), "perm: a=always,b=always,c=no");
}

#[tokio::test]
async fn chat_mode_from_config_without_bypass() {
    for mode in [ChatMode::Default, ChatMode::Plan] {
        let s = start_bypass(false, mode).await;
        let (session, _ev) = s.connect().await;
        let mut acp = open(&s, &session).await;
        let sid = acp.start_session().await;
        let i = info(&mut acp, &sid).await;
        assert_eq!(
            i["newParams"]["_meta"]["claudeCode"]["options"]["allowDangerouslySkipPermissions"],
            false
        );
        assert_eq!(i["currentMode"], mode.as_str());
        assert!(
            acp.all
                .contains(&format!("\"modeId\":\"{}\"", mode.as_str()))
        );
        let id = acp
            .request(
                "session/set_mode",
                json!({"sessionId": sid, "modeId": "bypassPermissions"}),
            )
            .await;
        assert_eq!(acp.response(id).await.0["error"]["code"], -32602);
        let i = info(&mut acp, &sid).await;
        assert_eq!(i["currentMode"], mode.as_str());
    }
}

#[tokio::test]
async fn chat_bypass_mode_survives_resume() {
    let s = start_bypass(true, ChatMode::Bypass).await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    let id = acp
        .request("initialize", json!({"protocolVersion": 1}))
        .await;
    acp.response(id).await;
    let id = acp.request("session/new", json!({})).await;
    let sid = acp.response(id).await.0["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    drop(acp);
    drop(session);
    for _ in 0..100 {
        if s.chats.live() == 0 && s.chat_worktrees().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (session, _ev) = s.connect().await;
    let ch = session
        .open_acp(Some(s.repo_url.clone()), Some(sid.clone()))
        .await
        .unwrap();
    let mut acp = Acp::new(ch);
    let id = acp
        .request("initialize", json!({"protocolVersion": 1}))
        .await;
    acp.response(id).await;
    let id = acp.request("session/load", json!({"sessionId": sid})).await;
    let (r, _) = acp.response(id).await;
    assert!(r.get("result").is_some(), "{r}");
    let i = info(&mut acp, &sid).await;
    assert_eq!(i["currentMode"], "bypassPermissions");
    assert_eq!(
        i["loadParams"]["_meta"]["claudeCode"]["options"]["permissionMode"],
        "bypassPermissions"
    );
}

/// Issue #52: a chat gets its own repository, not a worktree of the clone
/// jobs push from, so config and hooks it writes stay in its own `.git`.
#[tokio::test]
async fn chat_repo_does_not_share_git_dir_with_the_job_clone() {
    let s = start().await;
    let (session, _ev) = s.connect().await;
    let mut acp = open(&s, &session).await;
    acp.start_session().await;
    let wt = s.chat_worktrees()[0].clone();
    assert!(wt.join(".git").is_dir(), "a real .git, not a gitfile");
    let (ok, gd) = git_out(&wt, &["rev-parse", "--git-common-dir"]);
    assert!(ok);
    assert_eq!(Path::new(&gd), Path::new(".git"), "own common dir");
    let (_, origin) = git_out(&wt, &["config", "--get", "remote.origin.url"]);
    assert_eq!(origin, s.repo_url);
    // what the agent plants stays in the chat repo
    std::fs::create_dir_all(wt.join(".git/hooks")).unwrap();
    std::fs::write(wt.join(".git/hooks/pre-push"), "#!/bin/sh\n").unwrap();
    git(&wt, &["config", "core.fsmonitor", "touch /tmp/x"]);
    let clone = s.clone_dir();
    assert!(!clone.join(".git/hooks/pre-push").exists());
    let (set, _) = git_out(&clone, &["config", "--get", "core.fsmonitor"]);
    assert!(!set, "planted config reached the job clone");
    assert!(
        !clone.join(".git/worktrees").exists() || {
            std::fs::read_dir(clone.join(".git/worktrees"))
                .unwrap()
                .next()
                .is_none()
        }
    );
}
