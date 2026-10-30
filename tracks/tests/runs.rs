//! The run executor against a real Postgres (testcontainers) and a fake sandbox that speaks the
//! real step frames over `enzarb_proto::session::memory` pairs: an agent -> command -> end path
//! (with the result feeding an edge), failures, `no_credential`, resume after a tracks restart
//! (the step is not started again), unsupported nodes, Stop, and the run endpoints.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use enzarb_proto::session::memory::session_pair_with;
use enzarb_proto::session::{Event, ServerOptions, Session, StepInfo};
use enzarb_proto::{
    ClientKind, Ctrl, Secret, StepExec, StepFinished, StepGit, StepPhase, StepStart, feature,
};
use enzarb_tracks::exec::{ExecConfig, Executor};
use enzarb_tracks::github::{MODES_PATH, RepoSource, Snapshot, SourceError, WORKFLOWS_PATH};
use enzarb_tracks::projects::{ProjectLookup, ProjectRef};
use enzarb_tracks::sandbox::{ClaudeTokens, Link, LinkError, Sandboxes, SessionLink};
use enzarb_tracks::{api, db};
use ez_workflow::{Event as WfEvent, RunInfo, RunState};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tower::ServiceExt;

const MODES: &str = r#"
default_mode = "triage"
[modes.triage]
description = "Understand an issue"
tools = ["Read"]
"#;

/// agent (result feeds the edge) -> command (exit feeds the edge) -> end.
const WORKFLOWS: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "triage"
[workflows.wf.nodes.triage]
type = "agent"
mode = "triage"
instruction = "Issue #{{ event.issue.number }}"
result = { decision = { enum = ["go", "stop"] } }
push = true
next = [{ if = "result.decision == 'go'", to = "build" }, { to = "bad" }]
[workflows.wf.nodes.build]
type = "command"
task = "build"
args = ["{{ nodes.triage.result.decision }}"]
next = [{ if = "exit_code == 0", to = "good" }, { to = "bad" }]
[workflows.wf.nodes.good]
type = "end"
status = "done"
[workflows.wf.nodes.bad]
type = "end"
status = "failed"
"#;

const ACTION_WF: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "say"
[workflows.wf.nodes.say]
type = "action"
action = "comment"
body = "hi"
next = "good"
[workflows.wf.nodes.good]
type = "end"
status = "done"
"#;

struct Projects;
#[async_trait]
impl ProjectLookup for Projects {
    async fn owner(&self, repo: &str) -> Result<Option<ProjectRef>, String> {
        Ok((repo == "o/r").then(|| ProjectRef {
            name: "p".into(),
            default_branch: "main".into(),
        }))
    }
}

struct Source(&'static str);
#[async_trait]
impl RepoSource for Source {
    async fn snapshot(&self, _: &str, _: &str) -> Result<Snapshot, SourceError> {
        unreachable!("the executor reads pinned files")
    }
    async fn file_at(&self, _: &str, sha: &str, path: &str) -> Result<Option<String>, SourceError> {
        assert_eq!(sha, "abc1234", "files are read at the pinned sha");
        Ok(match path {
            WORKFLOWS_PATH => Some(self.0.to_string()),
            MODES_PATH => Some(MODES.to_string()),
            "AGENTS.md" => Some("be nice".to_string()),
            _ => None,
        })
    }
    async fn clone_token(&self, _: &str, write: bool) -> Result<String, SourceError> {
        Ok(if write { "write-token" } else { "read-token" }.into())
    }
}

struct Tokens(Option<&'static str>);
#[async_trait]
impl ClaudeTokens for Tokens {
    async fn token_for(&self, owner: &str) -> Result<Option<Secret>, String> {
        assert_eq!(owner, "owner");
        Ok(self.0.map(Secret::new))
    }
    async fn owner_of(&self, project: &str) -> Result<Option<String>, String> {
        assert_eq!(project, "p");
        Ok(Some("owner".into()))
    }
}

type Respond = Box<dyn Fn(&StepStart) -> Option<StepFinished> + Send + Sync>;

/// The sandbox side: remembers every step (like the real sandbox, across tracks reconnects).
struct Fake {
    respond: Respond,
    starts: Mutex<Vec<StepStart>>,
    steps: Mutex<HashMap<String, Option<StepFinished>>>,
    servers: Mutex<Vec<Session>>,
    stops: Mutex<Vec<String>>,
    connects: Mutex<usize>,
}

impl Fake {
    fn new(respond: Respond) -> Arc<Self> {
        Arc::new(Self {
            respond,
            starts: Mutex::default(),
            steps: Mutex::default(),
            servers: Mutex::default(),
            stops: Mutex::default(),
            connects: Mutex::new(0),
        })
    }

    fn finish(&self, fin: StepFinished) {
        self.steps
            .lock()
            .unwrap()
            .insert(fin.run_id.clone(), Some(fin.clone()));
        for s in self.servers.lock().unwrap().iter() {
            let _ = s.send_step_finished(&fin);
        }
    }

    fn started(&self) -> Vec<StepStart> {
        self.starts.lock().unwrap().clone()
    }
}

fn ok(run_id: &str) -> StepFinished {
    StepFinished {
        run_id: run_id.into(),
        exit: "success".into(),
        outputs: Default::default(),
        result: None,
        git: vec![],
        usage: None,
        environment: None,
        error: None,
    }
}

struct Dial(Arc<Fake>);

#[async_trait]
impl Sandboxes for Dial {
    async fn connect(&self, _project: &str) -> Result<Arc<dyn Link>, LinkError> {
        let this = &self.0;
        *this.connects.lock().unwrap() += 1;
        let ((client, cev), (server, mut sev)) = session_pair_with(
            ClientKind::Executor,
            ServerOptions {
                features: vec![feature::STEPS.into(), feature::EZ_STEPS.into()],
                ..ServerOptions::default()
            },
        )
        .await;
        assert!(client.has_feature(feature::EZ_STEPS));
        this.servers.lock().unwrap().push(server.clone());
        let fake = this.clone();
        tokio::spawn(async move {
            while let Some(ev) = sev.next().await {
                let Event::Ctrl(c) = ev else { continue };
                match c {
                    Ctrl::StepStart(s) => {
                        let existing = fake.steps.lock().unwrap().contains_key(&s.run_id);
                        let _ = server.accept_step(&s.run_id, StepPhase::Running, existing);
                        if existing {
                            continue;
                        }
                        fake.starts.lock().unwrap().push((*s).clone());
                        fake.steps.lock().unwrap().insert(s.run_id.clone(), None);
                        if let Some(mut f) = (fake.respond)(&s) {
                            f.run_id = s.run_id.clone();
                            fake.finish(f);
                        }
                    }
                    Ctrl::StepStatus { run_id, .. } => {
                        let known = fake.steps.lock().unwrap().get(&run_id).cloned();
                        let (state, result) = match known {
                            None => (StepPhase::Unknown, None),
                            Some(None) => (StepPhase::Running, None),
                            Some(Some(f)) => (StepPhase::Finished, Some(f)),
                        };
                        let _ = server.send_step_state(&StepInfo {
                            run_id,
                            state,
                            result,
                        });
                    }
                    Ctrl::StepStop { run_id } => {
                        fake.stops.lock().unwrap().push(run_id.clone());
                        let mut f = ok(&run_id);
                        f.exit = "interrupted".into();
                        fake.finish(f);
                    }
                    _ => {}
                }
            }
        });
        Ok(Arc::new(SessionLink::new(client, cev)))
    }
}

struct Env {
    _node: Box<dyn std::any::Any>,
    pool: PgPool,
}

async fn db_setup() -> Env {
    let node = Postgres::default().with_tag("18").start().await.unwrap();
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 8).await.unwrap();
    db::migrate(&pool).await.unwrap();
    Env {
        _node: Box::new(node),
        pool,
    }
}

fn executor(
    env: &Env,
    wf: &'static str,
    fake: &Arc<Fake>,
    token: Option<&'static str>,
) -> Arc<Executor> {
    Arc::new(Executor::new(
        env.pool.clone(),
        Arc::new(Source(wf)),
        Arc::new(Projects),
        Arc::new(Dial(fake.clone())),
        Arc::new(Tokens(token)),
        ExecConfig {
            concurrency: 2,
            poll: Duration::from_millis(100),
            lease: Duration::from_secs(1),
            retry_backoff: Duration::from_millis(200),
        },
    ))
}

/// A `pending` run of workflow `wf`, as routing creates it.
async fn pending_run(pool: &PgPool, wf: &str, event_id: i64) -> String {
    let file = ez_workflow::parse(if wf == "action" { ACTION_WF } else { WORKFLOWS }).unwrap();
    let event = WfEvent::new(
        "issues",
        Some("labeled".into()),
        json!({"action": "labeled", "issue": {"number": 7, "title": "T", "user": {"login": "a"}, "html_url": "u"}, "label": {"name": "ez"}}),
    );
    let id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
        .fetch_one(pool)
        .await
        .unwrap();
    let state = RunState::start(
        &file,
        "wf",
        &event,
        RunInfo {
            id: id.clone(),
            attempt: 1,
            test: false,
        },
    )
    .unwrap();
    sqlx::query(
        "INSERT INTO tracks.runs (id, workflow, project, repo, event_id, sha, status, current_node, visits, state)
         VALUES ($1::uuid, 'wf', 'p', 'o/r', $2, 'abc1234', 'pending', $3, $4, $5)",
    )
    .bind(&id)
    .bind(event_id)
    .bind(state.current())
    .bind(serde_json::to_value(&state.visits).unwrap())
    .bind(serde_json::to_value(&state).unwrap())
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn status(pool: &PgPool, id: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT status, reason FROM tracks.runs WHERE id = $1::uuid")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn wait_for(pool: &PgPool, id: &str, want: &str) -> (String, Option<String>) {
    for _ in 0..200 {
        let s = status(pool, id).await;
        if s.0 == want {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "run {id} never reached {want}: {:?}",
        status(pool, id).await
    );
}

fn start_loop(ex: &Arc<Executor>) -> tokio::sync::watch::Sender<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::spawn(ex.clone().run(rx));
    tx
}

fn agent_ok(go: bool) -> StepFinished {
    let mut f = ok("");
    f.result = Some(json!({"decision": if go { "go" } else { "stop" }}));
    f.git = vec![StepGit {
        repo: "o/r".into(),
        branch: "ez/wf/x".into(),
        head_sha: "deadbeef".into(),
        pushed: true,
    }];
    f.usage = Some(enzarb_proto::Usage {
        input_tokens: Some(11),
        output_tokens: Some(22),
        ..Default::default()
    });
    f
}

#[tokio::test]
async fn agent_command_end_path_feeds_results_into_edges() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|s| match &s.exec {
        StepExec::Agent { .. } => Some(agent_ok(true)),
        StepExec::Command { .. } => Some(ok("")),
    }));
    let ex = executor(&env, WORKFLOWS, &fake, Some("tok-123"));
    let id = pending_run(&env.pool, "wf", 1).await;
    let stop = start_loop(&ex);
    let (st, reason) = wait_for(&env.pool, &id, "done").await;
    assert_eq!((st.as_str(), reason), ("done", None));
    let _ = stop.send(true);

    let starts = fake.started();
    assert_eq!(starts.len(), 2);
    let agent = &starts[0];
    assert_eq!(agent.task_id, id);
    assert_eq!(agent.workspaces.len(), 1);
    let ws = &agent.workspaces[0];
    assert_eq!(ws.repo, "o/r");
    assert_eq!(ws.url, "https://github.com/o/r.git");
    assert_eq!(ws.branch, format!("ez/wf/{id}"));
    assert_eq!(ws.base, "main");
    assert!(ws.push);
    assert_eq!(ws.token.expose(), "write-token");
    assert_eq!(
        agent.env.get("CLAUDE_CODE_OAUTH_TOKEN").map(Secret::expose),
        Some("tok-123")
    );
    match &agent.exec {
        StepExec::Agent {
            runtime,
            prompt,
            result_schema,
            mode,
            ez,
            ..
        } => {
            assert_eq!(runtime.command, ["ez", "acp"]);
            assert_eq!(prompt, "Issue #7");
            assert_eq!(mode.as_deref(), Some("triage"));
            assert!(result_schema.as_ref().unwrap()["properties"]["decision"].is_object());
            let ez = ez.as_ref().unwrap();
            assert!(ez.modes_toml.contains("[modes.triage]"));
            assert_eq!(
                ez.instruction_files.get("AGENTS.md").map(String::as_str),
                Some("be nice")
            );
        }
        e => panic!("{e:?}"),
    }
    let cmd = &starts[1];
    assert!(!cmd.workspaces[0].push);
    assert_eq!(cmd.workspaces[0].token.expose(), "read-token");
    assert!(cmd.env.is_empty(), "command steps get no Claude token");
    assert_eq!(cmd.task_id, id, "nodes share the run's checkout");
    assert_ne!(cmd.run_id, agent.run_id);
    match &cmd.exec {
        StepExec::Command { run, .. } => {
            assert!(
                run.starts_with("mise run") && run.contains("build") && run.contains("go"),
                "{run}"
            )
        }
        e => panic!("{e:?}"),
    }

    type NodeRow = (String, String, Option<Value>, Option<Value>, Option<Value>);
    let rows: Vec<NodeRow> = sqlx::query_as(
        "SELECT node, status, outputs, usage, git FROM tracks.node_runs WHERE run_id = $1::uuid ORDER BY id",
    )
    .bind(&id)
    .fetch_all(&env.pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "triage");
    assert_eq!(rows[0].2.as_ref().unwrap()["result"]["decision"], "go");
    assert_eq!(rows[0].2.as_ref().unwrap()["git"]["pushed"], true);
    assert_eq!(rows[0].3.as_ref().unwrap()["output_tokens"], 22);
    assert_eq!(rows[1].2.as_ref().unwrap()["exit_code"], 0);
}

#[tokio::test]
async fn a_result_can_route_to_the_failure_end() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|_| Some(agent_ok(false))));
    let ex = executor(&env, WORKFLOWS, &fake, Some("t"));
    let id = pending_run(&env.pool, "wf", 1).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "failed").await;
    assert_eq!(fake.started().len(), 1, "the command node never ran");
}

#[tokio::test]
async fn a_failing_command_follows_its_edge_with_the_exit_code() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|s| match &s.exec {
        StepExec::Agent { .. } => Some(agent_ok(true)),
        StepExec::Command { .. } => {
            let mut f = ok("");
            f.exit = "failure".into();
            f.error = Some("the command exited with code 3".into());
            Some(f)
        }
    }));
    let ex = executor(&env, WORKFLOWS, &fake, Some("t"));
    let id = pending_run(&env.pool, "wf", 1).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "failed").await;
    let (out, status, err): (Value, String, Option<String>) = sqlx::query_as(
        "SELECT outputs, status, error FROM tracks.node_runs WHERE run_id = $1::uuid AND node = 'build'",
    )
    .bind(&id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(out["exit_code"], 3);
    assert_eq!(status, "failed");
    assert!(err.unwrap().contains("code 3"));
}

#[tokio::test]
async fn a_missing_claude_token_fails_the_run_without_starting_a_step() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|_| Some(agent_ok(true))));
    let ex = executor(&env, WORKFLOWS, &fake, None);
    let id = pending_run(&env.pool, "wf", 1).await;
    let _stop = start_loop(&ex);
    let (_, reason) = wait_for(&env.pool, &id, "failed").await;
    assert_eq!(reason.as_deref(), Some("no_credential"));
    assert!(fake.started().is_empty());
}

#[tokio::test]
async fn action_and_wait_nodes_are_not_supported_yet() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|_| Some(ok(""))));
    let ex = executor(&env, ACTION_WF, &fake, Some("t"));
    let id = pending_run(&env.pool, "action", 1).await;
    let _stop = start_loop(&ex);
    let (_, reason) = wait_for(&env.pool, &id, "failed").await;
    assert_eq!(reason.as_deref(), Some("node type not supported yet"));
}

#[tokio::test]
async fn a_restarted_tracks_re_attaches_and_never_starts_the_step_again() {
    let env = db_setup().await;
    // The agent step hangs: the sandbox keeps running it while tracks restarts.
    let fake = Fake::new(Box::new(|s| match &s.exec {
        StepExec::Agent { .. } => None,
        StepExec::Command { .. } => Some(ok("")),
    }));
    let first = executor(&env, WORKFLOWS, &fake, Some("t"));
    let id = pending_run(&env.pool, "wf", 1).await;
    let stop1 = start_loop(&first);
    for _ in 0..100 {
        if fake.started().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(fake.started().len(), 1);
    // "Crash": the first executor's loop and its drive tasks die with the runtime handle; we
    // can only stop the claim loop here, so also expire its lease by hand to simulate a replica
    // that is gone.
    let _ = stop1.send(true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    sqlx::query("UPDATE tracks.runs SET lease_until = now() - interval '1 second', lease_owner = 'dead' WHERE id = $1::uuid")
        .bind(&id)
        .execute(&env.pool)
        .await
        .unwrap();

    let second = executor(&env, WORKFLOWS, &fake, Some("t"));
    let _stop2 = start_loop(&second);
    // Wait until the second replica has re-attached (it opens its own connection).
    for _ in 0..100 {
        if *fake.connects.lock().unwrap() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        *fake.connects.lock().unwrap() >= 2,
        "re-attached through a new connection"
    );
    let agent_step = fake.started()[0].run_id.clone();
    let mut done = agent_ok(true);
    done.run_id = agent_step;
    fake.finish(done);
    wait_for(&env.pool, &id, "done").await;
    let agent_starts = fake
        .started()
        .iter()
        .filter(|s| matches!(s.exec, StepExec::Agent { .. }))
        .count();
    assert_eq!(agent_starts, 1, "the running step was never started again");
}

#[tokio::test]
async fn stop_cancels_the_run_and_stops_the_step() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|_| None));
    let ex = executor(&env, WORKFLOWS, &fake, Some("t"));
    let id = pending_run(&env.pool, "wf", 1).await;
    let _stop = start_loop(&ex);
    for _ in 0..100 {
        if fake.started().len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut state = api::AppState::new(vec![b'k'; 32]);
    state = state.with_pool(env.pool.clone());
    let app = api::router(state, 1 << 20);
    let call = |method: &str, uri: String| {
        let app = app.clone();
        let method = method.to_string();
        async move {
            let res = app
                .oneshot(
                    Request::builder()
                        .method(method.as_str())
                        .uri(uri)
                        .header(header::AUTHORIZATION, format!("Bearer {}", "k".repeat(32)))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = res.status();
            let body = res.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
            )
        }
    };

    let (s, body) = call("GET", format!("/v1/runs/{id}")).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(body["run"]["workflow"], "wf");
    assert_eq!(body["run"]["node_runs"][0]["node"], "triage");
    assert_eq!(body["run"]["node_runs"][0]["mode"], "triage");
    assert_eq!(
        body["run"]["node_runs"][0]["rendered"]["instruction"],
        "Issue #7"
    );
    let (s, list) = call("GET", "/v1/runs?project=p&repo=O/R".into()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["runs"].as_array().unwrap().len(), 1);
    let (s, _) = call("GET", "/v1/runs".into()).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call("GET", "/v1/runs/not-a-uuid".into()).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, _) = call("POST", format!("/v1/runs/{id}/stop")).await;
    assert_eq!(s, StatusCode::OK);
    let (_, reason) = wait_for(&env.pool, &id, "cancelled").await;
    assert_eq!(reason.as_deref(), Some("stopped"));
    assert_eq!(fake.stops.lock().unwrap().len(), 1);
    let (s, body) = call("POST", format!("/v1/runs/{id}/stop")).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "run_finished");
}

#[tokio::test]
async fn max_runs_keeps_extra_runs_pending() {
    let env = db_setup().await;
    // max_runs = 1 for this workflow.
    const ONE: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "triage"
max_runs = 1
[workflows.wf.nodes.triage]
type = "agent"
mode = "triage"
instruction = "x"
next = "good"
[workflows.wf.nodes.good]
type = "end"
status = "done"
"#;
    let fake = Fake::new(Box::new(|_| None));
    let ex = executor(&env, ONE, &fake, Some("t"));
    let a = pending_run(&env.pool, "wf", 1).await;
    // Same shape as WORKFLOWS' start node, so the stored state is valid for ONE too.
    let b = pending_run(&env.pool, "wf", 2).await;
    let _stop = start_loop(&ex);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let sa = status(&env.pool, &a).await.0;
    let sb = status(&env.pool, &b).await.0;
    let mut got = [sa, sb];
    got.sort();
    assert_eq!(got, ["pending".to_string(), "running".to_string()]);
    assert_eq!(fake.started().len(), 1);
}
