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
    /// Status answered to the failing attempts.
    fail_status: Arc<Mutex<StatusCode>>,
    /// Every callback request seen, failed or not.
    attempts: Arc<Mutex<u32>>,
}

async fn cb(State(s): State<CbState>, h: HeaderMap, Json(b): Json<RunFinished>) -> StatusCode {
    *s.attempts.lock().unwrap() += 1;
    {
        let mut n = s.fail_first.lock().unwrap();
        if *n > 0 {
            *n -= 1;
            return *s.fail_status.lock().unwrap();
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
    cbs: CbState,
    dir: tempfile::TempDir,
    origin: PathBuf,
    workspace: PathBuf,
}

async fn start(fail_callbacks: u32) -> Env {
    start_with(fail_callbacks, None, None).await
}

/// `mise_script`: body of a fake `mise` that sandbox-init runs in each worktree;
/// the repo then carries a `mise.toml`, so a failing install fails the run.
/// `docker_script`: body of a fake `docker`; the repo then has a `workflows/`
/// dir, so run preparation starts the test Postgres through it.
async fn start_with(
    fail_callbacks: u32,
    mise_script: Option<&str>,
    docker_script: Option<&str>,
) -> Env {
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
    if mise_script.is_some() {
        std::fs::write(seed.join("mise.toml"), "[tools]\n").unwrap();
    }
    if docker_script.is_some() {
        std::fs::create_dir(seed.join("workflows")).unwrap();
        std::fs::write(seed.join("workflows/.keep"), "").unwrap();
    }
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
        fail_status: Arc::new(Mutex::new(StatusCode::INTERNAL_SERVER_ERROR)),
        attempts: Arc::default(),
    };
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let cb_url = format!("http://{}/cb", l.local_addr().unwrap());
    let app = Router::new().route("/cb", post(cb)).with_state(cbs.clone());
    tokio::spawn(async move { axum::serve(l, app).await });

    let fake_bin = |name: &str, body: &str| {
        let m = root.join(name);
        std::fs::write(&m, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&m, std::fs::Permissions::from_mode(0o755)).unwrap();
        m.to_string_lossy().into_owned()
    };
    let mise_bin = mise_script.map_or(String::new(), |body| fake_bin("fake-mise", body));
    let docker_bin = docker_script.map_or_else(
        || Config::default().docker_bin,
        |body| fake_bin("fake-docker", body),
    );
    let config = Config {
        mise_bin,
        docker_bin,
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
        cbs,
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
        .unwrap_or_else(|_| {
            let d = self.run_dir(id);
            panic!(
                "callback not received: {} {}",
                std::fs::read_to_string(d.join("state.json")).unwrap_or_default(),
                std::fs::read_to_string(d.join("transcript.jsonl")).unwrap_or_default()
            )
        })
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

    /// Make the next `n` callbacks answer `status`.
    fn fail_callbacks(&self, n: u32, status: StatusCode) {
        *self.cbs.fail_first.lock().unwrap() = n;
        *self.cbs.fail_status.lock().unwrap() = status;
    }

    fn attempts(&self) -> u32 {
        *self.cbs.attempts.lock().unwrap()
    }

    /// Wait until the run's `state.json` carries the `abandoned` callback marker.
    async fn abandoned(&self, id: &str) {
        let path = self.run_dir(id).join("state.json");
        timeout(T, async {
            loop {
                let v: Option<serde_json::Value> = std::fs::read(&path)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok());
                if v.is_some_and(|v| v["callback"] == "abandoned") {
                    return;
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("callback abandoned");
    }

    /// Serve `m` on a new port (with this env's auth token).
    async fn serve(&self, m: AgentRunManager) -> SocketAddr {
        let config = Config {
            listen: ([127, 0, 0, 1], 0).into(),
            auth_token_file: self.dir.path().join("auth-token"),
            workspace: self.workspace.clone(),
            ..Config::default()
        };
        let reaper = Reaper::new(false);
        tokio::spawn(reaper.clone().run());
        let state = AppState::new(config.clone(), reaper).with_run_manager(Arc::new(m));
        let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, Arc::new(state)));
        addr
    }

    /// A second manager over the same workspace, as after a pod roll.
    fn restart(&self) -> AgentRunManager {
        let cfg = Config {
            workspace: self.workspace.clone(),
            callback_backoff: Duration::from_millis(100),
            ..Config::default()
        };
        let m = AgentRunManager::new(cfg, Reaper::new(false));
        m.recover();
        m
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
    // the worktree is removed when the run ends; the branch survives in the clone
    assert!(!env.workspace.join("worktrees/run-job-1").exists());
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
async fn reported_usage_reaches_the_callback_run_state_and_state_file() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("u1", "USAGE", "main", "enzarb/issue-20", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("u1").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let u = fin.usage.clone().expect("usage in the callback");
    assert_eq!(u.input_tokens, Some(1200));
    assert_eq!(u.output_tokens, Some(340));
    assert_eq!(u.cache_read_input_tokens, Some(9000));
    assert_eq!(u.cache_creation_input_tokens, None); // not reported: unknown, not 0
    assert_eq!(u.cost_usd, Some(0.42)); // the reported total, not the 0.25 running cost
    assert_eq!(u.models, vec!["claude-sonnet-5-5".to_string()]);

    // run_state serves it too, and so does the persisted record
    let info = session.run_status("u1").await.unwrap();
    assert_eq!(info.usage, fin.usage);
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(env.run_dir("u1").join("state.json")).unwrap())
            .unwrap();
    assert_eq!(state["usage"]["input_tokens"], 1200);
    assert!(state["usage"].get("cache_creation_input_tokens").is_none());

    // a run that reports tokens but no cost keeps the cost unknown
    session
        .run_start(env.run("u2", "USAGENOCOST", "main", "enzarb/issue-21", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("u2").await;
    let u = fin.usage.expect("usage in the callback");
    assert_eq!((u.input_tokens, u.output_tokens), (Some(5), Some(6)));
    assert_eq!(u.cost_usd, None);

    // an agent that reports nothing leaves usage absent
    session
        .run_start(env.run("u3", "NOOP", "main", "enzarb/issue-22", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("u3").await;
    assert_eq!(fin.usage, None);
    let info = session.run_status("u3").await.unwrap();
    assert_eq!(info.usage, None);
}

#[tokio::test]
async fn npm_token_reaches_the_agent_env_but_is_redacted_and_never_persisted() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = env.run("npm", "NPMENV", "feature", "feature", false);
    req.npm_token = Some("ghp_npmsecret987".into());
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("npm").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let rd = env.run_dir("npm");
    let transcript = std::fs::read_to_string(rd.join("transcript.jsonl")).unwrap();
    // The agent saw a value (not "unset"), and it was redacted on the way out.
    assert!(transcript.contains("NODE=***"), "{transcript}");
    assert!(!fin.summary.contains("ghp_npmsecret987"));
    let out = Command::new("grep")
        .args(["-r", "ghp_npmsecret987"])
        .arg(env.dir.path())
        .output()
        .unwrap();
    assert!(
        out.stdout.is_empty(),
        "npm token persisted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let req: serde_json::Value =
        serde_json::from_slice(&std::fs::read(rd.join("request.json")).unwrap()).unwrap();
    assert!(req.get("npm_token").is_none());
}

#[tokio::test]
async fn without_an_npm_token_node_auth_token_is_unset() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("nonpm", "NPMENV", "feature", "feature", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("nonpm").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let t = std::fs::read_to_string(env.run_dir("nonpm").join("transcript.jsonl")).unwrap();
    // Nothing of ours is set; the agent only inherits the process's own value.
    let ambient = std::env::var("NODE_AUTH_TOKEN").unwrap_or_else(|_| "unset".into());
    assert!(t.contains(&format!("NODE={ambient}")), "{t}");
}

#[tokio::test]
async fn toolchain_is_installed_in_the_worktree_before_the_agent_runs() {
    let env = start_with(
        0,
        Some(r#"test "$1" = install && pwd > "$PWD/../mise-ran""#),
        None,
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
    let prep = of_kind(&t, "prepare")[0];
    assert_eq!(prep["ok"], true);
    assert_eq!(prep["steps"][0]["step"], "mise install");
    assert_eq!(prep["steps"][0]["status"], "ok");
}

#[tokio::test]
async fn a_failing_mise_install_is_logged_and_the_agent_still_runs() {
    let env = start_with(0, Some("echo boom >&2; exit 3"), None).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("tcf", "EDIT", "feature", "feature", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("tcf").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    let t = transcript(&env, "tcf");
    let prep = of_kind(&t, "prepare")[0];
    assert_eq!(prep["ok"], false);
    assert_eq!(prep["steps"][0]["status"], "failed");
    // Preparation is best-effort: the agent started anyway.
    assert!(!of_kind(&t, "acp").is_empty());
}

#[tokio::test]
async fn test_postgres_url_reaches_the_agent_env() {
    let docker =
        r#"case "$1" in inspect) echo running;; port) echo 127.0.0.1:55432;; *) exit 0;; esac"#;
    let env = start_with(0, None, Some(docker)).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("pg", "PGENV", "feature", "feature", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("pg").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let t = transcript(&env, "pg");
    let prep = of_kind(&t, "prepare")[0];
    let pg = prep["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step"] == "postgres")
        .unwrap();
    assert_eq!(pg["status"], "ok");
    let raw = std::fs::read_to_string(env.run_dir("pg").join("transcript.jsonl")).unwrap();
    assert!(
        raw.contains("PG=postgres://postgres:postgres@127.0.0.1:55432/postgres"),
        "{raw}"
    );
}

#[tokio::test]
async fn without_a_postgres_repo_the_url_is_unset() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("nopg", "PGENV", "feature", "feature", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("nopg").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    let raw = std::fs::read_to_string(env.run_dir("nopg").join("transcript.jsonl")).unwrap();
    assert!(raw.contains("PG=unset"), "{raw}");
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
async fn every_permission_is_allowed_and_recorded() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = env.run("perm", "PERM", "main", "enzarb/issue-14", false);
    req.log_channel = Some(7);
    let started = session.run_start(req).await.unwrap();
    let mut log = started.log.unwrap();
    let (_, fin) = env.finished("perm").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    // the sandbox is the boundary: every request gets allow_always
    let a = "allow-with-updates";
    assert!(
        fin.summary.starts_with(&format!(
            "perm: git-status={a},git-push={a},curl={a},read-in={a},\
             read-out={a},edit-in={a},edit-out={a},fetch={a}\n"
        )),
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
    for id in [
        "git-status",
        "git-push",
        "curl",
        "read-in",
        "read-out",
        "edit-in",
        "edit-out",
        "fetch",
    ] {
        let p = by_id(id);
        assert_eq!(p["decision"], "allow", "{id}: {p}");
        assert_eq!(p["rule"], "sandbox", "{id}: {p}");
        assert_eq!(p["option_id"], a);
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
    // Cancel only once the agent is inside its turn (its "sleep" tool call is in the
    // transcript); a fixed delay raced the agent's startup on slow runners.
    let in_turn = |env: &Env| {
        std::fs::read_to_string(env.run_dir("slow").join("transcript.jsonl"))
            .map(|t| t.contains(r#""toolCallId":"t0""#))
            .unwrap_or(false)
    };
    for _ in 0..600 {
        if in_turn(&env) {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(in_turn(&env), "agent never started its turn");
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
async fn rejected_callback_with_401_is_abandoned_after_one_attempt() {
    let env = start(0).await;
    env.fail_callbacks(u32::MAX, StatusCode::UNAUTHORIZED);
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("job-401", "EDIT", "main", "enzarb/issue-401", false))
        .await
        .unwrap();
    env.abandoned("job-401").await;
    sleep(Duration::from_millis(500)).await; // several backoff periods
    assert_eq!(env.attempts(), 1);
    let rd = env.run_dir("job-401");
    assert!(!rd.join("callback.json").exists());
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(rd.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["state"], "succeeded");
    assert_eq!(state["callback"], "abandoned");
    assert!(
        state["summary"]
            .as_str()
            .unwrap()
            .contains("done: enzarb/issue-401")
    );
    assert!(rd.join("transcript.jsonl").exists());
    // an abandoned run is terminal: it still answers status and holds nothing
    let info = session.run_status("job-401").await.unwrap();
    assert_eq!(info.state, RunPhase::Succeeded);
    session
        .run_start(env.run("job-next", "EDIT", "main", "enzarb/issue-401", false))
        .await
        .unwrap();
    // the new run is rejected with the same 401: let it settle (one attempt, then
    // abandoned) so the count below is not racing its in-flight callback
    env.abandoned("job-next").await;
    sleep(Duration::from_millis(500)).await;
    assert_eq!(env.attempts(), 2, "one attempt per run");

    // a restart does not resurrect the retries of either run
    let _m = env.restart();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(env.attempts(), 2, "no further attempts after a restart");
    assert!(!rd.join("callback.json").exists());
    assert!(!env.run_dir("job-next").join("callback.json").exists());
}

#[tokio::test]
async fn transient_callback_statuses_keep_retrying() {
    for status in [
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::REQUEST_TIMEOUT,
    ] {
        let env = start(0).await;
        env.fail_callbacks(2, status);
        let (session, _e) = env.connect().await;
        session
            .run_start(env.run("job-r", "NOOP", "main", "enzarb/issue-r", false))
            .await
            .unwrap();
        let (_, fin) = env.finished("job-r").await;
        assert_eq!(fin.run_id, "job-r", "{status}");
        assert_eq!(env.attempts(), 3, "{status}");
        sleep(Duration::from_millis(200)).await;
        let state: serde_json::Value = serde_json::from_slice(
            &std::fs::read(env.run_dir("job-r").join("state.json")).unwrap(),
        )
        .unwrap();
        assert!(state.get("callback").is_none(), "{state}");
        assert!(!env.run_dir("job-r").join("callback.json").exists());
    }
}

#[tokio::test]
async fn stale_callback_without_marker_gets_one_attempt_then_is_abandoned() {
    // the job-1812 situation: finished before the upgrade, callback.json left behind
    let env = start(0).await;
    env.fail_callbacks(u32::MAX, StatusCode::UNAUTHORIZED);
    let rd = env.run_dir("job-1812");
    std::fs::create_dir_all(&rd).unwrap();
    let req = env.run("job-1812", "NOOP", "main", "enzarb/issue-1812", false);
    std::fs::write(rd.join("request.json"), serde_json::to_vec(&req).unwrap()).unwrap();
    std::fs::write(
        rd.join("state.json"),
        serde_json::to_vec(&json!({
            "run_id": "job-1812", "state": "succeeded", "repo_url": req.repo_url,
            "base_ref": "main", "branch": "enzarb/issue-1812", "push": false,
            "summary": "done", "created_ms": 1, "finished_ms": 2
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(rd.join("transcript.jsonl"), "{\"kind\":\"finished\"}\n").unwrap();
    std::fs::write(
        rd.join("callback.json"),
        serde_json::to_vec(&json!({
            "url": env.cb_url, "token": "cbtok", "first_attempt_ms": 1,
            "body": {
                "run_id": "job-1812", "outcome": "succeeded", "exit_code": 0,
                "summary": "done", "branch": "enzarb/issue-1812", "head_sha": "", "pushed": false
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let _m = env.restart();
    env.abandoned("job-1812").await;
    sleep(Duration::from_millis(500)).await;
    assert_eq!(env.attempts(), 1);
    assert!(!rd.join("callback.json").exists());
    assert!(rd.join("transcript.jsonl").exists());
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(rd.join("state.json")).unwrap()).unwrap();
    assert_eq!(state["summary"], "done");

    // and a further restart makes no attempt at all
    let _m2 = env.restart();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(env.attempts(), 1);
}

async fn capture_via(session: &Session, events: &mut Events) -> (String, Ctrl) {
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
            if let Some(Event::Ctrl(m @ Ctrl::CredentialCaptured { .. })) = events.next().await {
                return m;
            }
        }
    })
    .await
    .unwrap();
    (token, ev)
}

#[tokio::test]
async fn setup_token_for_a_legacy_peer_writes_the_file_and_sends_no_token() {
    let env = start(0).await;
    // An old proxy: no `claude_user_token` feature. It relays frames verbatim.
    let url = format!("ws://{}/v1/mux", env.addr);
    let (session, mut events) = timeout(T, Client::connect(&url, TOKEN, ClientKind::Proxy))
        .await
        .unwrap()
        .unwrap();
    let (token, ev) = capture_via(&session, &mut events).await;
    assert_eq!(
        ev,
        Ctrl::CredentialCaptured {
            kind: CredentialKind::ClaudeOauth,
            token: None,
            legacy: None
        }
    );
    let p = env.workspace.join(".enzarb/claude-token");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), token);
    assert_eq!(
        std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn setup_token_for_a_user_token_proxy_sends_the_token_and_keeps_nothing() {
    let env = start(0).await;
    let url = format!("ws://{}/v1/mux", env.addr);
    let (session, mut events) = timeout(
        T,
        Client::connect_offering(
            &url,
            TOKEN,
            ClientKind::Proxy,
            vec![enzarb_proto::feature::CLAUDE_USER_TOKEN.into()],
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let (token, ev) = capture_via(&session, &mut events).await;
    let Ctrl::CredentialCaptured {
        kind,
        token: got,
        legacy,
    } = ev
    else {
        unreachable!()
    };
    assert_eq!(kind, CredentialKind::ClaudeOauth);
    assert_eq!(got.as_ref().map(|t| t.expose()), Some(token.as_str()));
    assert_eq!(legacy, None);
    // Nothing on disk: the legacy file keeps its old contents.
    let p = env.workspace.join(".enzarb/claude-token");
    assert_eq!(std::fs::read_to_string(&p).unwrap().trim(), CLAUDE_TOKEN);
    let out = Command::new("grep")
        .args(["-r", &token])
        .arg(env.dir.path())
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "captured token persisted");
}

#[tokio::test]
async fn run_start_token_is_used_redacted_and_never_persisted() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    // A different (wrong) legacy token proves the per-run one takes precedence.
    std::fs::write(env.workspace.join(".enzarb/claude-token"), "wrong\n").unwrap();
    let tok = "sk-ant-oat01-PERRUNtoken";
    let mut req = env.run("perrun", "CLAUDEENV", "feature", "feature", false);
    req.claude_token = Some(tok.into());
    assert!(!format!("{req:?}").contains(tok), "RunStart Debug leaks");
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("perrun").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    let t = std::fs::read_to_string(env.run_dir("perrun").join("transcript.jsonl")).unwrap();
    assert!(t.contains("CL=***"), "{t}");
    assert!(!t.contains("bad token"), "{t}");
    assert!(!fin.summary.contains(tok));
    let out = Command::new("grep")
        .args(["-r", tok])
        .arg(&env.workspace)
        .output()
        .unwrap();
    assert!(
        out.stdout.is_empty(),
        "per-run token persisted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // The legacy file is untouched.
    assert_eq!(
        std::fs::read_to_string(env.workspace.join(".enzarb/claude-token")).unwrap(),
        "wrong\n"
    );

    // Without a file, the per-run token alone suffices.
    std::fs::remove_file(env.workspace.join(".enzarb/claude-token")).unwrap();
    let mut req = env.run("perrun2", "NOOP", "feature", "feature", false);
    req.claude_token = Some(tok.into());
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("perrun2").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);

    // An empty per-run token is absent: no file, so no_credential.
    let mut req = env.run("perrun3", "NOOP", "feature", "feature", false);
    req.claude_token = Some("  ".into());
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("perrun3").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("no_credential"), "{}", fin.summary);
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

fn make_stale(env: &Env, name: &str, age_secs: u64) -> PathBuf {
    let d = env.workspace.join("worktrees").join(name);
    std::fs::create_dir_all(d.join("target/debug")).unwrap();
    std::fs::write(d.join("target/debug/big"), "x").unwrap();
    let t = std::time::SystemTime::now() - Duration::from_secs(age_secs);
    std::fs::File::open(&d).unwrap().set_modified(t).unwrap();
    d
}

#[tokio::test]
async fn finished_runs_drop_their_worktree_but_keep_logs() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("clean", "EDIT", "main", "enzarb/clean", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("clean").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    assert!(!env.workspace.join("worktrees/run-clean").exists());
    assert!(env.run_dir("clean").join("transcript.jsonl").exists());
    assert!(env.run_dir("clean").join("state.json").exists());
    // git forgot the worktree too
    let repos = env.workspace.join("repos");
    let repo = std::fs::read_dir(&repos)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let list = git(&repo, &["worktree", "list", "--porcelain"]);
    assert!(!list.contains("run-clean"), "{list}");
}

#[tokio::test]
async fn failed_runs_drop_their_worktree_too() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("bad", "FAIL", "main", "enzarb/bad", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("bad").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(!env.workspace.join("worktrees/run-bad").exists());
}

#[tokio::test]
async fn a_new_run_prunes_stale_worktrees_but_not_fresh_or_foreign_dirs() {
    let env = start(0).await;
    // let the startup sweep (spawned by recover) finish first
    sleep(Duration::from_millis(300)).await;
    let stale = make_stale(&env, "run-old-job", 3600);
    let fresh = make_stale(&env, "run-just-now", 1);
    let other = make_stale(&env, "keepme", 3600);
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("next", "EDIT", "main", "enzarb/next", false))
        .await
        .unwrap();
    let (_, fin) = env.finished("next").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    assert!(!stale.exists());
    assert!(fresh.exists(), "recent dirs are left alone");
    assert!(other.exists(), "only worktrees/run-* is ever pruned");
}

#[tokio::test]
async fn startup_prunes_every_run_worktree() {
    let env = start(0).await;
    let a = make_stale(&env, "run-a", 1);
    let b = make_stale(&env, "run-b", 5000);
    let other = make_stale(&env, "toolchains", 5000);
    let file = env.workspace.join("worktrees/run-x.pid");
    std::fs::write(&file, "1").unwrap();
    let cfg = Config {
        workspace: env.workspace.clone(),
        ..Config::default()
    };
    AgentRunManager::new(cfg, Reaper::new(false)).recover();
    timeout(T, async {
        while a.exists() || b.exists() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(other.exists());
    assert!(file.exists());
}

/// Issue #52: an agent with the sandbox's uid plants config and hooks in the
/// shared clone. sandbox-init's authenticated fetch and push must run none of
/// it, never expose the token to it, and still reach the real origin.
#[tokio::test]
async fn planted_git_config_and_hooks_neither_run_nor_redirect_nor_see_the_token() {
    use base64::Engine as _;
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(env.run("first", "DIRTY", "main", "enzarb/first", true))
        .await
        .unwrap();
    assert!(env.finished("first").await.1.pushed);

    let root = env.dir.path();
    let marks = root.join("marks");
    std::fs::create_dir(&marks).unwrap();
    let evil = root.join("evil.git");
    git(root, &["init", "-q", "--bare", evil.to_str().unwrap()]);
    let hook = |dir: &Path, name: &str| {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nenv > {}/{name}\n", marks.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    let clone = env
        .workspace
        .join("repos")
        .join(sandbox_init::agent::repo_dir_name(&format!(
            "file://{}",
            env.origin.display()
        )));
    let names = [
        "pre-push",
        "post-checkout",
        "reference-transaction",
        "pre-commit",
        "post-commit",
        "fsmonitor-watchman",
    ];
    for n in names {
        hook(&clone.join(".git/hooks"), n);
        hook(&root.join("evilhooks"), n);
    }
    let script = |name: &str| {
        let p = root.join(format!("{name}.sh"));
        std::fs::write(&p, format!("#!/bin/sh\nenv > {}/{name}\n", marks.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    };
    let origin_url = format!("file://{}", env.origin.display());
    let evil_url = format!("file://{}", evil.display());
    let inc = root.join("inc.cfg");
    std::fs::write(
        &inc,
        format!(
            "[core]\n\thooksPath = {}\n[url \"{evil_url}\"]\n\tinsteadOf = {origin_url}\n\tpushInsteadOf = {origin_url}\n",
            root.join("evilhooks").display()
        ),
    )
    .unwrap();
    let cfg = clone.join(".git/config");
    let mut c = std::fs::read_to_string(&cfg).unwrap();
    c.push_str(&format!(
        "[core]\n\tfsmonitor = {}\n\tsshCommand = {}\n\thooksPath = {}\n\
         [credential]\n\thelper = !{}\n\
         [url \"{evil_url}\"]\n\tinsteadOf = {origin_url}\n\tpushInsteadOf = {origin_url}\n\
         [remote \"origin\"]\n\turl = {evil_url}\n\tpushurl = {evil_url}\n\
         [include]\n\tpath = {inc}\n[includeIf \"gitdir:/\"]\n\tpath = {inc}\n",
        script("fsmonitor"),
        script("ssh"),
        root.join("evilhooks").display(),
        script("credential"),
        inc = inc.display(),
    ));
    std::fs::write(&cfg, c).unwrap();

    // fetch (clone exists), worktree, leftovers commit, push
    session
        .run_start(env.run("second", "DIRTY", "main", "enzarb/second", true))
        .await
        .unwrap();
    let (_, fin) = env.finished("second").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{fin:?}");
    assert!(fin.pushed);
    assert_eq!(env.remote_sha("enzarb/second"), Some(fin.head_sha.clone()));
    let leaked: Vec<_> = std::fs::read_dir(&marks)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert!(leaked.is_empty(), "planted code ran: {leaked:?}");
    let refs = Command::new("git")
        .current_dir(&evil)
        .args(["for-each-ref"])
        .output()
        .unwrap();
    assert!(refs.stdout.is_empty(), "pushed to the planted remote");
    // the token (raw or in the basic header) is in no file anywhere
    let b64 = base64::engine::general_purpose::STANDARD.encode("x-access-token:secret-clone-token");
    for needle in ["secret-clone-token", b64.as_str()] {
        let out = Command::new("grep")
            .args(["-r", "-F", needle])
            .arg(root)
            .output()
            .unwrap();
        assert!(
            out.stdout.is_empty(),
            "token persisted: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let left = std::fs::read_dir(env.workspace.join(".enzarb/net"))
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "scratch repos removed");
}

/// `git` as the sandbox's own configured author (`Config::default()`).
fn git_agent(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=enzarb-agent",
            "-c",
            "user.email=agent@enzarb.dev",
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

#[tokio::test]
async fn retarget_run_rebases_the_child_without_an_agent_or_claude_token() {
    let env = start(0).await;
    // No claude token anywhere: a retarget run must not need one.
    std::fs::remove_file(env.workspace.join(".enzarb/claude-token")).unwrap();
    let seed = env.dir.path().join("rt-seed");
    git(
        env.dir.path(),
        &[
            "clone",
            "-q",
            env.origin.to_str().unwrap(),
            seed.to_str().unwrap(),
        ],
    );
    git_agent(&seed, &["checkout", "-qb", "enzarb/issue-1"]);
    std::fs::write(seed.join("p.txt"), "p\n").unwrap();
    git_agent(&seed, &["add", "."]);
    git_agent(&seed, &["commit", "-qm", "parent"]);
    let parent = git_agent(&seed, &["rev-parse", "HEAD"]);
    git_agent(&seed, &["checkout", "-qb", "enzarb/issue-2"]);
    std::fs::write(seed.join("c.txt"), "c\n").unwrap();
    git_agent(&seed, &["add", "."]);
    git_agent(&seed, &["commit", "-qm", "child"]);
    let child = git_agent(&seed, &["rev-parse", "HEAD"]);
    git_agent(&seed, &["checkout", "-q", "main"]);
    std::fs::write(seed.join("p.txt"), "p\n").unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-qm", "parent (#1) squashed"]);
    let main = git(&seed, &["rev-parse", "HEAD"]);
    for b in ["main", "enzarb/issue-1", "enzarb/issue-2"] {
        git(&seed, &["push", "-q", "origin", b]);
    }

    let (session, _events) = env.connect().await;
    let mut req = env.run("rt-1", "ignored", "main", "enzarb/issue-2", false);
    req.retarget = Some(enzarb_proto::Retarget {
        head_ref: "enzarb/issue-2".into(),
        onto: "main".into(),
        old_base: parent,
        expected_sha: child.clone(),
        protected: vec![],
    });
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("rt-1").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    assert!(fin.pushed);
    let v: serde_json::Value = serde_json::from_str(&fin.summary).unwrap();
    assert_eq!(v["outcome"], "retargeted", "{v}");
    let now = env.remote_sha("enzarb/issue-2").unwrap();
    assert_ne!(now, child);
    assert_eq!(v["head_sha"], now.as_str());
    assert_eq!(fin.head_sha, now);
    assert_eq!(env.remote_sha("main").unwrap(), main);
    let parent_of = git(&env.origin, &["rev-parse", &format!("{now}~1")]);
    assert_eq!(parent_of, main);
    assert!(!env.workspace.join("worktrees/run-rt-1").exists());

    // A request that fails validation never reaches the network and is not pushed.
    let mut req = env.run("rt-2", "ignored", "main", "main", true);
    req.retarget = Some(enzarb_proto::Retarget {
        head_ref: "main".into(),
        onto: "main".into(),
        old_base: main.clone(),
        expected_sha: main.clone(),
        protected: vec![],
    });
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("rt-2").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    assert!(!fin.pushed);
    let v: serde_json::Value = serde_json::from_str(&fin.summary).unwrap();
    assert_eq!(v["outcome"], "rejected");
    assert_eq!(env.remote_sha("main").unwrap(), main);
}

// ---- graph segment runs (ADR-0020) ----

const SEGMENT: &str = r#"
version: 1
name: issue
on: [issue_opened]
start: hello
nodes:
  hello: {type: command, run: "echo hi; echo $ENZARB_RUN_ID > made.txt", next: {"exit_code == 0": check, else: failed}}
  check: {type: command, run: "grep -q seg made.txt && echo found", next: implement}
  implement: {type: agent, prompt: implement, next: {pushed: open_pr, else: failed}}
  broken: {type: command, run: "exit 3", next: open_pr}
  open_pr: {type: action, action: open_change_request, next: done}
  failed: {type: end, status: failed}
  done: {type: end, status: done}
"#;

fn segment(env: &Env, id: &str, start: &str) -> RunStart {
    let mut req = env.run(id, "ignored", "main", &format!("enzarb/{id}"), true);
    req.graph = Some(enzarb_proto::GraphSegment {
        definition: serde_json::Value::String(SEGMENT.into()),
        start: start.into(),
        prompts: [("implement".to_string(), "EDIT the thing".to_string())].into(),
        ..Default::default()
    });
    req
}

#[tokio::test]
async fn segment_runs_commands_and_agent_until_an_action() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(segment(&env, "seg-1", "hello"))
        .await
        .unwrap();
    let (_, fin) = env.finished("seg-1").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    let g = fin.graph.expect("graph result");
    assert_eq!(g.stopped_at, "open_pr");
    assert_eq!(g.outputs["hello"]["stdout"], "hi\n");
    assert_eq!(g.outputs["hello"]["exit_code"], 0);
    assert_eq!(g.outputs["check"]["stdout"], "found\n");
    // The agent got the rendered prompt, committed and pushed.
    assert_eq!(g.outputs["implement"]["decision"], "done");
    assert_eq!(g.outputs["implement"]["pushed"], true);
    assert_eq!(g.visits["implement"], 1);
    assert!(fin.pushed);
    assert_eq!(Some(fin.head_sha.clone()), env.remote_sha("enzarb/seg-1"));
    assert_eq!(fin.summary, SUMMARY.replace("issue-1", "seg-1"));
}

#[tokio::test]
async fn segment_failing_command_fails_the_run() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(segment(&env, "seg-2", "broken"))
        .await
        .unwrap();
    let (_, fin) = env.finished("seg-2").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert_eq!(fin.summary, "node `broken` failed");
    assert!(!fin.pushed);
    let g = fin.graph.unwrap();
    assert_eq!(g.stopped_at, "broken");
    assert_eq!(g.outputs["broken"]["exit_code"], 3);
    assert_eq!(g.outputs["broken"]["ok"], false);
}

#[tokio::test]
async fn segment_starting_at_a_kernel_node_runs_nothing() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    session
        .run_start(segment(&env, "seg-3", "open_pr"))
        .await
        .unwrap();
    let (_, fin) = env.finished("seg-3").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    assert_eq!(fin.graph.unwrap().stopped_at, "open_pr");
    assert_eq!(env.remote_sha("enzarb/seg-3"), None);
}

#[tokio::test]
async fn segment_with_an_invalid_definition_fails() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = segment(&env, "seg-4", "hello");
    req.graph.as_mut().unwrap().definition =
        serde_json::Value::String("version: 1\nname: x\non: [a]\nstart: a\nnodes: {}".into());
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("seg-4").await;
    assert_eq!(fin.outcome, RunOutcome::Failed);
    assert!(fin.summary.contains("invalid"), "{}", fin.summary);
}

#[tokio::test]
async fn segment_agent_without_new_work_is_not_pushed() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = segment(&env, "seg-5", "implement");
    req.graph
        .as_mut()
        .unwrap()
        .prompts
        .insert("implement".into(), "LOOK only".into());
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("seg-5").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    let g = fin.graph.unwrap();
    assert_eq!(g.outputs["implement"]["pushed"], false);
    assert_eq!(g.stopped_at, "failed");
    assert!(!fin.pushed);
    assert_eq!(env.remote_sha("enzarb/seg-5"), None);
}

#[tokio::test]
async fn segment_command_edits_are_pushed_at_the_stop() {
    let env = start(0).await;
    let (session, _e) = env.connect().await;
    let mut req = segment(&env, "seg-6", "edit");
    req.graph.as_mut().unwrap().definition = serde_json::Value::String(
        "version: 1\nname: t\non: [issue_opened]\nstart: edit\nnodes:\n  edit: {type: command, run: \"echo x > c.txt\", next: open_pr}\n  open_pr: {type: action, action: open_change_request, next: done}\n  done: {type: end, status: done}\n".into(),
    );
    session.run_start(req).await.unwrap();
    let (_, fin) = env.finished("seg-6").await;
    assert_eq!(fin.outcome, RunOutcome::Succeeded, "{}", fin.summary);
    assert!(fin.pushed);
    assert_eq!(Some(fin.head_sha.clone()), env.remote_sha("enzarb/seg-6"));
}

/// A run started without a callback (`feature::RUN_RESULTS`): the result is
/// pushed as `run_finished` to the starting session, nothing is POSTed, no
/// `callback.json` (or token) is written, and `run_status` returns the full
/// result, also after sandbox-init restarts.
#[tokio::test]
async fn run_without_callback_delivers_run_finished_and_run_status_after_restart() {
    let env = start(0).await;
    let (session, mut events) = env.connect().await;
    assert!(
        session.has_feature(enzarb_proto::feature::RUN_RESULTS),
        "{:?}",
        session.features()
    );
    let mut req = env.run("job-nocb", "EDIT", "main", "enzarb/issue-9", true);
    req.callback_url = String::new();
    req.callback_token = String::new();
    session.run_start(req).await.unwrap();
    let fin = timeout(T, async {
        loop {
            match events.next().await {
                Some(Event::Ctrl(Ctrl::RunFinished(f))) => return f,
                Some(_) => {}
                None => panic!("session ended"),
            }
        }
    })
    .await
    .expect("run_finished pushed");
    assert_eq!(fin.run_id, "job-nocb");
    assert_eq!(fin.outcome, RunOutcome::Succeeded);
    assert!(fin.pushed);
    assert_eq!(Some(fin.head_sha.clone()), env.remote_sha("enzarb/issue-9"));

    let rd = env.run_dir("job-nocb");
    assert!(!rd.join("callback.json").exists(), "no callback file");
    let request = std::fs::read_to_string(rd.join("request.json")).unwrap();
    assert!(!request.contains("callback"), "{request}");
    sleep(Duration::from_millis(300)).await;
    assert!(env.got.lock().unwrap().is_empty(), "nothing POSTed");
    assert_eq!(env.attempts(), 0);

    let st = session.run_status("job-nocb").await.unwrap();
    assert_eq!(st.result.as_ref(), Some(&fin));

    // A subscription after the end is answered at once.
    session.run_subscribe(vec!["job-nocb".into()]).unwrap();
    let again = timeout(T, async {
        loop {
            if let Some(Event::Ctrl(Ctrl::RunFinished(f))) = events.next().await {
                return f;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(again, fin);

    // After a restart the result is still there.
    let addr = env.serve(env.restart()).await;
    let url = format!("ws://{addr}/v1/mux");
    let (s2, _e2) = timeout(T, Client::connect(&url, TOKEN, ClientKind::Executor))
        .await
        .unwrap()
        .unwrap();
    let st = s2.run_status("job-nocb").await.unwrap();
    assert_eq!(st.state, RunPhase::Succeeded);
    assert_eq!(st.result, Some(fin));
    assert!(!rd.join("callback.json").exists());
}
