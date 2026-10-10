//! Integration tests for agent runs: a real server, local bare repos, a fake
//! ACP agent (a node script) and a test HTTP callback server.

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use enzarb_proto::session::{Client, Event, Events, Session};
use enzarb_proto::{ClientKind, CredentialKind, Ctrl, RunFinished, RunOutcome, RunPhase, RunStart};
use sandbox_init::agent::AgentRunManager;
use sandbox_init::capture::ClaudeTokenCapture;
use sandbox_init::reaper::Reaper;
use sandbox_init::{AppState, Config, serve};
use serde_json::json;
use tokio::time::{sleep, timeout};

const TOKEN: &str = "test-token-123";
const CLAUDE_TOKEN: &str = "tok-123";
const T: Duration = Duration::from_secs(30);

const FAKE_AGENT: &str = include_str!("fixtures/fake-acp-agent.mjs");

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

type Callbacks = Arc<Mutex<Vec<(String, RunFinished)>>>;

#[derive(Clone)]
struct CbState {
    got: Callbacks,
    fail_first: Arc<Mutex<u32>>,
}

async fn cb(State(s): State<CbState>, h: HeaderMap, Json(b): Json<RunFinished>) -> StatusCode {
    {
        let mut n = s.fail_first.lock().unwrap();
        if *n > 0 {
            *n -= 1;
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    let auth = h
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    s.got.lock().unwrap().push((auth, b));
    StatusCode::OK
}

struct Env {
    addr: SocketAddr,
    cb_url: String,
    got: Callbacks,
    dir: tempfile::TempDir,
    origin: PathBuf,
    workspace: PathBuf,
}

async fn start(fail_callbacks: u32) -> Env {
    start_with(fail_callbacks, None).await
}

/// `mise_script`: body of a fake `mise` that sandbox-init runs in each worktree.
async fn start_with(fail_callbacks: u32, mise_script: Option<&str>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // origin with main (+ a feature branch)
    let origin = root.join("origin.git");
    git(
        root,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    let seed = root.join("seed");
    git(root, &["init", "-q", "-b", "main", seed.to_str().unwrap()]);
    std::fs::write(seed.join("README"), "hello\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-qm", "init"]);
    git(&seed, &["push", "-q", origin.to_str().unwrap(), "main"]);
    git(&seed, &["checkout", "-qb", "feature"]);
    std::fs::write(seed.join("f.txt"), "f\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-qm", "feature"]);
    git(&seed, &["push", "-q", origin.to_str().unwrap(), "feature"]);

    let agent = root.join("fake-acp-agent");
    std::fs::write(&agent, FAKE_AGENT).unwrap();
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).unwrap();
    let token_file = root.join("auth-token");
    std::fs::write(&token_file, format!("{TOKEN}\n")).unwrap();
    let workspace = root.join("workspace");
    std::fs::create_dir_all(workspace.join(".enzarb")).unwrap();
    std::fs::write(
        workspace.join(".enzarb/claude-token"),
        format!("{CLAUDE_TOKEN}\n"),
    )
    .unwrap();

    let got: Callbacks = Arc::default();
    let cbs = CbState {
        got: got.clone(),
        fail_first: Arc::new(Mutex::new(fail_callbacks)),
    };
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cb_url = format!("http://{}/cb", l.local_addr().unwrap());
    let app = Router::new().route("/cb", post(cb)).with_state(cbs);
    tokio::spawn(async move { axum::serve(l, app).await });

    let mise_bin = mise_script.map_or(String::new(), |body| {
        let m = root.join("fake-mise");
        std::fs::write(&m, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&m, std::fs::Permissions::from_mode(0o755)).unwrap();
        m.to_string_lossy().into_owned()
    });
    let config = Config {
        mise_bin,
        listen: ([127, 0, 0, 1], 0).into(),
        auth_token_file: token_file,
        workspace: workspace.clone(),
        shell: "/bin/sh".into(),
        grace: Duration::from_secs(2),
        acp_bin: agent.to_string_lossy().into_owned(),
        cancel_grace: Duration::from_secs(1),
        claude_token_file: workspace.join(".enzarb/claude-token"),
        callback_backoff: Duration::from_millis(100),
        ..Config::default()
    };
    let reaper = Reaper::new(false);
    tokio::spawn(reaper.clone().run());
    let runs = AgentRunManager::new(config.clone(), reaper.clone());
    runs.recover();
    let state = AppState::new(config.clone(), reaper)
        .with_run_manager(Arc::new(runs))
        .with_observer(Arc::new(ClaudeTokenCapture::new(
            config.claude_token_file.clone(),
        )));
    let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Arc::new(state)));
    Env {
        addr,
        cb_url,
        got,
        dir,
        origin,
        workspace,
    }
}

impl Env {
    async fn connect(&self) -> (Session, Events) {
        let url = format!("ws://{}/v1/mux", self.addr);
        timeout(T, Client::connect(&url, TOKEN, ClientKind::Executor))
            .await
            .unwrap()
            .unwrap()
    }

    fn run(&self, id: &str, prompt: &str, base: &str, branch: &str, push: bool) -> RunStart {
        serde_json::from_value(json!({
            "run_id": id,
            "repo_url": format!("file://{}", self.origin.display()),
            "base_ref": base, "branch": branch, "push": push,
            "clone_token": "secret-clone-token", "prompt": prompt, "timeout_secs": 60,
            "callback_url": self.cb_url, "callback_token": "cbtok", "log_channel": null
        }))
        .unwrap()
    }

    async fn finished(&self, id: &str) -> (String, RunFinished) {
        timeout(T, async {
            loop {
                if let Some(x) = self
                    .got
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|(_, b)| b.run_id == id)
                {
                    return x.clone();
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("callback not received")
    }

    fn remote_sha(&self, branch: &str) -> Option<String> {
        let out = Command::new("git")
            .current_dir(&self.origin)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ])
            .output()
            .unwrap();
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn run_dir(&self, id: &str) -> PathBuf {
        self.workspace.join(".enzarb/runs").join(id)
    }
}

const SUMMARY: &str =
    "Looking around.\ndone: enzarb/issue-1\n```json\n{\"decision\":\"done\",\"body\":\"pr\"}\n```";

#[tokio::test]
async fn implement_run_commits_pushes_and_calls_back() {
    let env = start(2).await; // the first two callbacks fail: retried with backoff
    let (session, mut events) = env.connect().await;
    let mut req = env.run("job-1", "EDIT", "main", "enzarb/issue-1", true);
    req.log_channel = Some(5);
    let started = session.run_start(req.clone()).await.unwrap();
    assert_eq!(started.accepted.state, RunPhase::Queued);
    assert!(!started.accepted.existing);
    let mut log = started.log.unwrap();

    let (auth, fin) = env.finished("job-1").await;
    assert_eq!(auth, "Bearer cbtok");
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    assert_eq!(fin.exit_code, Some(0));
    assert_eq!(fin.summary, SUMMARY);
    assert_eq!(fin.branch, "enzarb/issue-1");
    assert!(fin.pushed);
    assert_eq!(Some(fin.head_sha.clone()), env.remote_sha("enzarb/issue-1"));
    assert_ne!(Some(fin.head_sha.clone()), env.remote_sha("main"));

    // pushed states: running ... succeeded
    let mut states = Vec::new();
    while let Ok(Some(Event::Ctrl(Ctrl::RunState { state, summary, .. }))) =
        timeout(Duration::from_secs(2), events.next()).await
    {
        if state.is_terminal() {
            assert_eq!(summary.as_deref(), Some(SUMMARY));
        }
        states.push(state);
    }
    assert_eq!(states, vec![RunPhase::Running, RunPhase::Succeeded]);

    // transcript streamed on the log channel
    let mut kinds = Vec::new();
    while let Ok(Ok(Some(v))) = timeout(T, log.recv.recv_json::<serde_json::Value>()).await {
        kinds.push(v["kind"].as_str().unwrap().to_string());
    }
    for k in [
        "state",
        "worktree",
        "acp",
        "update",
        "agent_result",
        "finished",
    ] {
        assert!(kinds.iter().any(|x| x == k), "{kinds:?}");
    }

    // on-disk layout; the clone token is never persisted
    let rd = env.run_dir("job-1");
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(rd.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["state"], "succeeded");
    assert!(rd.join("transcript.jsonl").exists());
    assert!(env.workspace.join("worktrees/run-job-1/agent.txt").exists());
    let out = Command::new("grep")
        .args(["-r", "secret-clone-token"])
        .arg(env.dir.path())
        .output()
        .unwrap();
    assert!(
        out.stdout.is_empty(),
        "token persisted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    sleep(Duration::from_millis(300)).await;
    assert!(
        !rd.join("callback.json").exists(),
        "delivered callback removed"
    );

    // idempotent
    let again = session
        .run_start(RunStart {
            log_channel: None,
            ..req
        })
        .await
        .unwrap();
    assert!(again.accepted.existing);
    assert_eq!(again.accepted.state, RunPhase::Succeeded);
    let info = session.run_status("job-1").await.unwrap();
    assert_eq!(info.state, RunPhase::Succeeded);
    assert_eq!(info.exit_code, Some(0));
    assert_eq!(env.got.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn dirty_worktree_is_committed_and_nothing_to_push_is_not() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("dirty", "DIRTY", "main", "enzarb/issue-2", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("dirty").await;
    assert!(fin.pushed, "{fin:?}");
    assert_eq!(Some(fin.head_sha), env.remote_sha("enzarb/issue-2"));

    session
        .run_start(env.run("noop", "NOOP", "main", "enzarb/issue-3", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("noop").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    assert!(!fin.pushed);
    assert_eq!(env.remote_sha("enzarb/issue-3"), None);
}

#[tokio::test]
async fn address_run_continues_the_pr_branch() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let before = env.remote_sha("feature").unwrap();
    session
        .run_start(env.run("addr", "EDIT", "feature", "feature", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("addr").await;
    assert!(fin.pushed);
    let after = env.remote_sha("feature").unwrap();
    assert_ne!(before, after);
    assert_eq!(after, fin.head_sha);
    // history keeps the feature commit
    let log = git(&env.origin, &["log", "--format=%s", "feature"]);
    assert!(
        log.contains("feature") && log.contains("agent change"),
        "{log}"
    );
}

#[tokio::test]
async fn read_only_run_does_not_push() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("ro", "EDIT", "feature", "feature", false))
        .await
        .unwrap();
    let before = env.remote_sha("feature").unwrap();
    let (_, fin) = env.finished("ro").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    assert!(!fin.pushed);
    assert_eq!(env.remote_sha("feature").unwrap(), before);
    assert_ne!(fin.head_sha, before); // the local commit exists in the worktree
}

#[tokio::test]
async fn toolchain_is_installed_in_the_worktree_before_the_agent_runs() {
    let env = start_with(
        0,
        Some(r#"test "$1" = install && pwd > "$PWD/../mise-ran""#),
    )
    .await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("tc", "EDIT", "feature", "feature", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("tc").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let ran = std::fs::read_to_string(env.workspace.join("worktrees/mise-ran")).unwrap();
    assert!(ran.trim().ends_with("worktrees/run-tc"), "{ran}");
    let t = transcript(&env, "tc");
    assert_eq!(of_kind(&t, "toolchain")[0]["ok"], true);
}

#[tokio::test]
async fn a_failing_toolchain_install_does_not_fail_the_run() {
    let env = start_with(0, Some("echo boom >&2; exit 3")).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("tcf", "EDIT", "feature", "feature", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("tcf").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let t = transcript(&env, "tcf");
    let rec = of_kind(&t, "toolchain")[0];
    assert_eq!(rec["ok"], false);
    assert_eq!(rec["code"], 3);
    assert!(rec["stderr"].as_str().unwrap().contains("boom"));
}

/// All transcript records of a run (the file, once the run is over).
fn transcript(env: &Env, id: &str) -> Vec<serde_json::Value> {
    std::fs::read_to_string(env.run_dir(id).join("transcript.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn of_kind<'a>(t: &'a [serde_json::Value], kind: &str) -> Vec<&'a serde_json::Value> {
    t.iter().filter(|r| r["kind"] == kind).collect()
}

fn pid_alive(env: &Env, id: &str) -> bool {
    let pid: i32 = std::fs::read_to_string(env.workspace.join(format!("worktrees/run-{id}.pid")))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // sandbox-init's reaper collects the child: signal 0 fails once it is gone
    std::thread::sleep(Duration::from_millis(300));
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

#[tokio::test]
async fn crashing_agent_fails_the_run() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("bad", "FAIL", "main", "enzarb/issue-9", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("bad").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(
        fin.summary.contains("exited unexpectedly with code 1"),
        "{}",
        fin.summary
    );
    assert!(!fin.pushed);
    let t = transcript(&env, "bad");
    assert!(
        of_kind(&t, "log")
            .iter()
            .any(|r| r["source"] == "acp" && r["line"] == "boom"),
        "agent stderr is in the transcript"
    );
}

#[tokio::test]
async fn refusal_and_empty_turns_fail_the_run() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("refuse", "REFUSE", "main", "enzarb/issue-10", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("refuse").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("refusal"), "{}", fin.summary);
    assert!(fin.summary.contains("I cannot do that."), "{}", fin.summary);

    session
        .run_start(env.run("empty", "EMPTY", "main", "enzarb/issue-11", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("empty").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("no message"), "{}", fin.summary);
}

#[tokio::test]
async fn missing_token_fails_and_a_rejected_token_surfaces_the_agent_error() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    std::fs::write(env.workspace.join(".enzarb/claude-token"), "wrong\n").unwrap();
    session
        .run_start(env.run("badtok", "NOOP", "main", "enzarb/issue-12", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("badtok").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(
        fin.summary.contains("Authentication required"),
        "{}",
        fin.summary
    );

    std::fs::remove_file(env.workspace.join(".enzarb/claude-token")).unwrap();
    session
        .run_start(env.run("notok", "NOOP", "main", "enzarb/issue-13", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("notok").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("not logged in"), "{}", fin.summary);
}

#[tokio::test]
async fn permission_decisions_follow_the_policy_and_are_recorded() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = env.run("perm", "PERM", "main", "enzarb/issue-14", false);
    req.log_channel = Some(7);
    let started = session.run_start(req).await.unwrap();
    let mut log = started.log.unwrap();
    let (_, fin) = env.finished("perm").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    // what the agent received: allow_once ("allow-once") or reject_once ("reject"), never "always"
    assert!(
        fin.summary.starts_with(
            "perm: git-status=allow-once,git-push=reject,curl=reject,read-in=allow-once,\
             read-out=reject,edit-in=allow-once,edit-out=reject,fetch=reject\n"
        ),
        "{}",
        fin.summary
    );
    let t = transcript(&env, "perm");
    let perms = of_kind(&t, "permission");
    assert_eq!(perms.len(), 8);
    let by_id = |id: &str| {
        perms
            .iter()
            .find(|p| p["tool_call_id"] == id)
            .unwrap_or_else(|| panic!("no permission record for {id}"))
    };
    for (id, decision, rule) in [
        ("git-status", "allow", "shell"),
        ("git-push", "deny", "shell"),
        ("curl", "deny", "shell"),
        ("read-in", "allow", "read"),
        ("read-out", "deny", "read"),
        ("edit-in", "allow", "edit"),
        ("edit-out", "deny", "edit"),
        ("fetch", "deny", "fetch"),
    ] {
        let p = by_id(id);
        assert_eq!(p["decision"], decision, "{id}: {p}");
        assert_eq!(p["rule"], rule, "{id}: {p}");
        assert!(!p["reason"].as_str().unwrap().is_empty());
        let option = if decision == "allow" {
            "allow-once"
        } else {
            "reject"
        };
        assert_eq!(p["option_id"], option);
    }
    assert_eq!(
        by_id("git-push")["input"]["command"],
        "git push origin HEAD"
    );
    assert_eq!(by_id("curl")["tool_kind"], "execute");
    // the decisions were also streamed live
    let mut streamed = 0;
    while let Ok(Ok(Some(v))) = timeout(T, log.recv.recv_json::<serde_json::Value>()).await {
        if v["kind"] == "permission" {
            streamed += 1;
        }
    }
    assert_eq!(streamed, 8);
}

#[tokio::test]
async fn updates_are_recorded_as_acp_session_updates() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("upd", "NOOP", "main", "enzarb/issue-15", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("upd").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let t = transcript(&env, "upd");
    let kinds: Vec<&str> = t.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds.first(), Some(&"state"));
    assert_eq!(kinds.last(), Some(&"finished"));
    let updates: Vec<&str> = of_kind(&t, "update")
        .iter()
        .map(|r| r["update"]["sessionUpdate"].as_str().unwrap())
        .collect();
    assert_eq!(
        updates,
        [
            "agent_message_chunk",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "agent_message_chunk",
            "agent_message_chunk"
        ]
    );
    assert_eq!(of_kind(&t, "update")[0]["session_id"], "sess-1");
    let acp = of_kind(&t, "acp");
    assert_eq!(acp[0]["event"], "initialized");
    assert_eq!(acp[0]["agent_info"]["name"], "fake-acp-agent");
    assert_eq!(acp[1]["event"], "session");
    assert_eq!(acp[1]["session_id"], "sess-1");
    let result = of_kind(&t, "agent_result")[0];
    assert_eq!(result["stop_reason"], "end_turn");
    assert!(result["text"].as_str().unwrap().ends_with("```"));
}

#[tokio::test]
async fn deaf_agent_is_killed_after_the_cancel_grace() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("deaf", "SLEEPDEAF", "main", "enzarb/issue-16", false))
        .await
        .unwrap();
    loop {
        if session.run_status("deaf").await.unwrap().state == RunPhase::Running {
            sleep(Duration::from_millis(800)).await;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    let started = std::time::Instant::now();
    session.run_cancel("deaf").unwrap();
    let (_, fin) = env.finished("deaf").await;
    assert_eq!(fin.outcome, RunOutcome::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(!pid_alive(&env, "deaf"), "agent survived the cancel");
}

#[tokio::test]
async fn timeout_cancels_the_agent_and_fails_the_run() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = env.run("late2", "SLEEP", "main", "enzarb/issue-17", false);
    req.timeout_secs = 2;
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("late2").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("timed out"), "{}", fin.summary);
    let t = transcript(&env, "late2");
    assert!(
        of_kind(&t, "acp")
            .iter()
            .any(|r| r["event"] == "cancel" && r["reason"] == "TimedOut")
    );
    assert!(!pid_alive(&env, "late2"));
}

#[tokio::test]
async fn missing_branch_base_fails() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("nobase", "NOOP", "nope", "enzarb/x", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("nobase").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("not found"), "{}", fin.summary);
}

#[tokio::test]
async fn cancel_kills_the_process_group() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("slow", "SLEEP", "main", "enzarb/issue-5", false))
        .await
        .unwrap();
    loop {
        if session.run_status("slow").await.unwrap().state == RunPhase::Running {
            // give the agent a moment to start its turn
            sleep(Duration::from_millis(500)).await;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    session.run_cancel("slow").unwrap();
    let (_, fin) = env.finished("slow").await; // would take 60 s if not killed
    assert_eq!(fin.outcome, RunOutcome::Cancelled);
    assert_eq!(fin.exit_code, None);
    assert_eq!(
        session.run_status("slow").await.unwrap().state,
        RunPhase::Cancelled
    );
    // session/cancel reached the agent, which ended its turn gracefully
    let t = transcript(&env, "slow");
    assert!(of_kind(&t, "acp").iter().any(|r| r["event"] == "cancel"));
    assert!(
        of_kind(&t, "update")
            .iter()
            .any(|r| r["update"]["content"]["text"] == "got-cancel"),
        "{t:?}"
    );
    assert!(!pid_alive(&env, "slow"));
}

#[tokio::test]
async fn timeout_fails_the_run() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = env.run("late", "SLEEP", "main", "enzarb/issue-6", false);
    req.timeout_secs = 2;
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("late").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("timed out"), "{}", fin.summary);
}

#[tokio::test]
async fn unknown_run_and_invalid_request() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let err = session.run_status("ghost").await.unwrap_err();
    assert!(err.to_string().contains("unknown_run"), "{err}");
    let mut req = env.run("../evil", "NOOP", "main", "b", false);
    req.run_id = "../evil".into();
    let err = session.run_start(req).await.err().expect("rejected");
    assert!(err.to_string().contains("invalid_request"), "{err}");
}

#[tokio::test]
async fn interrupted_runs_fail_on_restart_and_pending_callbacks_resume() {
    let env = start(0).await;
    let rd = env.run_dir("stale");
    std::fs::create_dir_all(&rd).unwrap();
    let req = env.run("stale", "NOOP", "main", "enzarb/issue-7", false);
    std::fs::write(rd.join("request.json"), serde_json::to_vec(&req).unwrap()).unwrap();
    std::fs::write(
        rd.join("state.json"),
        serde_json::to_vec(&json!({
            "run_id": "stale", "state": "running", "repo_url": req.repo_url, "base_ref": "main",
            "branch": "enzarb/issue-7", "push": false, "created_ms": 1
        }))
        .unwrap(),
    )
    .unwrap();
    // a second manager over the same workspace recovers it
    let cfg = Config {
        workspace: env.workspace.clone(),
        callback_backoff: Duration::from_millis(100),
        ..Config::default()
    };
    let m = AgentRunManager::new(cfg, Reaper::new(false));
    m.recover();
    let (_, fin) = env.finished("stale").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("restarted"), "{}", fin.summary);
}

#[tokio::test]
async fn setup_token_pty_captures_the_token() {
    let env = start(0).await;
    let (session, mut events) = env.connect().await;
    let token = format!("sk-ant-oat01-{}", "xY9_-".repeat(20));
    let (a, b) = token.split_at(70);
    let script = format!(
        "echo setup-token >/dev/null; printf '\\033[1mYour token\\033[0m\\r\\n{a}\\r\\n{b}\\r\\n\\r\\nStore it'; sleep 1"
    );
    let _pty = session
        .open_pty(80, 24, Some(vec!["sh".into(), "-c".into(), script]))
        .await
        .unwrap();
    let ev = timeout(T, async {
        loop {
            if let Some(Event::Ctrl(Ctrl::CredentialCaptured { kind })) = events.next().await {
                return kind;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(ev, CredentialKind::ClaudeOauth);
    let p = env.workspace.join(".enzarb/claude-token");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), token);
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn successive_runs_reuse_a_branch_name() {
    let env = start(0).await;
    let (session, _events) = env.connect().await;
    for id in ["job-a", "job-b"] {
        session
            .run_start(env.run(id, "EDIT", "main", "enzarb/plan-22", false))
            .await
            .unwrap();
        let (_, fin) = env.finished(id).await;
        assert_eq!(fin.outcome, RunOutcome::Succeeded, "{id}: {}", fin.summary);
    }
}
