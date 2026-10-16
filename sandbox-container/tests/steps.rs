//! End-to-end tests of the step executor: a real sandbox-init server, the
//! executor's client session, git against local bare repos and a fake ACP
//! agent (`fixtures/fake-step-agent.mjs`, run with node).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use enzarb_proto::session::{Client, Event, Events, Session};
use enzarb_proto::{
    ClientKind, Ctrl, Mode, Runtime, Secret, SessionEvent, StepExec, StepFinished, StepOutput,
    StepPhase, StepStart, StepWorkspace,
};
use sandbox_init::reaper::Reaper;
use sandbox_init::repos::RepoLocks;
use sandbox_init::steps::{CONTINUE_PROMPT, StepManager};
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
        let config = Config {
            listen: ([127, 0, 0, 1], 0).into(),
            auth_token_file: self.dir.path().join("token"),
            workspace: self.ws.clone(),
            claude_token_file: self.dir.path().join("no-claude-token"),
            cancel_grace: Duration::from_secs(3),
            net_retry_delays: vec![],
            ..Config::default()
        };
        let reaper = Reaper::new(false);
        tokio::spawn(reaper.clone().run());
        let steps = StepManager::new(config.clone(), reaper.clone(), RepoLocks::default())
            .with_self_exe(env!("CARGO_BIN_EXE_sandbox-init").into());
        steps.recover();
        let listener = tokio::net::TcpListener::bind(config.listen).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = AppState::new(config, reaper).with_steps(steps);
        tokio::spawn(serve(listener, Arc::new(state)));
        addr
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
                  && echo to-stderr >&2 && test -n \"$GH_TOKEN\""
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
    assert_eq!(start(&s, &mut ev, st).await, (StepPhase::Running, false));
    let (evs, fin) = collect(&mut ev, "cmd-1", |_| false).await;
    let fin = fin.unwrap();
    assert_eq!(fin.exit, "success", "{fin:?}");
    assert_eq!(fin.outputs["res"], json!({"a": 1}));
    assert_eq!(fin.outputs["out"], json!("hi\n"));
    assert!(!fin.outputs.contains_key("gone"));
    let stdout = texts(&evs, "stdout");
    assert!(stdout.contains("to-stdout [redacted]"), "{stdout}");
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
        runtime: Runtime {
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
        },
        model: Some("claude-test-model".into()),
        prompt: prompt.into(),
        result_schema: schema,
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
