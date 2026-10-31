//! The run executor against a real Postgres (testcontainers) and a fake sandbox that speaks the
//! real step frames over `enzarb_proto::session::memory` pairs: an agent -> command -> end path
//! (with the result feeding an edge), failures, `no_credential`, resume after a tracks restart
//! (the step is not started again), Stop, the run endpoints, and (ADR-0029 step 5) `action`
//! nodes against a fake GitHub HTTP server (comment, labels, pull request, idempotent replay after a
//! simulated crash, test runs) and `wait` nodes resumed or not by routed events, or timed out.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Uri};
use axum::http::{Request, StatusCode, header};
use enzarb_proto::session::memory::session_pair_with;
use enzarb_proto::session::{Event, ServerOptions, Session, StepInfo};
use enzarb_proto::{
    ClientKind, Ctrl, Secret, StepExec, StepFinished, StepGit, StepPhase, StepStart, feature,
};
use enzarb_tracks::actions::{ActionError, Actions, Call, GithubActions};
use enzarb_tracks::exec::{ExecConfig, Executor};
use enzarb_tracks::github::{MODES_PATH, RepoSource, Snapshot, SourceError, WORKFLOWS_PATH};
use enzarb_tracks::projects::{ProjectLookup, ProjectRef};
use enzarb_tracks::router::Router;
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

[modes.planner]
description = "Plan"
tools = ["Read"]
mcp_tools = ["mcp__enzarb__submit_result"]
[modes.implementer]
description = "Implement"
tools = ["Read", "Edit"]
mcp_tools = ["mcp__enzarb__submit_result"]
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

const FLOW: &str = r#"
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
next = [{ if = "git.pushed", to = "say" }, { to = "bad" }]
[workflows.wf.nodes.say]
type = "action"
action = "comment"
body = "Plan {{ nodes.triage.result.decision }} for #{{ subject.number }}"
next = "label"
[workflows.wf.nodes.label]
type = "action"
action = "add_labels"
labels = ["working", "issue-{{ subject.number }}"]
next = "unlabel"
[workflows.wf.nodes.unlabel]
type = "action"
action = "remove_labels"
labels = ["ez"]
next = "pr"
[workflows.wf.nodes.pr]
type = "action"
action = "open_pull_request"
title = "Fix #{{ subject.number }}"
body = "Closes #{{ subject.number }} ({{ nodes.say.number }})"
draft = true
next = "good"
[workflows.wf.nodes.good]
type = "end"
status = "done"
[workflows.wf.nodes.bad]
type = "end"
status = "failed"
"#;

/// Waits for a comment by the subject's author; times out after 1s only in `WAIT_SHORT`.
const WAIT_WF: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "w"
[workflows.wf.nodes.w]
type = "wait"
on = ["issue_comment.created"]
if = "event.comment.user.login == subject.author"
timeout = "7d"
next = [{ if = "timed_out", to = "stale" }, { to = "resumed" }]
[workflows.wf.nodes.resumed]
type = "end"
status = "done"
[workflows.wf.nodes.stale]
type = "end"
status = "cancelled"
"#;

const WAIT_SHORT: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "w"
[workflows.wf.nodes.w]
type = "wait"
on = ["issue_comment.created"]
timeout = "1s"
next = [{ if = "timed_out", to = "stale" }, { to = "resumed" }]
[workflows.wf.nodes.resumed]
type = "end"
status = "done"
[workflows.wf.nodes.stale]
type = "end"
status = "cancelled"
"#;

/// Waits for the pull request of the run branch to close.
const WAIT_PR: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "w"
[workflows.wf.nodes.w]
type = "wait"
on = ["pull_request.closed"]
next = "resumed"
[workflows.wf.nodes.resumed]
type = "end"
status = "done"
"#;

/// A fake GitHub REST API: records every request, keeps comments and pull requests.
#[derive(Default)]
struct GhState {
    reqs: Vec<(String, String, Value, String)>,
    comments: Vec<Value>,
    pulls: Vec<Value>,
}

struct Gh {
    st: Mutex<GhState>,
    /// `ahead_by` of the run branch; `None`: the branch does not exist (404).
    compare: Mutex<Option<u64>>,
    url: String,
}

async fn gh_handler(
    State(g): State<Arc<Gh>>,
    method: Method,
    uri: Uri,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> (StatusCode, axum::Json<Value>) {
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let auth = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let pq = uri
        .path_and_query()
        .map(|p| p.to_string())
        .unwrap_or_default();
    let path = uri.path().to_string();
    let mut st = g.st.lock().unwrap();
    st.reqs.push((method.to_string(), pq, body.clone(), auth));
    let ok = |v: Value| (StatusCode::OK, axum::Json(v));
    if path.contains("/compare/") {
        return match *g.compare.lock().unwrap() {
            Some(n) => ok(json!({"ahead_by": n})),
            None => (
                StatusCode::NOT_FOUND,
                axum::Json(json!({"message": "Not Found"})),
            ),
        };
    }
    if path.ends_with("/comments") {
        if method == Method::GET {
            return ok(json!(st.comments));
        }
        let id = 1000 + st.comments.len() as u64;
        let c = json!({"id": id, "body": body["body"],
            "html_url": format!("https://github.com/o/r/issues/7#issuecomment-{id}")});
        st.comments.push(c.clone());
        return (StatusCode::CREATED, axum::Json(c));
    }
    if path.ends_with("/pulls") {
        if method == Method::GET {
            return ok(json!(st.pulls));
        }
        let pr = json!({"number": 42, "html_url": "https://github.com/o/r/pull/42",
            "head": {"ref": body["head"]}});
        st.pulls.push(pr.clone());
        return (StatusCode::CREATED, axum::Json(pr));
    }
    if path.ends_with("/labels") || path.contains("/labels/") {
        return ok(json!([]));
    }
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({"message": "no route"})),
    )
}

async fn fake_github() -> Arc<Gh> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let g = Arc::new(Gh {
        st: Mutex::default(),
        compare: Mutex::new(Some(2)),
        url: format!("http://{}", listener.local_addr().unwrap()),
    });
    let app = axum::Router::new()
        .fallback(gh_handler)
        .with_state(g.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    g
}

impl Gh {
    fn requests(&self, method: &str, contains: &str) -> Vec<(String, Value, String)> {
        self.st
            .lock()
            .unwrap()
            .reqs
            .iter()
            .filter(|(m, p, _, _)| m == method && p.contains(contains))
            .map(|(_, p, b, a)| (p.clone(), b.clone(), a.clone()))
            .collect()
    }
    fn total(&self) -> usize {
        self.st.lock().unwrap().reqs.len()
    }
}

/// The token source of the actions: records the permissions asked for.
#[derive(Default)]
struct ActSource {
    perms: Mutex<Vec<Vec<(String, String)>>>,
}

#[async_trait]
impl RepoSource for ActSource {
    async fn snapshot(&self, _: &str, _: &str) -> Result<Snapshot, SourceError> {
        unreachable!()
    }
    async fn action_token(
        &self,
        repo: &str,
        permissions: &[(&str, &str)],
    ) -> Result<String, SourceError> {
        assert_eq!(repo, "o/r");
        self.perms.lock().unwrap().push(
            permissions
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        Ok("act-token".into())
    }
}

/// Performs the call, then reports a transient failure for the first call of each node visit:
/// the tracks process "died" after GitHub answered and before the result was written.
struct CrashAfterCall {
    inner: Arc<dyn Actions>,
    seen: Mutex<std::collections::HashSet<String>>,
}

#[async_trait]
impl Actions for CrashAfterCall {
    async fn perform(&self, call: &Call<'_>) -> Result<Value, ActionError> {
        let r = self.inner.perform(call).await;
        if self.seen.lock().unwrap().insert(call.marker.to_string()) {
            return Err(ActionError::Transient("crashed before the write".into()));
        }
        r
    }
}

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
        Ok(Snapshot {
            sha: "abc1234".into(),
            workflows: Some(self.0.to_string()),
            modes: Some(MODES.to_string()),
        })
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
                features: vec![
                    feature::STEPS.into(),
                    feature::EZ_STEPS.into(),
                    feature::STEP_START_REF.into(),
                ],
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
    // The connectors schema as the receiver creates it (only what routing reads).
    for q in [
        "CREATE SCHEMA connectors",
        "CREATE TABLE connectors.events (id bigserial PRIMARY KEY, source text NOT NULL DEFAULT 'github',
           delivery_id text NOT NULL, event text NOT NULL, action text, repo text, sender text,
           payload jsonb NOT NULL, received_at timestamptz NOT NULL DEFAULT now())",
        "CREATE VIEW connectors.event_v AS SELECT id, source, delivery_id, event, action, repo,
           sender, payload, received_at FROM connectors.events",
    ] {
        sqlx::query(q).execute(&pool).await.unwrap();
    }
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
    executor_with(
        env,
        wf,
        fake,
        token,
        Arc::new(enzarb_tracks::actions::NoActions),
    )
}

fn executor_with(
    env: &Env,
    wf: &'static str,
    fake: &Arc<Fake>,
    token: Option<&'static str>,
    actions: Arc<dyn Actions>,
) -> Arc<Executor> {
    Arc::new(
        Executor::new(
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
        )
        .with_actions(actions),
    )
}

/// A `pending` run of workflow `wf`, as routing creates it.
async fn pending_run(pool: &PgPool, wf: &str, event_id: i64) -> String {
    pending_run_with(
        pool,
        if wf == "action" { ACTION_WF } else { WORKFLOWS },
        event_id,
        false,
    )
    .await
}

async fn pending_run_with(pool: &PgPool, text: &str, event_id: i64, test: bool) -> String {
    let file = ez_workflow::parse(text).unwrap();
    let event = WfEvent::new(
        "issues",
        Some("labeled".into()),
        json!({"action": "labeled", "issue": {"number": 7, "state": "open", "title": "T", "body": "B", "user": {"login": "a"}, "html_url": "u"}, "label": {"name": "ez"}}),
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
            test,
        },
    )
    .unwrap();
    sqlx::query(
        "INSERT INTO tracks.runs (id, workflow, project, repo, subject_key, event_id, sha, status, test, current_node, visits, state)
         VALUES ($1::uuid, 'wf', 'p', 'o/r', 'o/r#7', $2, 'abc1234', 'pending', $6, $3, $4, $5)",
    )
    .bind(&id)
    .bind(event_id)
    .bind(state.current())
    .bind(serde_json::to_value(&state.visits).unwrap())
    .bind(serde_json::to_value(&state).unwrap())
    .bind(test)
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

const PR_WF: &str = r#"
version = 1
[workflows.wf]
on = ["pull_request.opened"]
start = "review"
[workflows.wf.nodes.review]
type = "agent"
mode = "triage"
instruction = "Review #{{ subject.number }}"
push = true
next = "done"
[workflows.wf.nodes.done]
type = "end"
status = "done"
"#;

/// Inserts a pending `wf` run of `PR_WF` for a `pull_request.opened` event whose head repo is
/// `head_repo`, base branch `develop`.
async fn pending_pr_run(pool: &PgPool, head_repo: &str) -> String {
    let file = ez_workflow::parse(PR_WF).unwrap();
    let event = WfEvent::new(
        "pull_request",
        Some("opened".into()),
        json!({"action": "opened", "pull_request": {"number": 9, "title": "T", "body": "", "user": {"login": "a"},
            "html_url": "u", "base": {"ref": "develop", "sha": "b".repeat(40)},
            "head": {"ref": "feat", "sha": "c".repeat(40), "repo": {"full_name": head_repo}}}}),
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
        "INSERT INTO tracks.runs (id, workflow, project, repo, subject_key, event_id, sha, status, test, current_node, visits, state)
         VALUES ($1::uuid, 'wf', 'p', 'o/r', 'o/r#9', 1, 'abc1234', 'pending', false, $2, $3, $4)",
    )
    .bind(&id)
    .bind(state.current())
    .bind(serde_json::to_value(&state.visits).unwrap())
    .bind(serde_json::to_value(&state).unwrap())
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A run about a pull request works on the PR's code: the workspace starts at
/// `refs/pull/N/head`, `base` is the PR's base branch, the prompt says so, and a PR from a fork is
/// read-only.
#[tokio::test]
async fn a_pull_request_run_starts_from_the_pr_head_and_forks_are_read_only() {
    for (head_repo, push) in [("o/r", true), ("fork/r", false)] {
        let env = db_setup().await;
        let fake = Fake::new(Box::new(|_| Some(agent_ok(false))));
        let ex = executor(&env, PR_WF, &fake, Some("tok"));
        let id = pending_pr_run(&env.pool, head_repo).await;
        let stop = start_loop(&ex);
        let (st, reason) = wait_for(&env.pool, &id, "done").await;
        assert_eq!((st.as_str(), reason), ("done", None));
        let _ = stop.send(true);
        let starts = fake.started();
        let ws = &starts[0].workspaces[0];
        assert_eq!(ws.start_ref.as_deref(), Some("refs/pull/9/head"));
        assert_eq!(ws.base, "develop");
        assert_eq!(ws.branch, format!("ez/wf/{id}"));
        assert_eq!(ws.push, push, "{head_repo}");
        match &starts[0].exec {
            StepExec::Agent { prompt, .. } => {
                assert!(prompt.starts_with("Review #9"), "{prompt}");
                assert!(
                    prompt.contains("git diff origin/develop...HEAD"),
                    "{prompt}"
                );
                assert_eq!(prompt.contains("read-only"), !push, "{prompt}");
            }
            other => panic!("{other:?}"),
        }
    }
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
    assert_eq!(ws.start_ref, None);
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

/// A failed end without a reason gets a generic one naming the end node and the node before it.
#[tokio::test]
async fn a_failed_end_without_a_reason_records_a_generic_one() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|_| Some(agent_ok(false))));
    let ex = executor(&env, WORKFLOWS, &fake, Some("t"));
    let id = pending_run(&env.pool, "wf", 1).await;
    let _stop = start_loop(&ex);
    let (_, reason) = wait_for(&env.pool, &id, "failed").await;
    assert_eq!(
        reason.as_deref(),
        Some("ended at `bad` (failed) after `triage`")
    );
}

const END_REASON_WF: &str = r#"
version = 1
[workflows.wf]
on = ["issues.labeled"]
start = "triage"
[workflows.wf.nodes.triage]
type = "agent"
mode = "triage"
instruction = "Issue #{{ event.issue.number }}"
result = { decision = { enum = ["go", "stop"] } }
next = "bad"
[workflows.wf.nodes.bad]
type = "end"
status = "failed"
reason = "nothing to do: {{ nodes.triage.result.decision }}"
"#;

#[tokio::test]
async fn a_custom_end_reason_is_rendered_into_the_run() {
    let env = db_setup().await;
    let fake = Fake::new(Box::new(|_| Some(agent_ok(false))));
    let ex = executor(&env, END_REASON_WF, &fake, Some("t"));
    let id = pending_run_with(&env.pool, END_REASON_WF, 1, false).await;
    let _stop = start_loop(&ex);
    let (_, reason) = wait_for(&env.pool, &id, "failed").await;
    assert_eq!(reason.as_deref(), Some("nothing to do: stop"));
}

const WR_WF: &str = r#"
version = 1
[workflows.wf]
on = ["workflow_run.completed"]
start = "fix"
[workflows.wf.nodes.fix]
type = "agent"
mode = "triage"
instruction = "Fix"
push = true
next = "done"
[workflows.wf.nodes.done]
type = "end"
status = "done"
"#;

/// A `workflow_run` run starts from the failing branch when it lives in this repository; a fork's
/// does not.
#[tokio::test]
async fn a_workflow_run_starts_from_the_failing_branch_unless_it_is_a_fork() {
    for (head_repo, want) in [("o/r", Some("refs/heads/feat/x")), ("fork/r", None)] {
        let env = db_setup().await;
        let fake = Fake::new(Box::new(|_| Some(agent_ok(false))));
        let ex = executor(&env, WR_WF, &fake, Some("tok"));
        let file = ez_workflow::parse(WR_WF).unwrap();
        let event = WfEvent::new(
            "workflow_run",
            Some("completed".into()),
            json!({"action": "completed", "workflow_run": {"name": "CI", "conclusion": "failure",
                "head_branch": "feat/x", "head_sha": "c".repeat(40), "html_url": "u",
                "head_repository": {"full_name": head_repo}}}),
        );
        let id: String = sqlx::query_scalar("SELECT gen_random_uuid()::text")
            .fetch_one(&env.pool)
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
            "INSERT INTO tracks.runs (id, workflow, project, repo, event_id, sha, status, test, current_node, visits, state)
             VALUES ($1::uuid, 'wf', 'p', 'o/r', 1, 'abc1234', 'pending', false, $2, $3, $4)",
        )
        .bind(&id)
        .bind(state.current())
        .bind(serde_json::to_value(&state.visits).unwrap())
        .bind(serde_json::to_value(&state).unwrap())
        .execute(&env.pool)
        .await
        .unwrap();
        let stop = start_loop(&ex);
        wait_for(&env.pool, &id, "done").await;
        let _ = stop.send(true);
        let starts = fake.started();
        let ws = &starts[0].workspaces[0];
        assert_eq!(ws.start_ref.as_deref(), want, "{head_repo}");
        assert_eq!(ws.base, "main");
        assert_eq!(ws.branch, format!("ez/wf/{id}"));
    }
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

async fn github_actions(gh: &Arc<Gh>) -> (Arc<ActSource>, Arc<dyn Actions>) {
    let src = Arc::new(ActSource::default());
    let actions: Arc<dyn Actions> = Arc::new(GithubActions::new(src.clone(), &gh.url).unwrap());
    (src, actions)
}

async fn node_outputs(pool: &PgPool, id: &str, node: &str) -> Value {
    sqlx::query_scalar("SELECT outputs FROM tracks.node_runs WHERE run_id = $1::uuid AND node = $2")
        .bind(id)
        .bind(node)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn agent_fake() -> Arc<Fake> {
    Fake::new(Box::new(|_| Some(agent_ok(true))))
}

#[tokio::test]
async fn agent_then_comment_labels_and_pull_request() {
    let env = db_setup().await;
    let gh = fake_github().await;
    let (src, actions) = github_actions(&gh).await;
    let ex = executor_with(&env, FLOW, &agent_fake(), Some("t"), actions);
    let id = pending_run_with(&env.pool, FLOW, 100, false).await;
    let _stop = start_loop(&ex);
    let (st, reason) = wait_for(&env.pool, &id, "done").await;
    assert_eq!((st.as_str(), reason), ("done", None));

    // comment: hidden marker, subject number from the run
    let posts = gh.requests("POST", "/issues/7/comments");
    assert_eq!(posts.len(), 1);
    let body = posts[0].1["body"].as_str().unwrap();
    assert!(body.starts_with("Plan go for #7"), "{body}");
    assert!(
        body.contains(&format!("<!-- ez-tracks:{id}:say:1 -->")),
        "{body}"
    );
    assert_eq!(posts[0].2, "Bearer act-token");
    let say = node_outputs(&env.pool, &id, "say").await;
    assert_eq!(say["number"], 1000);
    assert!(say["url"].as_str().unwrap().contains("issuecomment-1000"));

    // labels
    let add = gh.requests("POST", "/issues/7/labels");
    assert_eq!(add[0].1, json!({"labels": ["working", "issue-7"]}));
    assert_eq!(gh.requests("DELETE", "/issues/7/labels/ez").len(), 1);

    // pull request: run branch against the default branch, outputs feed the next template
    let prs = gh.requests("POST", "/repos/o/r/pulls");
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].1["head"], format!("ez/wf/{id}"));
    assert_eq!(prs[0].1["base"], "main");
    assert_eq!(prs[0].1["title"], "Fix #7");
    assert_eq!(prs[0].1["draft"], true);
    assert_eq!(prs[0].1["body"], "Closes #7 (1000)");
    let pr = node_outputs(&env.pool, &id, "pr").await;
    assert_eq!(pr["number"], 42);
    assert_eq!(pr["url"], "https://github.com/o/r/pull/42");

    // each action asked for only what it needs
    let perms = src.perms.lock().unwrap().clone();
    let p = |k: &str, v: &str| (k.to_string(), v.to_string());
    assert_eq!(
        perms,
        vec![
            vec![p("issues", "write")],
            vec![p("issues", "write")],
            vec![p("issues", "write")],
            vec![p("pull_requests", "write"), p("contents", "read")],
        ]
    );
}

#[tokio::test]
async fn open_pull_request_fails_without_pushed_commits() {
    for (ahead, want) in [(None, "has not been pushed"), (Some(0), "no commits")] {
        let env = db_setup().await;
        let gh = fake_github().await;
        *gh.compare.lock().unwrap() = ahead;
        let (_, actions) = github_actions(&gh).await;
        let ex = executor_with(&env, FLOW, &agent_fake(), Some("t"), actions);
        let id = pending_run_with(&env.pool, FLOW, 100, false).await;
        let _stop = start_loop(&ex);
        let (_, reason) = wait_for(&env.pool, &id, "failed").await;
        assert!(reason.as_deref().unwrap().contains(want), "{reason:?}");
        assert!(gh.requests("POST", "/repos/o/r/pulls").is_empty());
        let (status, err): (String, Option<String>) = sqlx::query_as(
            "SELECT status, error FROM tracks.node_runs WHERE run_id = $1::uuid AND node = 'pr'",
        )
        .bind(&id)
        .fetch_one(&env.pool)
        .await
        .unwrap();
        assert_eq!(status, "failed");
        assert!(err.unwrap().contains(want));
    }
}

#[tokio::test]
async fn a_crash_between_the_github_call_and_the_write_does_not_repeat_the_action() {
    let env = db_setup().await;
    let gh = fake_github().await;
    let (_, actions) = github_actions(&gh).await;
    let crashing: Arc<dyn Actions> = Arc::new(CrashAfterCall {
        inner: actions,
        seen: Mutex::default(),
    });
    let ex = executor_with(&env, FLOW, &agent_fake(), Some("t"), crashing);
    let id = pending_run_with(&env.pool, FLOW, 100, false).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "done").await;

    // every action ran once at GitHub although every node was attempted twice
    assert_eq!(gh.requests("POST", "/issues/7/comments").len(), 1);
    assert_eq!(gh.requests("POST", "/repos/o/r/pulls").len(), 1);
    assert_eq!(gh.st.lock().unwrap().comments.len(), 1);
    assert_eq!(gh.st.lock().unwrap().pulls.len(), 1);
    // the replay found the comment by its marker (it listed comments) and reused its id
    assert!(!gh.requests("GET", "/issues/7/comments").is_empty());
    assert_eq!(node_outputs(&env.pool, &id, "say").await["number"], 1000);
    assert_eq!(node_outputs(&env.pool, &id, "pr").await["number"], 42);
    // one node_runs row per node visit
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks.node_runs WHERE run_id = $1::uuid")
            .bind(&id)
            .fetch_one(&env.pool)
            .await
            .unwrap();
    assert_eq!(n, 5);
}

#[tokio::test]
async fn test_runs_record_the_call_and_never_reach_github() {
    let env = db_setup().await;
    let gh = fake_github().await;
    let (src, actions) = github_actions(&gh).await;
    let fake = agent_fake();
    let ex = executor_with(&env, FLOW, &fake, Some("t"), actions);
    let id = pending_run_with(&env.pool, FLOW, 100, true).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "done").await;
    assert_eq!(gh.total(), 0, "no request reached GitHub");
    assert!(src.perms.lock().unwrap().is_empty(), "no token was minted");
    assert!(!fake.started()[0].workspaces[0].push, "nothing is pushed");
    let say = node_outputs(&env.pool, &id, "say").await;
    assert_eq!(say["test"], true);
    assert!(say["number"].as_u64().unwrap() > 0);
    assert!(say["url"].as_str().unwrap().contains("test-run"));
    assert!(
        say["call"]["body"]
            .as_str()
            .unwrap()
            .starts_with("Plan go for #7")
    );
    assert_eq!(say["call"]["action"], "comment");
    let pr = node_outputs(&env.pool, &id, "pr").await;
    assert_eq!(pr["call"]["branch"], format!("ez-test/{id}"));
    assert_eq!(pr["call"]["draft"], true);
}

async fn event(pool: &PgPool, name: &str, action: &str, payload: Value) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO connectors.events (delivery_id, event, action, repo, payload)
         VALUES (gen_random_uuid()::text, $1, $2, 'o/r', $3) RETURNING id",
    )
    .bind(name)
    .bind(action)
    .bind(payload)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn comment_event(issue: u64, who: &str) -> Value {
    json!({"action": "created", "issue": {"number": issue, "title": "T", "user": {"login": "a"}, "html_url": "u"},
        "comment": {"user": {"login": who}}})
}

fn router_for(env: &Env, wf: &'static str) -> Arc<Router> {
    Arc::new(Router::new(
        env.pool.clone(),
        Arc::new(Source(wf)),
        Arc::new(Projects),
        2,
        Duration::from_secs(1),
    ))
}

async fn decision_of(pool: &PgPool, id: i64) -> (String, Value) {
    sqlx::query_as("SELECT outcome, resumed FROM tracks.event_decisions WHERE event_id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_wait_is_resumed_only_by_a_matching_event_about_its_subject() {
    let env = db_setup().await;
    let ex = executor(&env, WAIT_WF, &agent_fake(), Some("t"));
    let router = router_for(&env, WAIT_WF);
    let id = pending_run_with(&env.pool, WAIT_WF, 100, false).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "waiting").await;

    // The parked node records what it waits for and until when.
    let (wait, st): (Value, String) = sqlx::query_as(
        "SELECT wait, status FROM tracks.node_runs WHERE run_id = $1::uuid AND node = 'w'",
    )
    .bind(&id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(st, "waiting");
    assert_eq!(wait["on"], json!(["issue_comment.created"]));
    assert!(wait["if"].as_str().unwrap().contains("subject.author"));
    assert!(wait["until"].is_string());

    // A comment by someone else (the `if` fails), a comment on another issue, another event.
    let other_user = event(&env.pool, "issue_comment", "created", comment_event(7, "b")).await;
    let other_issue = event(&env.pool, "issue_comment", "created", comment_event(8, "a")).await;
    let other_event = event(&env.pool, "issues", "edited", comment_event(7, "a")).await;
    for e in [other_user, other_issue, other_event] {
        router.process(e).await;
        let (outcome, resumed) = decision_of(&env.pool, e).await;
        assert_eq!(outcome, "no_match");
        assert_eq!(resumed, json!([]));
    }
    assert_eq!(status(&env.pool, &id).await.0, "waiting");

    // The matching event resumes it, once, even when two replicas route the same event.
    let hit = event(&env.pool, "issue_comment", "created", comment_event(7, "a")).await;
    tokio::join!(router.process(hit), router.process(hit));
    let (outcome, resumed) = decision_of(&env.pool, hit).await;
    assert_eq!(outcome, "resumed");
    assert_eq!(resumed[0]["run"], id);
    assert_eq!(resumed[0]["node"], "w");
    let (st, reason) = wait_for(&env.pool, &id, "done").await;
    assert_eq!((st.as_str(), reason), ("done", None));
    let out = node_outputs(&env.pool, &id, "w").await;
    assert_eq!(out["timed_out"], false);
    assert_eq!(out["event"]["comment"]["user"]["login"], "a");
    let w: Value = sqlx::query_scalar(
        "SELECT wait FROM tracks.node_runs WHERE run_id = $1::uuid AND node = 'w'",
    )
    .bind(&id)
    .fetch_one(&env.pool)
    .await
    .unwrap();
    assert_eq!(w["resumed_by_event"], hit);

    // A later event finds no waiting run.
    let late = event(&env.pool, "issue_comment", "created", comment_event(7, "a")).await;
    router.process(late).await;
    assert_eq!(decision_of(&env.pool, late).await.0, "no_match");
}

#[tokio::test]
async fn a_pull_request_of_the_run_branch_resumes_the_wait() {
    let env = db_setup().await;
    let ex = executor(&env, WAIT_PR, &agent_fake(), Some("t"));
    let router = router_for(&env, WAIT_PR);
    let id = pending_run_with(&env.pool, WAIT_PR, 100, false).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "waiting").await;
    let pr = |number: u64, head: String| {
        json!({"action": "closed", "pull_request": {"number": number, "title": "T",
            "user": {"login": "bot"}, "html_url": "u", "head": {"ref": head}}})
    };
    let unrelated = event(
        &env.pool,
        "pull_request",
        "closed",
        pr(99, "feature/x".into()),
    )
    .await;
    router.process(unrelated).await;
    assert_eq!(decision_of(&env.pool, unrelated).await.0, "no_match");
    assert_eq!(status(&env.pool, &id).await.0, "waiting");
    let linked = event(
        &env.pool,
        "pull_request",
        "closed",
        pr(99, format!("ez/wf/{id}")),
    )
    .await;
    router.process(linked).await;
    assert_eq!(decision_of(&env.pool, linked).await.0, "resumed");
    wait_for(&env.pool, &id, "done").await;
}

#[tokio::test]
async fn a_wait_takes_its_timeout_edge() {
    let env = db_setup().await;
    let ex = executor(&env, WAIT_SHORT, &agent_fake(), Some("t"));
    let id = pending_run_with(&env.pool, WAIT_SHORT, 100, false).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "waiting").await;
    wait_for(&env.pool, &id, "cancelled").await;
    let out = node_outputs(&env.pool, &id, "w").await;
    assert_eq!(out["timed_out"], true);
    assert_eq!(out["event"], Value::Null);
}

#[tokio::test]
async fn a_stopped_waiting_run_is_cancelled() {
    let env = db_setup().await;
    let ex = executor(&env, WAIT_WF, &agent_fake(), Some("t"));
    let id = pending_run_with(&env.pool, WAIT_WF, 100, false).await;
    let _stop = start_loop(&ex);
    wait_for(&env.pool, &id, "waiting").await;
    sqlx::query("UPDATE tracks.runs SET stop_requested = true WHERE id = $1::uuid")
        .bind(&id)
        .execute(&env.pool)
        .await
        .unwrap();
    let (_, reason) = wait_for(&env.pool, &id, "cancelled").await;
    assert_eq!(reason.as_deref(), Some("stopped"));
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

// ----- plan approval gate (the `implement-labeled-issues` template) -----

/// The template's workflow as workflow `wf`, with `vars.approve_plan` and the approval timeout set.
fn plan_wf(approve: bool, timeout: &str) -> &'static str {
    let templates: Vec<Value> = serde_json::from_str(include_str!(
        "../../ui/src/lib/rule-templates/templates.json"
    ))
    .unwrap();
    let mut wf = templates
        .iter()
        .find(|t| t["id"] == "implement-labeled-issues")
        .unwrap()["workflow"]
        .clone();
    wf["vars"]["approve_plan"] = json!(approve);
    wf["nodes"]["approve_plan"]["timeout"] = json!(timeout);
    let file: ez_workflow::WorkflowsFile =
        serde_json::from_value(json!({"version": 1, "workflows": {"wf": wf}})).unwrap();
    Box::leak(ez_workflow::to_toml(&file).unwrap().into_boxed_str())
}

/// planner: `result.plan` is "PLAN n" (n counts plan steps); implementer: pushes.
fn plan_fake() -> Arc<Fake> {
    let n = Mutex::new(0u32);
    Fake::new(Box::new(move |s| {
        let StepExec::Agent { mode, .. } = &s.exec else {
            return None;
        };
        let mut f = ok("");
        if mode.as_deref() == Some("planner") {
            *n.lock().unwrap() += 1;
            f.result = Some(json!({"plan": format!("PLAN {}", n.lock().unwrap())}));
        } else {
            f.git = vec![StepGit {
                repo: "o/r".into(),
                branch: "ez/wf/x".into(),
                head_sha: "deadbeef".into(),
                pushed: true,
            }];
        }
        Some(f)
    }))
}

/// `(mode, prompt)` of every agent step started, in order.
fn agent_prompts(fake: &Fake) -> Vec<(String, String)> {
    fake.started()
        .iter()
        .filter_map(|s| match &s.exec {
            StepExec::Agent { mode, prompt, .. } => {
                Some((mode.clone().unwrap_or_default(), prompt.clone()))
            }
            _ => None,
        })
        .collect()
}

struct PlanEnv {
    env: Env,
    gh: Arc<Gh>,
    fake: Arc<Fake>,
    app: axum::Router,
    router: Arc<Router>,
    id: String,
    _stop: tokio::sync::watch::Sender<bool>,
}

async fn plan_env(approve: bool, timeout: &str) -> PlanEnv {
    let env = db_setup().await;
    let gh = fake_github().await;
    let (_, actions) = github_actions(&gh).await;
    let wf = plan_wf(approve, timeout);
    let fake = plan_fake();
    let ex = executor_with(&env, wf, &fake, Some("t"), actions);
    let router = router_for(&env, wf);
    let id = pending_run_with(&env.pool, wf, 100, false).await;
    let stop = start_loop(&ex);
    let state = api::AppState::new(vec![b'k'; 32])
        .with_pool(env.pool.clone())
        .with_routing(Arc::new(Source(wf)), Arc::new(Projects));
    PlanEnv {
        app: api::router(state, 1 << 20),
        env,
        gh,
        fake,
        router,
        id,
        _stop: stop,
    }
}

impl PlanEnv {
    async fn resume(&self, id: &str, body: Value) -> (StatusCode, Value) {
        let res = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/runs/{id}/resume"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", "k".repeat(32)))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn comment(&self, body: &str, who: &str, kind: &str, assoc: &str) -> i64 {
        let e = event(
            &self.env.pool,
            "issue_comment",
            "created",
            json!({"action": "created",
                "issue": {"number": 7, "state": "open", "title": "T", "body": "B", "user": {"login": "a"}, "html_url": "u"},
                "comment": {"body": body, "user": {"login": who, "type": kind}, "author_association": assoc}}),
        )
        .await;
        self.router.process(e).await;
        e
    }
}

#[tokio::test]
async fn the_plan_is_posted_and_approval_through_the_resume_api_starts_implementation() {
    let p = plan_env(true, "14d").await;
    wait_for(&p.env.pool, &p.id, "waiting").await;
    // The plan is a run output and a comment on the issue that says how to approve.
    let plan = node_outputs(&p.env.pool, &p.id, "plan").await;
    assert_eq!(plan["result"]["plan"], "PLAN 1");
    let posts = p.gh.requests("POST", "/issues/7/comments");
    assert_eq!(posts.len(), 1);
    let body = posts[0].1["body"].as_str().unwrap();
    assert!(
        body.contains("PLAN 1") && body.contains("@enzarb approve"),
        "{body}"
    );
    assert!(body.contains("@enzarb revise"), "{body}");
    assert_eq!(agent_prompts(&p.fake).len(), 1);

    let (s, v) = p
        .resume(&p.id, json!({"actor": "carol", "action": "approve"}))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["resumed"]["node"], "approve_plan");
    let (st, reason) = wait_for(&p.env.pool, &p.id, "done").await;
    assert_eq!((st.as_str(), reason), ("done", None));
    let prompts = agent_prompts(&p.fake);
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1].0, "implementer");
    assert!(prompts[1].1.contains("Implement issue #7") && prompts[1].1.contains("PLAN 1"));
    // Who approved is recorded on the wait, and the next node saw the event.
    let out = node_outputs(&p.env.pool, &p.id, "approve_plan").await;
    assert_eq!(out["event"]["action"], "approve");
    assert_eq!(out["event"]["name"], "manual");
    assert_eq!(out["event"]["sender"]["login"], "carol");
    let w: Value = sqlx::query_scalar(
        "SELECT wait FROM tracks.node_runs WHERE run_id = $1::uuid AND node = 'approve_plan'",
    )
    .bind(&p.id)
    .fetch_one(&p.env.pool)
    .await
    .unwrap();
    assert_eq!(
        (w["resumed_by"].as_str(), w["action"].as_str()),
        (Some("carol"), Some("approve"))
    );

    // The run is no longer waiting.
    let (s, v) = p
        .resume(&p.id, json!({"actor": "carol", "action": "approve"}))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert_eq!(v["error"]["code"], "not_waiting");
    let (s, _) = p
        .resume(
            "00000000-0000-0000-0000-000000000000",
            json!({"actor": "c", "action": "approve"}),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn revise_loops_back_to_plan_with_the_feedback() {
    let p = plan_env(true, "14d").await;
    wait_for(&p.env.pool, &p.id, "waiting").await;
    let (s, _) = p
        .resume(
            &p.id,
            json!({"actor": "carol", "action": "revise", "text": "use fewer files"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Parks again, after planning a second time and posting the new plan.
    for _ in 0..200 {
        if p.gh.requests("POST", "/issues/7/comments").len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    wait_for(&p.env.pool, &p.id, "waiting").await;
    let prompts = agent_prompts(&p.fake);
    assert_eq!(prompts.len(), 2);
    assert!(
        !prompts[0].1.contains("Revise the plan"),
        "{}",
        prompts[0].1
    );
    assert_eq!(prompts[1].0, "planner");
    assert!(prompts[1].1.contains("PLAN 1"), "{}", prompts[1].1);
    assert!(
        prompts[1]
            .1
            .contains("Revise the plan using this feedback:")
            && prompts[1].1.contains("use fewer files"),
        "{}",
        prompts[1].1
    );
    assert!(prompts[1].1.contains("Issue #7: T"), "{}", prompts[1].1);
    let posts = p.gh.requests("POST", "/issues/7/comments");
    assert!(posts[1].1["body"].as_str().unwrap().contains("PLAN 2"));

    let (s, _) = p
        .resume(&p.id, json!({"actor": "carol", "action": "approve"}))
        .await;
    assert_eq!(s, StatusCode::OK);
    wait_for(&p.env.pool, &p.id, "done").await;
    let impl_prompt = &agent_prompts(&p.fake)[2];
    assert_eq!(impl_prompt.0, "implementer");
    assert!(impl_prompt.1.contains("PLAN 2"));
}

#[tokio::test]
async fn a_writers_comment_approves_but_bots_and_outsiders_do_not() {
    let p = plan_env(true, "14d").await;
    wait_for(&p.env.pool, &p.id, "waiting").await;
    for (body, who, kind, assoc) in [
        ("@enzarb approve", "mallory", "User", "NONE"),
        ("@enzarb approve", "enzarb[bot]", "Bot", "MEMBER"),
        ("@enzarb approve", "mallory", "User", "CONTRIBUTOR"),
        ("looks good", "carol", "User", "OWNER"),
    ] {
        let e = p.comment(body, who, kind, assoc).await;
        let errs: Value =
            sqlx::query_scalar("SELECT errors FROM tracks.event_decisions WHERE event_id = $1")
                .bind(e)
                .fetch_one(&p.env.pool)
                .await
                .unwrap();
        assert_eq!(
            decision_of(&p.env.pool, e).await.0,
            "no_match",
            "{who} {errs}"
        );
    }
    assert_eq!(status(&p.env.pool, &p.id).await.0, "waiting");
    // A comment on another issue never resumes it.
    let other = event(
        &p.env.pool,
        "issue_comment",
        "created",
        json!({"action": "created", "issue": {"number": 8, "state": "open", "title": "T", "body": "B", "user": {"login": "a"}, "html_url": "u"},
            "comment": {"body": "@enzarb approve", "user": {"login": "carol", "type": "User"}, "author_association": "OWNER"}}),
    )
    .await;
    p.router.process(other).await;
    assert_eq!(decision_of(&p.env.pool, other).await.0, "no_match");

    let e = p
        .comment("@enzarb approve", "carol", "User", "COLLABORATOR")
        .await;
    assert_eq!(decision_of(&p.env.pool, e).await.0, "resumed");
    wait_for(&p.env.pool, &p.id, "done").await;
    let prompts = agent_prompts(&p.fake);
    assert_eq!(prompts[1].0, "implementer");
    assert!(prompts[1].1.contains("Implement issue #7"));
}

#[tokio::test]
async fn a_comment_revise_loops_back_with_the_comment_as_feedback() {
    let p = plan_env(true, "14d").await;
    wait_for(&p.env.pool, &p.id, "waiting").await;
    let e = p
        .comment(
            "@enzarb revise split the migration",
            "carol",
            "User",
            "OWNER",
        )
        .await;
    assert_eq!(decision_of(&p.env.pool, e).await.0, "resumed");
    for _ in 0..200 {
        if agent_prompts(&p.fake).len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let prompts = agent_prompts(&p.fake);
    assert_eq!(prompts[1].0, "planner");
    assert!(
        prompts[1].1.contains("split the migration"),
        "{}",
        prompts[1].1
    );
}

#[tokio::test]
async fn the_wait_decides_which_actions_resume_it() {
    let p = plan_env(true, "14d").await;
    wait_for(&p.env.pool, &p.id, "waiting").await;
    let (s, v) = p
        .resume(&p.id, json!({"actor": "carol", "action": "dance"}))
        .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["error"]["code"], "not_accepted");
    assert_eq!(status(&p.env.pool, &p.id).await.0, "waiting");
    let (s, _) = p
        .resume(&p.id, json!({"actor": "", "action": "approve"}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_approve_plan_implementation_starts_right_after_the_plan() {
    let p = plan_env(false, "14d").await;
    let (st, reason) = wait_for(&p.env.pool, &p.id, "done").await;
    assert_eq!((st.as_str(), reason), ("done", None));
    let prompts = agent_prompts(&p.fake);
    assert_eq!(
        prompts.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        ["planner", "implementer"]
    );
    // No plan comment, no wait.
    assert!(p.gh.requests("POST", "/issues/7/comments").is_empty());
    let (s, v) = p
        .resume(&p.id, json!({"actor": "carol", "action": "approve"}))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("not_waiting"))
    );
}

#[tokio::test]
async fn an_unapproved_plan_times_out() {
    let p = plan_env(true, "1s").await;
    let (_, reason) = wait_for(&p.env.pool, &p.id, "cancelled").await;
    assert_eq!(reason.as_deref(), Some("plan not approved in time"));
    assert_eq!(agent_prompts(&p.fake).len(), 1);
    let out = node_outputs(&p.env.pool, &p.id, "approve_plan").await;
    assert_eq!(out["timed_out"], true);
}
