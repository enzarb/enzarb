//! End-to-end tests of the step executor: a real sandbox-init server, the
//! executor's client session, git against local bare repos and a fake ACP
//! agent (`fixtures/fake-step-agent.mjs`, run with node).

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use enzarb_proto::session::{Client, Event, Events, Session};
use enzarb_proto::{
    ClientKind, Ctrl, EnvironmentKind, Mode, Runtime, Secret, SessionEvent, StepExec, StepFinished,
    StepOutput, StepPhase, StepStart, StepWorkspace,
};
use sandbox_init::reaper::Reaper;
use sandbox_init::repos::RepoLocks;
use sandbox_init::steps::{CONTINUE_PROMPT, NO_RESULT_PROMPT, StepManager};
use sandbox_init::{AppState, Config, serve};
use serde_json::{Value, json};
use tokio::time::timeout;

const AUTH: &str = "test-auth-token-123456";
const T: Duration = Duration::from_secs(30);
const WS_TOKEN: &str = "ghs_workspaceTOKEN123456";
const API_KEY: &str = "super-secret-api-key-value";

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
            "-c",
            "protocol.file.allow=always",
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

struct Env {
    dir: tempfile::TempDir,
    ws: PathBuf,
    bare: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("workspace");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(dir.path().join("token"), AUTH).unwrap();
        std::fs::create_dir(dir.path().join("cgroup")).unwrap();
        std::fs::write(
            dir.path().join("cgroup/memory.events"),
            "oom 0\noom_kill 0\n",
        )
        .unwrap();
        let bare = dir.path().join("remote.git");
        git(
            dir.path(),
            &["init", "-q", "--bare", bare.to_str().unwrap()],
        );
        let seed = dir.path().join("seed");
        git(dir.path(), &["init", "-q", seed.to_str().unwrap()]);
        std::fs::write(seed.join("README.md"), "hi\n").unwrap();
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "init"]);
        git(&seed, &["push", "-q", bare.to_str().unwrap(), "main"]);
        Self { dir, ws, bare }
    }

    fn url(&self) -> String {
        format!("file://{}", self.bare.display())
    }

    async fn server(&self) -> SocketAddr {
        self.server_with_steps().await.0
    }

    async fn server_with_steps(&self) -> (SocketAddr, StepManager) {
        self.server_cfg(|_| {}).await
    }

    async fn server_cfg(&self, tweak: impl FnOnce(&mut Config)) -> (SocketAddr, StepManager) {
        let mut config = Config {
            listen: ([127, 0, 0, 1], 0).into(),
            auth_token_file: self.dir.path().join("token"),
            workspace: self.ws.clone(),
            claude_token_file: self.dir.path().join("no-claude-token"),
            cancel_grace: Duration::from_secs(3),
            net_retry_delays: vec![],
            cgroup_root: self.dir.path().join("cgroup"),
            ..Config::default()
        };
        tweak(&mut config);
        let reaper = Reaper::new(false);
        tokio::spawn(reaper.clone().run());
        let steps = StepManager::new(config.clone(), reaper.clone(), RepoLocks::default())
            .with_self_exe(env!("CARGO_BIN_EXE_sandbox-init").into());
        steps.recover();
        let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(config, reaper).with_steps(steps.clone());
        tokio::spawn(serve(listener, Arc::new(state)));
        (addr, steps)
    }

    fn workspace(&self, branch: &str) -> StepWorkspace {
        StepWorkspace {
            repo: "o/r".into(),
            url: self.url(),
            mode: Mode::Write,
            branch: branch.into(),
            base: "main".into(),
            push: true,
            token: Secret::new(WS_TOKEN),
        }
    }
}

async fn connect(addr: SocketAddr) -> (Session, Events) {
    timeout(
        T,
        Client::connect(&format!("ws://{addr}/v1/mux"), AUTH, ClientKind::Executor),
    )
    .await
    .unwrap()
    .unwrap()
}

/// Start `step` and subscribe; returns the accepted state.
async fn start(s: &Session, ev: &mut Events, step: StepStart) -> (StepPhase, bool) {
    let id = step.run_id.clone();
    s.send_ctrl(&Ctrl::StepStart(Box::new(step))).unwrap();
    let acc = loop {
        match timeout(T, ev.next()).await.unwrap().unwrap() {
            Event::Ctrl(Ctrl::StepAccepted {
                run_id,
                state,
                existing,
            }) if run_id == id => break (state, existing),
            Event::Ctrl(Ctrl::Error { message, .. }) => panic!("error: {message}"),
            _ => {}
        }
    };
    s.send_ctrl(&Ctrl::StepStatus {
        run_id: id,
        after_seq: 0,
    })
    .unwrap();
    acc
}

/// Collect events until `step_finished` (or until `stop` says so).
async fn collect(
    ev: &mut Events,
    id: &str,
    mut stop: impl FnMut(&SessionEvent) -> bool,
) -> (Vec<SessionEvent>, Option<StepFinished>) {
    let mut out: Vec<SessionEvent> = Vec::new();
    loop {
        match timeout(T, ev.next()).await.expect("timed out").unwrap() {
            Event::Ctrl(Ctrl::SessionEvent(e)) if e.run_id == id => {
                if out.iter().any(|o| o.seq == e.seq) {
                    continue;
                }
                let done = stop(&e);
                out.push(e);
                if done {
                    return (out, None);
                }
            }
            Event::Ctrl(Ctrl::StepFinished(f)) if f.run_id == id => return (out, Some(f)),
            _ => {}
        }
    }
}

fn texts(evs: &[SessionEvent], kind: &str) -> String {
    evs.iter()
        .filter(|e| e.kind == kind)
        .map(|e| e.payload["text"].as_str().unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("")
}

fn step(id: &str, exec: StepExec, workspaces: Vec<StepWorkspace>) -> StepStart {
    StepStart {
        run_id: id.into(),
        attempt: 1,
        task_id: "task-1".into(),
        workspaces,
        exec,
        outputs: Default::default(),
        env: [("API_KEY".to_string(), Secret::new(API_KEY))].into(),
        files: Default::default(),
        github_token: Some(Secret::new("ghs_githubTOKEN987654")),
    }
}

#[tokio::test]
async fn command_step_outputs_events_and_leased_push() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let mut st = step(
        "cmd-1",
        StepExec::Command {
            run: "echo hi > out.txt && echo '{\"a\":1}' > res.json && echo \"to-stdout $API_KEY\" \
                  && echo to-stderr >&2 && test -n \"$GH_TOKEN\" \
                  && printf 'protocol=https\\nhost=github.com\\n\\n' | git credential fill \
                     | grep -qx \"password=$GH_TOKEN\" \
                  && echo \"file $(cat .enzarb/sec.txt)\" && test \"$(stat -c %a .enzarb/sec.txt)\" = 600 \
                  && git add -A && git status --porcelain --ignored > status.txt"
                .into(),
            tools: vec![],
        },
        vec![env.workspace("feature/x")],
    );
    st.outputs = [
        (
            "res".to_string(),
            StepOutput {
                file: "res.json".into(),
                format: Some("json".into()),
                optional: false,
            },
        ),
        (
            "out".to_string(),
            StepOutput {
                file: "out.txt".into(),
                format: None,
                optional: false,
            },
        ),
        (
            "gone".to_string(),
            StepOutput {
                file: "missing.md".into(),
                format: None,
                optional: true,
            },
        ),
    ]
    .into();
    st.files = [(
        ".enzarb/sec.txt".to_string(),
        Secret::new("filesecretVALUE42"),
    )]
    .into();
    assert_eq!(start(&s, &mut ev, st).await, (StepPhase::Running, false));
    let (evs, fin) = collect(&mut ev, "cmd-1", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.outputs["res"], json!({"a": 1}));
    assert_eq!(fin.outputs["out"], json!("hi\n"));
    assert!(!fin.outputs.contains_key("gone"));
    let stdout = texts(&evs, "stdout");
    assert!(stdout.contains("to-stdout [redacted]"), "{stdout}");
    assert!(stdout.contains("file [redacted]"), "{stdout}");
    assert!(!stdout.contains("filesecretVALUE42"));
    let tree = git(
        &env.bare,
        &["ls-tree", "-r", "--name-only", "refs/heads/feature/x"],
    );
    assert!(!tree.contains("sec.txt"), "{tree}");
    assert!(texts(&evs, "stderr").contains("to-stderr"));
    // seq is dense from 1
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, i as u64 + 1);
    }
    let all = serde_json::to_string(&evs).unwrap();
    assert!(!all.contains(API_KEY) && !all.contains(WS_TOKEN));
    // pushed with the branch created from main
    assert_eq!(fin.git.len(), 1);
    let g = &fin.git[0];
    assert!(g.pushed);
    assert_eq!(
        git(&env.bare, &["rev-parse", "refs/heads/feature/x"]),
        g.head_sha
    );
    assert_eq!(
        git(&env.bare, &["show", &format!("{}:out.txt", g.head_sha)]),
        "hi"
    );
    // the token never reached disk
    let wsdir = env.ws.join("tasks/task-1/o/r");
    let cfg = std::fs::read_to_string(wsdir.join(".git/config")).unwrap();
    assert!(!cfg.contains(WS_TOKEN));

    // step_status after the fact replays and reports the result
    s.send_ctrl(&Ctrl::StepStatus {
        run_id: "cmd-1".into(),
        after_seq: evs.len() as u64 - 1,
    })
    .unwrap();
    let mut replayed = 0;
    loop {
        match timeout(T, ev.next()).await.unwrap().unwrap() {
            Event::Ctrl(Ctrl::SessionEvent(_)) => replayed += 1,
            Event::Ctrl(Ctrl::StepState { state, result, .. }) => {
                assert_eq!(state, StepPhase::Finished);
                assert_eq!(result.unwrap().exit, "success");
                break;
            }
            _ => {}
        }
    }
    assert_eq!(replayed, 1);
    // idempotent start
    let again = step(
        "cmd-1",
        StepExec::Command {
            run: "false".into(),
            tools: vec![],
        },
        vec![],
    );
    assert_eq!(start(&s, &mut ev, again).await, (StepPhase::Finished, true));

    // A second step on the same task loses its lease to a concurrent push.
    let st = step(
        "cmd-2",
        StepExec::Command {
            run: "sleep 2; echo more > b.txt".into(),
            tools: vec![],
        },
        vec![env.workspace("feature/x")],
    );
    start(&s, &mut ev, st).await;
    collect(&mut ev, "cmd-2", |e| e.payload.get("command").is_some()).await;
    let other = env.dir.path().join("other");
    git(
        env.dir.path(),
        &[
            "clone",
            "-q",
            "-b",
            "feature/x",
            &env.url(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("c.txt"), "x").unwrap();
    git(&other, &["add", "."]);
    git(&other, &["commit", "-q", "-m", "concurrent"]);
    git(&other, &["push", "-q", "origin", "feature/x"]);
    let theirs = git(&other, &["rev-parse", "HEAD"]);
    let (_, fin) = collect(&mut ev, "cmd-2", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert!(!fin.git[0].pushed);
    assert!(fin.error.unwrap().contains("push"));
    assert_eq!(
        git(&env.bare, &["rev-parse", "refs/heads/feature/x"]),
        theirs
    );
}

fn agent_exec(env: &Env, prompt: &str, schema: Option<Value>) -> StepExec {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/fake-step-agent.mjs"
    );
    StepExec::Agent {
        runtime: Box::new(Runtime {
            name: "fake".into(),
            install: vec![],
            protocol: "acp".into(),
            command: vec!["node".into(), fixture.into()],
            env: [(
                "FAKE_STATE".to_string(),
                env.dir
                    .path()
                    .join("agent-state")
                    .to_string_lossy()
                    .into_owned(),
            )]
            .into(),
            capabilities: json!({}),
        }),
        model: Some("claude-test-model".into()),
        prompt: prompt.into(),
        result_schema: schema,
        mode: None,
        ez: None,
    }
}

#[tokio::test]
async fn agent_step_events_redaction_and_submit_result() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    start(
        &s,
        &mut ev,
        step("ag-1", agent_exec(&env, "echo please", None), vec![]),
    )
    .await;
    let (evs, fin) = collect(&mut ev, "ag-1", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.usage.unwrap().input_tokens, Some(10));
    let kinds: Vec<&str> = evs.iter().map(|e| e.kind.as_str()).collect();
    for k in [
        "prompt",
        "assistant",
        "thinking",
        "tool_call",
        "tool_result",
        "usage",
    ] {
        assert!(kinds.contains(&k), "{k} missing in {kinds:?}");
    }
    let said = texts(&evs, "assistant");
    assert!(said.contains("hello world"), "{said}");
    assert!(
        said.contains("secret=[redacted] gh=[redacted] model=claude-test-model"),
        "{said}"
    );
    assert_eq!(texts(&evs, "prompt"), "echo please");

    let schema = json!({"type": "object", "required": ["verdict"],
        "properties": {"verdict": {"enum": ["approve", "reject"]}}});
    start(
        &s,
        &mut ev,
        step("ag-2", agent_exec(&env, "submit", Some(schema)), vec![]),
    )
    .await;
    let (evs, fin) = collect(&mut ev, "ag-2", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(
        texts(&evs, "assistant"),
        "tools=submit_result invalid=true valid=false"
    );
    assert_eq!(fin.result, Some(json!({"verdict": "approve"})));
}

#[tokio::test]
async fn agent_step_continues_until_the_result_is_submitted() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let schema = json!({"type": "object", "required": ["verdict"],
        "properties": {"verdict": {"enum": ["approve", "reject"]}}});
    start(
        &s,
        &mut ev,
        step("ag-c", agent_exec(&env, "bg run", Some(schema)), vec![]),
    )
    .await;
    let (evs, fin) = collect(&mut ev, "ag-c", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.result, Some(json!({"verdict": "approve"})));
    assert_eq!(texts(&evs, "prompt"), "bg run");
    let users: Vec<&str> = evs
        .iter()
        .filter(|e| e.kind == "user")
        .map(|e| e.payload["text"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(users, [NO_RESULT_PROMPT]);
    let sessions = evs
        .iter()
        .filter(|e| e.payload["event"] == "session")
        .count();
    assert_eq!(sessions, 1);
    // Usage of both turns is summed into the one step_finished.
    assert_eq!(fin.usage.unwrap().input_tokens, Some(7));
}

#[tokio::test]
async fn agent_step_without_tool_calls_is_not_continued() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let schema = json!({"type": "object"});
    start(
        &s,
        &mut ev,
        step("ag-q", agent_exec(&env, "quiet", Some(schema)), vec![]),
    )
    .await;
    let (evs, fin) = collect(&mut ev, "ag-q", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(
        fin.error.as_deref(),
        Some("agent finished without submitting a result")
    );
    assert_eq!(fin.result, None);
    assert_eq!(evs.iter().filter(|e| e.kind == "user").count(), 0);
}

#[tokio::test]
async fn agent_step_stops_after_a_turn_without_tool_calls() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let schema = json!({"type": "object"});
    start(
        &s,
        &mut ev,
        step("ag-t", agent_exec(&env, "twice", Some(schema)), vec![]),
    )
    .await;
    let (evs, fin) = collect(&mut ev, "ag-t", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(
        fin.error.as_deref(),
        Some("agent finished without submitting a result")
    );
    assert_eq!(fin.result, None);
    assert!(
        !evs.iter()
            .any(|e| e.payload["event"] == "invalid_result_file")
    );
    assert_eq!(evs.iter().filter(|e| e.kind == "user").count(), 1);
}

#[tokio::test]
async fn agent_step_without_schema_is_not_continued() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    start(
        &s,
        &mut ev,
        step("ag-n", agent_exec(&env, "bg run", None), vec![]),
    )
    .await;
    let (evs, fin) = collect(&mut ev, "ag-n", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    assert_eq!(evs.iter().filter(|e| e.kind == "user").count(), 0);
}

#[tokio::test]
async fn agent_step_steer_and_stop() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    start(
        &s,
        &mut ev,
        step("ag-s", agent_exec(&env, "slow", None), vec![]),
    )
    .await;
    collect(&mut ev, "ag-s", |e| e.kind == "prompt").await;
    s.send_ctrl(&Ctrl::StepSteer {
        run_id: "ag-s".into(),
        text: "more tests".into(),
    })
    .unwrap();
    let (evs, fin) = collect(&mut ev, "ag-s", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    assert_eq!(texts(&evs, "steer"), "more tests");
    assert!(texts(&evs, "assistant").contains("slow donesteered: more tests"));

    start(
        &s,
        &mut ev,
        step("ag-w", agent_exec(&env, "wait", None), vec![]),
    )
    .await;
    collect(&mut ev, "ag-w", |e| e.kind == "assistant").await;
    s.send_ctrl(&Ctrl::StepStop {
        run_id: "ag-w".into(),
    })
    .unwrap();
    let (_, fin) = collect(&mut ev, "ag-w", |_| false).await;
    assert_eq!(fin.unwrap().exit, "interrupted");
    s.send_ctrl(&Ctrl::StepStatus {
        run_id: "ag-w".into(),
        after_seq: 10_000,
    })
    .unwrap();
    loop {
        if let Event::Ctrl(Ctrl::StepState { state, .. }) =
            timeout(T, ev.next()).await.unwrap().unwrap()
        {
            assert_eq!(state, StepPhase::Interrupted);
            break;
        }
    }
    // step_resume with the secrets still in memory: session/load + continue
    s.send_ctrl(&Ctrl::StepResume {
        run_id: "ag-w".into(),
    })
    .unwrap();
    let (evs, fin) = collect(&mut ev, "ag-w", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    assert_eq!(texts(&evs, "user"), CONTINUE_PROMPT);
    assert!(texts(&evs, "assistant").contains("resumed"));
}

#[tokio::test]
async fn agent_step_resumes_after_a_restart() {
    let env = Env::new();
    let a = env.server().await;
    let (s, mut ev) = connect(a).await;
    start(
        &s,
        &mut ev,
        step("ag-r", agent_exec(&env, "wait", None), vec![]),
    )
    .await;
    let (before, _) = collect(&mut ev, "ag-r", |e| e.kind == "assistant").await;

    // A new sandbox-init on the same workspace: the step is interrupted.
    let b = env.server().await;
    let (s2, mut ev2) = connect(b).await;
    s2.send_ctrl(&Ctrl::StepResume {
        run_id: "ag-r".into(),
    })
    .unwrap();
    match timeout(T, ev2.next()).await.unwrap().unwrap() {
        Event::Ctrl(Ctrl::Error { code, .. }) => assert_eq!(code, "secrets_required"),
        _ => panic!("expected secrets_required"),
    }
    let acc = start(
        &s2,
        &mut ev2,
        step("ag-r", agent_exec(&env, "wait", None), vec![]),
    )
    .await;
    assert_eq!(acc, (StepPhase::Running, true));
    let (evs, fin) = collect(&mut ev2, "ag-r", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    // the old events are replayed once, the new ones continue the sequence
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, i as u64 + 1);
    }
    assert!(evs.len() > before.len());
    assert_eq!(texts(&evs[before.len()..], "user"), CONTINUE_PROMPT);
    assert!(!serde_json::to_string(&evs).unwrap().contains("REPLAYED"));
    // stop the first server's agent
    s.send_ctrl(&Ctrl::StepStop {
        run_id: "ag-r".into(),
    })
    .unwrap();
}

/// The executor sends only `step_start` (no `step_status`) and must still receive the run's
/// `session_event`s and its `step_finished` on that session.
#[tokio::test]
async fn the_starting_session_receives_events_and_the_result() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let st = step(
        "only-start-1",
        StepExec::Command {
            run: "echo started-only".into(),
            tools: vec![],
        },
        vec![env.workspace("feature/only-start")],
    );
    s.send_ctrl(&Ctrl::StepStart(Box::new(st))).unwrap();
    let (events, fin) = collect(&mut ev, "only-start-1", |_| false).await;
    let fin = fin.expect("step_finished on the starting session");
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert!(
        !events.is_empty(),
        "session events reach the starting session"
    );
}

/// A sandbox shutdown (SIGTERM, pod roll) interrupts a running agent step without reporting a
/// result; after the restart the step is `interrupted`, and `step_start` resumes the session.
#[tokio::test]
async fn shutdown_interrupts_a_running_step_and_it_resumes() {
    let env = Env::new();
    let (a, steps_a) = env.server_with_steps().await;
    let (s, mut ev) = connect(a).await;
    start(
        &s,
        &mut ev,
        step("ag-sd", agent_exec(&env, "wait", None), vec![]),
    )
    .await;
    let (before, _) = collect(&mut ev, "ag-sd", |e| e.kind == "assistant").await;

    steps_a.interrupt_all(Duration::from_secs(5)).await;
    // No result reaches the engine's session.
    let late = timeout(Duration::from_millis(800), async {
        loop {
            if let Event::Ctrl(Ctrl::StepFinished(f)) = ev.next().await.unwrap()
                && f.run_id == "ag-sd"
            {
                return f;
            }
        }
    })
    .await;
    assert!(late.is_err(), "no step_finished on shutdown: {late:?}");
    assert!(!env.ws.join(".enzarb/steps/ag-sd/finished.json").exists());

    // Restart on the same workspace: interrupted, and step_start resumes it.
    let b = env.server().await;
    let (s2, mut ev2) = connect(b).await;
    let acc = start(
        &s2,
        &mut ev2,
        step("ag-sd", agent_exec(&env, "wait", None), vec![]),
    )
    .await;
    assert_eq!(acc, (StepPhase::Running, true));
    let (evs, fin) = collect(&mut ev2, "ag-sd", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    assert!(evs.len() > before.len());
    assert_eq!(texts(&evs[before.len()..], "user"), CONTINUE_PROMPT);
}

/// Step outputs under `.enzarb/out/` are collected but never committed, and a later step on the
/// same task workspace checks out cleanly (no "local changes would be overwritten").
#[tokio::test]
async fn step_outputs_are_not_committed_and_the_next_step_checks_out() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let mut st = step(
        "out-1",
        StepExec::Command {
            run: "mkdir -p .enzarb/out && echo plan > .enzarb/out/plan.md && echo code > code.txt"
                .into(),
            tools: vec![],
        },
        vec![env.workspace("feature/out")],
    );
    st.outputs = [(
        "plan".to_string(),
        StepOutput {
            file: ".enzarb/out/plan.md".into(),
            format: None,
            optional: false,
        },
    )]
    .into();
    start(&s, &mut ev, st).await;
    let (_, fin) = collect(&mut ev, "out-1", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.outputs["plan"], "plan\n");
    let files = git(
        &env.bare,
        &["ls-tree", "-r", "--name-only", "refs/heads/feature/out"],
    );
    assert!(files.contains("code.txt"), "{files}");
    assert!(
        !files.contains(".enzarb/out"),
        "outputs were committed: {files}"
    );

    // A second step of the same task on the same branch: the stale output must not block it.
    let st2 = step(
        "out-2",
        StepExec::Command {
            run:
                "mkdir -p .enzarb/out && echo plan2 > .enzarb/out/plan.md && echo more >> code.txt"
                    .into(),
            tools: vec![],
        },
        vec![env.workspace("feature/out")],
    );
    start(&s, &mut ev, st2).await;
    let (_, fin2) = collect(&mut ev, "out-2", |_| false).await;
    assert_eq!(fin2.unwrap().exit, "success");
    let files = git(
        &env.bare,
        &["ls-tree", "-r", "--name-only", "refs/heads/feature/out"],
    );
    assert!(!files.contains(".enzarb/out"), "{files}");
}

/// Branches that still track `.enzarb/out/` (committed before outputs were excluded): a step on
/// one, then a step on another whose tracked copy differs, checks out cleanly.
#[tokio::test]
async fn tracked_step_outputs_do_not_block_switching_branches() {
    let env = Env::new();
    let seed = env.dir.path().join("seed");
    for (branch, text) in [("feature/t1", "one\n"), ("feature/t2", "two\n")] {
        git(&seed, &["checkout", "-q", "-B", branch, "main"]);
        std::fs::create_dir_all(seed.join(".enzarb/out")).unwrap();
        std::fs::write(seed.join(".enzarb/out/plan.md"), text).unwrap();
        git(&seed, &["add", "-f", "."]);
        git(&seed, &["commit", "-q", "-m", branch]);
        git(&seed, &["push", "-q", env.bare.to_str().unwrap(), branch]);
    }
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let mut ws1 = env.workspace("feature/t1");
    ws1.push = false;
    let st1 = step(
        "tr-1",
        StepExec::Command {
            run: "mkdir -p .enzarb/out && echo ctx > .enzarb/out/context.json".into(),
            tools: vec![],
        },
        vec![ws1],
    );
    start(&s, &mut ev, st1).await;
    let (_, fin1) = collect(&mut ev, "tr-1", |_| false).await;
    assert_eq!(fin1.unwrap().exit, "success");

    let st2 = step(
        "tr-2",
        StepExec::Command {
            run: "echo code > code.txt".into(),
            tools: vec![],
        },
        vec![env.workspace("feature/t2")],
    );
    start(&s, &mut ev, st2).await;
    let (_, fin2) = collect(&mut ev, "tr-2", |_| false).await;
    let fin2 = fin2.unwrap();
    assert_eq!(fin2.exit, "success", "{fin2:?}");
    let files = git(
        &env.bare,
        &["ls-tree", "-r", "--name-only", "refs/heads/feature/t2"],
    );
    assert!(files.contains("code.txt"), "{files}");
    assert!(!files.contains(".enzarb/out"), "{files}");
}

/// When space is low, build caches (`CACHEDIR.TAG` directories) go from idle task workspaces;
/// a task with a running step keeps its cache.
#[tokio::test]
async fn workspace_prune_spares_tasks_with_a_running_step() {
    let env = Env::new();
    let (addr, steps) = env.server_with_steps().await;
    let cache = |task: &str| env.ws.join("tasks").join(task).join("o/r/target");
    for t in ["task-1", "task-idle"] {
        std::fs::create_dir_all(cache(t)).unwrap();
        std::fs::write(
            cache(t).join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
    }
    std::fs::create_dir_all(env.ws.join("tasks/task-idle/o/r/src")).unwrap();
    let (s, mut ev) = connect(addr).await;
    // A running step of task-1 (`step` uses that task id).
    start(
        &s,
        &mut ev,
        step(
            "busy-1",
            StepExec::Command {
                run: "sleep 3".into(),
                tools: vec![],
            },
            vec![],
        ),
    )
    .await;
    // 100% free required: always "low", so every idle cache goes.
    // Like a full disk: nothing new can be created at the workspace root.
    let removed = common::read_only(&env.ws, || {
        sandbox_init::workspace_prune::prune_once(&env.ws, &steps, 100)
    });
    assert_eq!(removed, 1);
    assert!(cache("task-1").exists(), "the busy task kept its cache");
    assert!(
        !cache("task-idle").exists(),
        "the idle task's cache was removed"
    );
    assert!(
        std::fs::read_dir(env.ws.join("tasks/task-idle/o/r"))
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().contains("enzarb-prune")),
        "nothing left aside"
    );
    assert!(
        env.ws.join("tasks/task-idle/o/r/src").exists(),
        "sources stay"
    );
    s.step_stop("busy-1").ok();
}

/// Steps of different tasks on one repo get the same `CARGO_TARGET_DIR`; `start.env` wins.
#[tokio::test]
async fn steps_share_one_cargo_target_dir_per_repo() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let want = env.ws.join(".cache/target/o/r");
    let check = format!("test \"$CARGO_TARGET_DIR\" = {}", want.display());
    for (id, task, branch) in [
        ("sh-1", "task-a", "feature/a"),
        ("sh-2", "task-b", "feature/b"),
    ] {
        let mut st = step(
            id,
            StepExec::Command {
                run: check.clone(),
                tools: vec![],
            },
            vec![env.workspace(branch)],
        );
        st.task_id = task.into();
        start(&s, &mut ev, st).await;
        let (_, fin) = collect(&mut ev, id, |_| false).await;
        let fin = fin.unwrap();
        assert_eq!(fin.exit, "success", "{fin:?}");
    }
    assert!(want.is_dir());
}

/// The shared target dir is pruned last, only when idle, and not while a build is recent.
#[tokio::test]
async fn workspace_prune_frees_idle_shared_target_dirs() {
    let env = Env::new();
    let (_addr, steps) = env.server_with_steps().await;
    let dir = env.ws.join(".cache/target/o/r");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    // Just written: a build may be running.
    assert_eq!(
        sandbox_init::workspace_prune::prune_once(&env.ws, &steps, 100),
        0
    );
    assert!(dir.exists());
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::open(&dir)
        .unwrap()
        .set_modified(old)
        .unwrap();
    // Not low: untouched.
    assert_eq!(
        sandbox_init::workspace_prune::prune_once(&env.ws, &steps, 1),
        0
    );
    assert!(dir.exists());
    assert_eq!(
        sandbox_init::workspace_prune::prune_once(&env.ws, &steps, 100),
        1
    );
    assert!(!dir.exists());
}

/// A fresh task workspace on the default branch itself (a read step: triage, plan) has the
/// branch's files checked out. The clone is `--no-checkout`, and checking out the branch it is
/// already on changes nothing.
#[tokio::test]
async fn a_fresh_workspace_on_the_default_branch_has_its_files() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let mut ws = env.workspace("main");
    ws.mode = Mode::Read;
    ws.push = false;
    start(
        &s,
        &mut ev,
        step(
            "fresh-1",
            StepExec::Command {
                run: "test -f README.md".into(),
                tools: vec![],
            },
            vec![ws],
        ),
    )
    .await;
    let (_, fin) = collect(&mut ev, "fresh-1", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "README.md is missing: {fin:?}");
}

/// Uncommitted changes left on the default branch (a read step, or a clone never checked out)
/// do not block a later step on the task branch.
#[tokio::test]
async fn leftovers_on_the_default_branch_do_not_block_the_task_branch() {
    let env = Env::new();
    let seed = env.dir.path().join("seed");
    git(&seed, &["checkout", "-q", "-B", "feature/y", "main"]);
    std::fs::write(seed.join("README.md"), "changed on the branch\n").unwrap();
    git(&seed, &["commit", "-q", "-am", "y"]);
    git(
        &seed,
        &["push", "-q", env.bare.to_str().unwrap(), "feature/y"],
    );
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    // A read step on its own branch off main (as `enzarb/task-<id>`), which leaves README.md
    // modified and a file deleted from the index (like a clone that was never checked out).
    let mut ws = env.workspace("task/read");
    ws.mode = Mode::Read;
    ws.push = false;
    let st = step(
        "left-1",
        StepExec::Command {
            run: "echo scribble >> README.md && git rm -q --cached README.md".into(),
            tools: vec![],
        },
        vec![ws],
    );
    start(&s, &mut ev, st).await;
    let (_, fin) = collect(&mut ev, "left-1", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    // The task branch, whose README.md differs: the checkout must not abort.
    let st2 = step(
        "left-2",
        StepExec::Command {
            run: "grep -q 'changed on the branch' README.md".into(),
            tools: vec![],
        },
        vec![env.workspace("feature/y")],
    );
    start(&s, &mut ev, st2).await;
    let (_, fin2) = collect(&mut ev, "left-2", |_| false).await;
    let fin2 = fin2.unwrap();
    assert_eq!(fin2.exit, "success", "{fin2:?}");
}

/// Run a command step with no workspaces; returns how it finished.
async fn run_cmd(env: &Env, id: &str, cmd: String) -> StepFinished {
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    start(
        &s,
        &mut ev,
        step(
            id,
            StepExec::Command {
                run: cmd,
                tools: vec![],
            },
            vec![],
        ),
    )
    .await;
    collect(&mut ev, id, |_| false).await.1.unwrap()
}

#[tokio::test]
async fn code_failure_has_no_environment() {
    let env = Env::new();
    let fin = run_cmd(&env, "env-code", "exit 3".into()).await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(fin.environment, None);
    let ok = run_cmd(&env, "env-ok", "true".into()).await;
    assert_eq!(ok.exit, "success");
    assert_eq!(ok.environment, None);
}

#[tokio::test]
async fn enospc_in_the_output_is_disk_full() {
    let env = Env::new();
    let fin = run_cmd(
        &env,
        "env-disk",
        "echo 'write error: No space left on device' >&2; exit 1".into(),
    )
    .await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    let e = fin.environment.expect("environment");
    assert_eq!(e.kind, EnvironmentKind::DiskFull);
    assert!(!e.detail.is_empty());
}

#[tokio::test]
async fn exit_127_in_the_step_is_not_an_environment_failure() {
    let env = Env::new();
    let fin = run_cmd(
        &env,
        "env-127",
        "echo 'sh: x: command not found' >&2; exit 127".into(),
    )
    .await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(fin.environment, None);
}

#[tokio::test]
async fn a_hostname_error_in_the_step_output_is_a_code_failure() {
    let env = Env::new();
    let fin = run_cmd(
        &env,
        "env-host",
        "echo 'curl: Could not resolve host: example.invalid' >&2; exit 1".into(),
    )
    .await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(fin.environment, None);
}

/// A real full filesystem needs a mount; skipped where that is not permitted.
#[tokio::test]
async fn a_full_workspace_is_disk_full() {
    let env = Env::new();
    let mounted = Command::new("mount")
        .args(["-t", "tmpfs", "-o", "size=64k"])
        .arg("tmpfs")
        .arg(&env.ws)
        .status()
        .is_ok_and(|s| s.success());
    if !mounted {
        eprintln!("skipped: cannot mount a tmpfs here");
        return;
    }
    let fin = run_cmd(
        &env,
        "env-full",
        "dd if=/dev/zero of=big bs=1k count=1024 2>/dev/null; exit 1".into(),
    )
    .await;
    let _ = Command::new("umount").arg(&env.ws).status();
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(fin.environment.unwrap().kind, EnvironmentKind::DiskFull);
}

fn bump_oom(env: &Env) -> String {
    let f = env.dir.path().join("cgroup/memory.events");
    format!("printf 'oom 1\\noom_kill 1\\n' > {}", f.display())
}

#[tokio::test]
async fn an_oom_kill_is_reported() {
    let env = Env::new();
    let fin = run_cmd(&env, "env-oom", format!("{}; kill -9 $$", bump_oom(&env))).await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    let e = fin.environment.expect("environment");
    assert_eq!(e.kind, EnvironmentKind::Oom);
    assert!(e.detail.contains("oom_kill 0→1"), "{}", e.detail);
}

#[tokio::test]
async fn a_bare_sigkill_is_not_oom() {
    let env = Env::new();
    let fin = run_cmd(&env, "env-kill", "kill -9 $$".into()).await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(fin.environment, None);
}

#[tokio::test]
async fn a_stopped_step_is_not_oom_even_if_the_counter_moved() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let cmd = format!("{}; echo ready; sleep 60", bump_oom(&env));
    start(
        &s,
        &mut ev,
        step(
            "env-stop",
            StepExec::Command {
                run: cmd,
                tools: vec![],
            },
            vec![],
        ),
    )
    .await;
    collect(&mut ev, "env-stop", |e| {
        e.payload["text"]
            .as_str()
            .is_some_and(|t| t.contains("ready"))
    })
    .await;
    s.send_ctrl(&Ctrl::StepStop {
        run_id: "env-stop".into(),
    })
    .unwrap();
    let fin = collect(&mut ev, "env-stop", |_| false).await.1.unwrap();
    assert_eq!(fin.exit, "interrupted");
    assert_eq!(fin.environment, None);
}

fn sys_events<'a>(
    evs: &'a [SessionEvent],
    name: &'a str,
) -> impl Iterator<Item = &'a SessionEvent> {
    evs.iter()
        .filter(move |e| e.kind == "system" && e.payload["event"] == name)
}

#[tokio::test]
async fn environment_failed_agent_step_resumes_on_step_start() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let exec = || agent_exec(&env, "enospc", None);
    start(&s, &mut ev, step("ag-env", exec(), vec![])).await;
    let (first, fin) = collect(&mut ev, "ag-env", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "failure", "{fin:?}");
    assert_eq!(fin.environment.unwrap().kind, EnvironmentKind::DiskFull);
    let sid = sys_events(&first, "session").next().unwrap().payload["session_id"].clone();

    // The engine re-sends step_start with the same run_id: the step continues.
    let acc = start(&s, &mut ev, step("ag-env", exec(), vec![])).await;
    assert_eq!(acc, (StepPhase::Running, true));
    let (mut evs, fin) = collect(&mut ev, "ag-env", |_| false).await;
    evs.sort_by_key(|e| e.seq);
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.environment, None);
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, i as u64 + 1);
    }
    let loaded: Vec<_> = sys_events(&evs, "session_loaded").collect();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].payload["session_id"], sid);
    assert_eq!(sys_events(&evs, "session").count(), 1);
    assert!(texts(&evs[first.len()..], "assistant").contains("resumed"));
    assert_eq!(texts(&evs[first.len()..], "user"), CONTINUE_PROMPT);
}

#[tokio::test]
async fn code_failure_step_is_replayed_not_rerun() {
    let env = Env::new();
    let addr = env.server().await;
    let (s, mut ev) = connect(addr).await;
    let counter = env.dir.path().join("count");
    let cmd = format!("echo x >> {}; exit 3", counter.display());
    let exec = || StepExec::Command {
        run: cmd.clone(),
        tools: vec![],
    };
    start(&s, &mut ev, step("code-fail", exec(), vec![])).await;
    let (first, fin) = collect(&mut ev, "code-fail", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "failure");
    assert_eq!(fin.environment, None);

    let acc = start(&s, &mut ev, step("code-fail", exec(), vec![])).await;
    assert_eq!(acc, (StepPhase::Finished, true));
    // A replay: the logged events, then the unchanged result as `step_state`.
    let mut again = Vec::new();
    loop {
        match timeout(T, ev.next()).await.unwrap().unwrap() {
            Event::Ctrl(Ctrl::SessionEvent(e)) => again.push(e),
            Event::Ctrl(Ctrl::StepState { state, result, .. }) => {
                assert_eq!(state, StepPhase::Finished);
                assert_eq!(result.unwrap(), fin);
                break;
            }
            _ => {}
        }
    }
    assert!(again.len() <= first.len());
    assert_eq!(sys_events(&again, "resumed").count(), 0);
    assert_eq!(std::fs::read_to_string(&counter).unwrap(), "x\n");
}

/// A command that fails with ENOSPC once, then sleeps (marker file `m`).
fn flaky_cmd(env: &Env) -> String {
    let m = env.dir.path().join("m");
    format!(
        "if [ -e {m} ]; then echo again >> {m}; sleep 3; else touch {m}; \
         echo 'No space left on device' >&2; exit 1; fi",
        m = m.display()
    )
}

#[tokio::test]
async fn environment_failed_step_in_the_pruned_workspace_is_busy_again() {
    let env = Env::new();
    let (addr, steps) = env.server_with_steps().await;
    let cache = env.ws.join("tasks/task-1/o/r/target");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(
        cache.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    let (s, mut ev) = connect(addr).await;
    let exec = || StepExec::Command {
        run: flaky_cmd(&env),
        tools: vec![],
    };
    start(&s, &mut ev, step("env-busy", exec(), vec![])).await;
    let (_, fin) = collect(&mut ev, "env-busy", |_| false).await;
    assert_eq!(
        fin.unwrap().environment.unwrap().kind,
        EnvironmentKind::DiskFull
    );

    // Finished: the workspace is idle, so its cache is pruned.
    let acc = start(&s, &mut ev, step("env-busy", exec(), vec![])).await;
    assert_eq!(acc, (StepPhase::Running, true));
    let removed = common::read_only(&env.ws, || {
        sandbox_init::workspace_prune::prune_once(&env.ws, &steps, 100)
    });
    assert_eq!(removed, 0);
    assert!(cache.exists(), "the resumed step keeps its task busy");
    // The command ran again in the same run, and finished for good.
    let (_, fin) = collect(&mut ev, "env-busy", |_| false).await;
    assert_eq!(fin.unwrap().exit, "success");
    assert!(
        std::fs::read_to_string(env.dir.path().join("m"))
            .unwrap()
            .contains("again")
    );
}

// ---- ez_steps (ADR-0029 rollout step 4) ----

const MODES: &str = "default_mode = \"implement\"\n[modes.implement]\ndescription = \"x\"\n[modes.review]\ndescription = \"y\"\n";

impl Env {
    /// A server whose `ez` is the fake ACP agent and whose `mise` records its argv.
    async fn ez_server(&self) -> SocketAddr {
        use std::os::unix::fs::PermissionsExt;
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/fake-step-agent.mjs"
        );
        let ez = self.dir.path().join("ez");
        std::fs::write(
            &ez,
            format!(
                "#!/bin/sh\n[ \"$1\" = acp ] || exit 2\nFAKE_STATE={} exec node {fixture}\n",
                self.dir.path().join("agent-state").display()
            ),
        )
        .unwrap();
        let mise = self.dir.path().join("mise");
        std::fs::write(
            &mise,
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\n",
                self.dir.path().join("mise.log").display()
            ),
        )
        .unwrap();
        for p in [&ez, &mise] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (addr, _) = self
            .server_cfg(|c| {
                c.acp_bin = ez.to_string_lossy().into_owned();
                c.mise_bin = mise.to_string_lossy().into_owned();
            })
            .await;
        addr
    }
}

fn ez_exec(prompt: &str, mode: Option<&str>, schema: Option<Value>) -> StepExec {
    StepExec::Agent {
        runtime: Box::new(Runtime {
            name: "ez".into(),
            install: vec![],
            protocol: "acp".into(),
            command: vec!["ez".into(), "acp".into()],
            env: Default::default(),
            capabilities: json!({}),
        }),
        model: None,
        prompt: prompt.into(),
        result_schema: schema,
        mode: mode.map(String::from),
        ez: Some(enzarb_proto::EzSpec {
            modes_toml: MODES.into(),
            instruction_files: [("AGENTS.md".to_string(), "pinned rules\n".to_string())].into(),
        }),
    }
}

fn ez_step(id: &str, exec: StepExec, ws: StepWorkspace) -> StepStart {
    let mut s = step(id, exec, vec![ws]);
    s.task_id = "wf-run-1".into();
    s
}

async fn run_one(addr: SocketAddr, st: StepStart) -> (Vec<SessionEvent>, StepFinished) {
    let (s, mut ev) = connect(addr).await;
    let id = st.run_id.clone();
    start(&s, &mut ev, st).await;
    let (evs, fin) = collect(&mut ev, &id, |_| false).await;
    (evs, fin.unwrap())
}

#[tokio::test]
async fn ez_step_refuses_an_unknown_mode() {
    let env = Env::new();
    let addr = env.ez_server().await;
    let ws = env.workspace("ez/wf/run-1");
    let (_, fin) = run_one(
        addr,
        ez_step("m-1", ez_exec("ezwork", Some("nope"), None), ws),
    )
    .await;
    assert_eq!(fin.exit, "failure", "{fin:?}");
    let e = fin.error.unwrap();
    assert!(
        e.contains("unknown mode \"nope\"") && e.contains("implement, review"),
        "{e}"
    );
    assert!(!env.dir.path().join("agent-state/mode-set").exists());
}

#[tokio::test]
async fn ez_step_runs_in_the_mode_with_pinned_config_and_pushes_the_run_branch() {
    let env = Env::new();
    let addr = env.ez_server().await;
    let (evs, fin) = run_one(
        addr,
        ez_step(
            "m-2",
            ez_exec("ezwork a.txt", Some("review"), None),
            env.workspace("ez/wf/run-1"),
        ),
    )
    .await;
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(
        std::fs::read_to_string(env.dir.path().join("agent-state/mode-set")).unwrap(),
        "review"
    );
    assert_eq!(texts(&evs, "prompt"), "ezwork a.txt");
    let e: Value = serde_json::from_slice(
        &std::fs::read(env.dir.path().join("agent-state/ezenv.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(e["unattended"], "1");
    let conf = e["config"].as_str().unwrap();
    assert!(conf.ends_with(".enzarb/ez/m-2/config"), "{conf}");
    assert!(
        e["sessions"]
            .as_str()
            .unwrap()
            .ends_with(".enzarb/ez/m-2/sessions")
    );
    assert_eq!(
        std::fs::read_to_string(format!("{conf}/modes.toml")).unwrap(),
        MODES
    );
    assert!(
        std::fs::read_to_string(format!("{conf}/config.toml"))
            .unwrap()
            .contains("claude-code")
    );
    // Pinned copies are in the worktree but never committed; the agent's file is.
    let wt = env.ws.join("tasks/wf-run-1/o/r");
    assert_eq!(
        std::fs::read_to_string(wt.join("AGENTS.md")).unwrap(),
        "pinned rules\n"
    );
    assert!(fin.git[0].pushed, "{fin:?}");
    let files = git(&env.bare, &["ls-tree", "-r", "--name-only", "ez/wf/run-1"]);
    assert!(
        files.contains("a.txt") && !files.contains("AGENTS.md") && !files.contains(".ez"),
        "{files}"
    );
}

#[tokio::test]
async fn ez_nodes_of_one_run_share_the_worktree_and_branch() {
    let env = Env::new();
    let addr = env.ez_server().await;
    let (_, f1) = run_one(
        addr,
        ez_step(
            "n-1",
            ez_exec("ezwork one.txt", Some("implement"), None),
            env.workspace("ez/wf/run-2"),
        ),
    )
    .await;
    assert_eq!(f1.exit, "success", "{f1:?}");
    // The second node sees the first one's (pushed) work in the same checkout.
    let marker = env.ws.join("tasks/wf-run-1/o/r/one.txt");
    assert!(marker.exists());
    std::fs::write(env.ws.join("tasks/wf-run-1/o/r/.same-checkout"), "x").unwrap();
    let (_, f2) = run_one(
        addr,
        ez_step(
            "n-2",
            ez_exec("ezwork two.txt", Some("review"), None),
            env.workspace("ez/wf/run-2"),
        ),
    )
    .await;
    assert_eq!(f2.exit, "success", "{f2:?}");
    assert!(env.ws.join("tasks/wf-run-1/o/r/.same-checkout").exists());
    assert!(env.ws.join("tasks/wf-run-1/o/r/one.txt").exists());
    let log = git(&env.bare, &["log", "--format=%s", "ez/wf/run-2"]);
    assert_eq!(
        log.lines()
            .filter(|l| l.starts_with("enzarb: changes"))
            .count(),
        2,
        "{log}"
    );
    assert_eq!(f2.git[0].branch, "ez/wf/run-2");
}

#[tokio::test]
async fn ez_step_captures_the_submitted_result() {
    let env = Env::new();
    let addr = env.ez_server().await;
    let schema = json!({"type": "object", "required": ["verdict"],
        "properties": {"verdict": {"enum": ["approve", "reject"]}}});
    let (_, fin) = run_one(
        addr,
        ez_step(
            "r-1",
            ez_exec("submit it", Some("implement"), Some(schema)),
            env.workspace("ez/wf/run-3"),
        ),
    )
    .await;
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.result, Some(json!({"verdict": "approve"})));
}

#[tokio::test]
async fn ez_step_without_push_never_pushes() {
    let env = Env::new();
    let addr = env.ez_server().await;
    let mut ws = env.workspace("ez/wf/run-4");
    ws.push = false;
    let (_, fin) = run_one(
        addr,
        ez_step("p-1", ez_exec("ezwork np.txt", Some("implement"), None), ws),
    )
    .await;
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert!(!fin.git[0].pushed);
    let refs = git(&env.bare, &["branch", "--list", "ez/*"]);
    assert!(refs.is_empty(), "{refs}");
}

#[tokio::test]
async fn ez_command_node_runs_in_the_run_worktree() {
    let env = Env::new();
    let addr = env.ez_server().await;
    let (_, f1) = run_one(
        addr,
        ez_step(
            "c-0",
            ez_exec("ezwork made.txt", None, None),
            env.workspace("ez/wf/run-5"),
        ),
    )
    .await;
    assert_eq!(f1.exit, "success", "{f1:?}");
    let cmd = StepExec::Command {
        run: format!(
            "{}/mise run test -- 'a b' && test -f made.txt && pwd > where.txt",
            env.dir.path().display()
        ),
        tools: vec![],
    };
    let (_, f2) = run_one(addr, ez_step("c-1", cmd, env.workspace("ez/wf/run-5"))).await;
    assert_eq!(f2.exit, "success", "{f2:?}");
    assert_eq!(
        std::fs::read_to_string(env.dir.path().join("mise.log"))
            .unwrap()
            .trim(),
        "run test -- a b"
    );
    assert!(
        std::fs::read_to_string(env.ws.join("tasks/wf-run-1/o/r/where.txt"))
            .unwrap()
            .contains("wf-run-1")
    );
}
