//! End-to-end engine tests on Postgres with the fake connector and the fake step runner: the
//! real reducer, interpreter, executor and ticker, driven by inbox events.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use engine::secrets::SecretsKey;
use engine::testing::TestDb;
use engine::{
    Engine, EngineConfig, FakeStepRunner, Library, ProjectInfo, StaticPermissions,
    StaticProjectIndex, StaticSandboxResolver,
};
use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{MergeReadiness, Permission, RepoKey, WorkItemKey};
use enzarb_proto::{StepExec, StepFinished, StepGit};
use serde_json::{Value, json};
use tokio::sync::watch;

const LIB: &str = "enzarb/workflows";
const LIB_SHA: &str = "1111111111111111111111111111111111111111";
const APP: &str = "acme/app";
const APP_SHA: &str = "2222222222222222222222222222222222222222";
const HEAD: &str = "3333333333333333333333333333333333333333";

fn rk(r: &str) -> RepoKey {
    let (o, n) = r.split_once('/').unwrap();
    RepoKey::new("github", o, n)
}

struct H {
    db: TestDb,
    engine: Engine,
    gh: Arc<FakeConnector>,
    runner: FakeStepRunner,
    perms: Arc<StaticPermissions>,
    stop: watch::Sender<bool>,
}

fn load_library(gh: &FakeConnector) {
    load_library_at(gh, LIB_SHA, &[]);
    gh.set_ref(rk(LIB), "v1", LIB_SHA);
    gh.set_ref(rk(APP), "main", APP_SHA);
}

/// The test library at `sha`, with `overrides` (path, text) replacing files.
fn load_library_at(gh: &FakeConnector, sha: &str, overrides: &[(&str, &str)]) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lib");
    fn walk(
        gh: &FakeConnector,
        root: &std::path::Path,
        dir: &std::path::Path,
        sha: &str,
        overrides: &[(&str, &str)],
    ) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(gh, root, &p, sha, overrides);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
                let text = match overrides.iter().find(|(o, _)| *o == rel) {
                    Some((_, t)) => t.to_string(),
                    None => std::fs::read_to_string(&p).unwrap(),
                };
                gh.set_file(rk(LIB), sha, &rel, &text);
            }
        }
    }
    walk(gh, &root, &root, sha, overrides);
}

async fn start(stall: Duration, background: bool) -> Option<H> {
    let db = TestDb::start().await?;
    let gh = Arc::new(FakeConnector::new());
    load_library(&gh);
    let index = StaticProjectIndex::shared();
    index.insert(ProjectInfo {
        name: "p".into(),
        owner: "alice".into(),
        repos: vec![(APP.into(), "main".into())],
        ..Default::default()
    });
    let perms = Arc::new(StaticPermissions::new(Permission::Read));
    perms.set("alice", Permission::Write);
    let runner = FakeStepRunner::new();
    let key = SecretsKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([9u8; 32]))
        .unwrap();
    let engine = Engine {
        pool: db.pool.clone(),
        index,
        library: Library::new(gh.clone(), &format!("{LIB}@v1"), vec![])
            .with_ref_ttl(Duration::ZERO),
        permissions: perms.clone(),
        runner: Arc::new(runner.clone()),
        sandboxes: Arc::new(StaticSandboxResolver::new()),
        secrets_key: Some(key),
        claude: None,
        findings: engine::Findings::none(),
        cfg: EngineConfig {
            stall,
            poll: Duration::from_millis(50),
            backoff_base: Duration::from_millis(100),
            ..EngineConfig::default()
        },
    };
    let (stop, rx) = watch::channel(false);
    if background {
        let (e1, r1) = (engine.clone(), rx.clone());
        tokio::spawn(async move { e1.run_reducer(r1).await });
        let (e2, r2) = (engine.clone(), rx.clone());
        tokio::spawn(async move { e2.run_executor(r2).await });
    }
    Some(H {
        db,
        engine,
        gh,
        runner,
        perms,
        stop,
    })
}

impl Drop for H {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

impl H {
    async fn ingest(
        &self,
        dedupe: &str,
        subject: Option<String>,
        ty: &str,
        payload: Value,
    ) -> Option<i64> {
        sqlx::query_scalar("SELECT engine_api.ingest($1, NULL, $2, $3, $4)")
            .bind(dedupe)
            .bind(subject)
            .bind(ty)
            .bind(payload)
            .fetch_one(&self.db.pool)
            .await
            .unwrap()
    }

    async fn open_issue(&self, n: u64, title: &str) {
        self.ingest(
            &format!("github:issue-{n}"),
            Some(format!("github:{APP}#{n}")),
            "issue",
            json!({"type": "issue", "action": "opened", "number": n, "title": title, "body": "b",
                   "id": format!("d{n}"), "repo": APP, "actor": {"login": "alice"},
                   "subject": {"kind": "issue", "number": n}}),
        )
        .await;
    }

    async fn submit(&self, task: i64, kind: &str, data: Value) {
        sqlx::query("SELECT engine_api.submit($1, $2, $3, $4)")
            .bind(task)
            .bind(json!({"login": "alice"}))
            .bind(kind)
            .bind(data)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn projection(&self, task: i64) -> Value {
        sqlx::query_scalar("SELECT to_jsonb(v) FROM engine_api.task_v v WHERE task_id = $1")
            .bind(task)
            .fetch_optional(&self.db.pool)
            .await
            .unwrap()
            .unwrap_or(Value::Null)
    }

    async fn notes(&self) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT type || ': ' || COALESCE(note, '-') FROM engine.events ORDER BY id",
        )
        .fetch_all(&self.db.pool)
        .await
        .unwrap()
    }

    /// Polls `f` until it returns `Some`, up to 10 s.
    async fn until<T, F, Fut>(&self, what: &str, f: F) -> T
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Option<T>>,
    {
        for _ in 0..200 {
            if let Some(v) = f().await {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let jobs: Vec<String> = sqlx::query_scalar("SELECT kind || ' ' || status || ' ' || COALESCE(error, '') || ' ' || input::text FROM engine.jobs ORDER BY id").fetch_all(&self.db.pool).await.unwrap();
        panic!(
            "timed out waiting for {what}; events: {:#?}; jobs: {jobs:#?}",
            self.notes().await
        );
    }

    async fn wait_status(&self, task: i64, pred: impl Fn(&Value) -> bool, what: &str) -> Value {
        self.until(what, || async {
            let p = self.projection(task).await;
            pred(&p).then_some(p)
        })
        .await
    }

    async fn started(&self, n: usize) -> enzarb_proto::StepStart {
        self.until(&format!("step start #{n}"), || async {
            let s = self.runner.started();
            (s.len() >= n).then(|| s[n - 1].clone())
        })
        .await
    }

    async fn set_secret(&self, name: &str, value: &str) {
        sqlx::query("SELECT engine_api.secret_set('project', 'p', $1, $2)")
            .bind(name)
            .bind(value)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn first_task(&self) -> i64 {
        self.until("a task", || async {
            sqlx::query_scalar("SELECT min(id) FROM engine.tasks")
                .fetch_one(&self.db.pool)
                .await
                .unwrap()
        })
        .await
    }
}

fn finished(run_id: &str, exit: &str) -> StepFinished {
    StepFinished {
        run_id: run_id.into(),
        exit: exit.into(),
        outputs: Default::default(),
        result: None,
        git: vec![],
        usage: None,
        environment: None,
        error: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn issue_to_merged_pull_request() {
    let Some(h) = start(Duration::from_secs(3600), true).await else {
        return;
    };
    let mut listener = sqlx::postgres::PgListener::connect_with(&h.db.pool)
        .await
        .unwrap();
    listener.listen("engine_projection").await.unwrap();
    h.set_secret("API_KEY", "sk-test-secret").await;
    h.engine.tick().await.unwrap();
    let ct: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT ciphertext FROM engine.secrets WHERE name = 'API_KEY'")
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
    assert!(ct.is_some(), "pending secret encrypted by the ticker");

    h.open_issue(7, "Crash on start").await;
    let task = h.first_task().await;
    let n = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(n.payload(), task.to_string());

    // Plan step: agent with the rendered prompt, the named secret and the result schema.
    let plan = h.started(1).await;
    let StepExec::Agent {
        prompt,
        result_schema,
        runtime,
        ..
    } = &plan.exec
    else {
        panic!("agent step")
    };
    assert_eq!(prompt, "Plan Crash on start");
    assert!(result_schema.is_some());
    assert_eq!(runtime.command, vec!["claude-agent-acp"]);
    assert_eq!(
        plan.env.get("API_KEY").map(|s| s.expose()),
        Some("sk-test-secret")
    );
    assert_eq!(plan.workspaces[0].repo, APP);
    assert_eq!(plan.workspaces[0].branch, format!("enzarb/task-{task}"));
    let p = h.projection(task).await;
    assert_eq!(p["status"], "running");
    assert_eq!(p["column"], "doing");

    // Session events are stored; steering reaches the running session.
    h.runner.push_event(
        &plan.run_id,
        "assistant",
        json!({"text": "thinking about it"}),
    );
    h.submit(task, "steer", json!({"body": "also check the logs"}))
        .await;
    h.until("steer", || async {
        h.runner
            .steers()
            .iter()
            .any(|(r, t)| *r == plan.run_id && t.contains("also check the logs"))
            .then_some(())
    })
    .await;
    let count: i64 = h
        .until("session event", || async {
            let c: i64 =
                sqlx::query_scalar("SELECT count(*) FROM engine.session_events WHERE run_id = $1")
                    .bind(&plan.run_id)
                    .fetch_one(&h.db.pool)
                    .await
                    .unwrap();
            (c > 0).then_some(c)
        })
        .await;
    assert_eq!(count, 1);

    // An invalid result fails the step; a retry is the workflow's business. Here: a valid one.
    let mut f = finished(&plan.run_id, "success");
    f.result = Some(json!({"summary": "fix the null check"}));
    h.runner.finish(f);
    let p = h
        .wait_status(
            task,
            |p| p["reason"] == "awaiting_approval",
            "approval wait",
        )
        .await;
    assert_eq!(p["needs_you"], true);
    assert_eq!(p["text"], "Approve the plan for #7");
    assert_eq!(p["actions"][0]["command"], "approve");

    // A non-writer cannot approve; alice can.
    h.perms.set("bob", Permission::Read);
    sqlx::query("SELECT engine_api.submit($1, '{\"login\": \"bob\"}', 'command', '{\"command\": \"approve\"}')")
        .bind(task).execute(&h.db.pool).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.projection(task).await["reason"], "awaiting_approval");
    h.submit(task, "command", json!({"command": "approve"}))
        .await;

    // Implement: a command step with a write workspace; the push is verified on GitHub.
    let imp = h.started(2).await;
    assert!(matches!(&imp.exec, StepExec::Command { run, .. } if run == "make fix ISSUE=7"));
    assert!(imp.workspaces[0].push);
    let branch = imp.workspaces[0].branch.clone();
    h.gh.set_branch_head(rk(APP), &branch, HEAD);
    h.gh.set_ci(rk(APP), HEAD, json!({"state": "success", "failing": []}));
    let mut f = finished(&imp.run_id, "success");
    f.git = vec![StepGit {
        repo: APP.into(),
        branch: branch.clone(),
        head_sha: HEAD.into(),
        pushed: true,
    }];
    h.runner.finish(f);
    let pr_num = h
        .until("pull request opened", || async {
            h.gh.change_requests().first().map(|p| p.number)
        })
        .await;
    let pr_key = WorkItemKey::new(rk(APP), pr_num);
    h.gh.set_merge_readiness(
        pr_key.clone(),
        MergeReadiness {
            head_sha: HEAD.into(),
            base_sha: None,
            mergeable: Some(true),
            mergeable_state: "clean".into(),
            ci: enzarb_connector::CiState::Success,
        },
    );

    // open PR (linked as a subject) -> CI fact -> merge -> wait for the merged event.
    let p = h
        .wait_status(task, |p| p["reason"] == "awaiting_merge", "merge wait")
        .await;
    let prs = h.gh.change_requests();
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].number, pr_num);
    assert_eq!(prs[0].title, "Fix #7");
    assert!(
        p["subjects"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["key"] == format!("github:{APP}#{pr_num}"))
    );
    let merges =
        h.gh.calls()
            .iter()
            .filter(|c| {
                matches!(
                    c.call,
                    enzarb_connector::fake::Call::MergeChangeRequest { .. }
                )
            })
            .count();
    assert_eq!(merges, 1);

    h.ingest("github:pr-closed", Some(format!("github:{APP}#{pr_num}")), "pull_request",
        json!({"type": "pull_request", "action": "closed", "number": pr_num, "merged": true,
               "repo": APP, "actor": {"login": "alice"}, "subject": {"kind": "pull_request", "number": pr_num}})).await;
    let p = h
        .wait_status(task, |p| p["status"] == "ended", "ended")
        .await;
    assert_eq!(p["end_status"], "done");
    assert_eq!(p["column"], "done");

    // History, metrics and the detail view.
    let detail: Value = sqlx::query_scalar("SELECT engine_api.task_detail($1)")
        .bind(task)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(detail["step_runs"].as_array().unwrap().len(), 2);
    assert!(detail["transitions"].as_array().unwrap().len() >= 6);
    assert_eq!(detail["facts"]["ci"]["state"], "success");
    let board: Value = sqlx::query_scalar("SELECT engine_api.board_v('p')")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(board["columns"][0]["id"], "review");
    let wf: Value = sqlx::query_scalar("SELECT engine_api.workflow_v('p', 'acme/app')")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(wf["workflows"][0]["phases"][0]["id"], "plan");
    let runs: i64 = sqlx::query_scalar("SELECT sum(runs)::bigint FROM engine_api.metrics('p', now() - interval '1 hour', now() + interval '1 hour')")
        .fetch_one(&h.db.pool).await.unwrap();
    assert_eq!(runs, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pull_request_links_to_issue_task_and_unrouted_events_are_kept() {
    let Some(h) = start(Duration::from_secs(3600), true).await else {
        return;
    };
    h.set_secret("API_KEY", "k").await;
    h.open_issue(8, "Link me").await;
    let task = h.first_task().await;
    h.started(1).await;
    h.gh.set_pr_references(WorkItemKey::new(rk(APP), 20), vec![8]);
    h.ingest("github:pr20", Some(format!("github:{APP}#20")), "pull_request",
        json!({"type": "pull_request", "action": "opened", "number": 20, "title": "Fix", "body": "",
               "repo": APP, "actor": {"login": "carol"}, "subject": {"kind": "pull_request", "number": 20}})).await;
    let linked: i64 = h
        .until("pr linked", || async {
            sqlx::query_scalar("SELECT task_id FROM engine.subjects WHERE key = $1")
                .bind(format!("github:{APP}#20"))
                .fetch_optional(&h.db.pool)
                .await
                .unwrap()
        })
        .await;
    assert_eq!(linked, task);
    let tasks: i64 = sqlx::query_scalar("SELECT count(*) FROM engine.tasks")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(tasks, 1, "no separate PR task");

    // An unlinked PR without references starts its own task through the `prs` binding.
    h.ingest(
        "github:pr21",
        Some(format!("github:{APP}#21")),
        "pull_request",
        json!({"type": "pull_request", "action": "opened", "number": 21, "repo": APP,
               "actor": {"login": "carol"}, "subject": {"kind": "pull_request", "number": 21}}),
    )
    .await;
    // A label on an unknown issue matches no binding: recorded, not routed.
    h.ingest(
        "github:label",
        Some(format!("github:{APP}#99")),
        "label",
        json!({"type": "label", "action": "added", "label": "x", "repo": APP,
               "subject": {"kind": "issue", "number": 99}}),
    )
    .await;
    let note: String = h.until("unrouted note", || async {
        sqlx::query_scalar("SELECT note FROM engine.events WHERE dedupe_key = 'github:label' AND processed_at IS NOT NULL")
            .fetch_optional(&h.db.pool).await.unwrap().flatten()
    }).await;
    assert!(note.starts_with("unrouted"), "{note}");
    h.until("pr task", || async {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM engine.tasks")
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
        (n == 2).then_some(())
    })
    .await;
    // Duplicate deliveries are dropped by the inbox.
    assert!(
        h.ingest("github:label", None, "label", json!({}))
            .await
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopt_pass_is_idempotent() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let issue = |n: u64| enzarb_connector::ItemRef {
        kind: enzarb_connector::ItemKind::Issue,
        number: n,
        url: String::new(),
        title: format!("old {n}"),
        body: String::new(),
        author: "alice".into(),
        head_ref: None,
        base_ref: None,
        head_sha: None,
    };
    h.gh.set_open_issues(rk(APP), vec![issue(1), issue(2)]);
    assert_eq!(h.engine.adopt().await.unwrap(), 2);
    while h.engine.process_next().await.unwrap() {}
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM engine.tasks")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(n, 2, "{:#?}", h.notes().await);
    assert_eq!(h.engine.adopt().await.unwrap(), 0);
    while h.engine.process_next().await.unwrap() {}
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM engine.tasks")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(n, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_step_resumes_and_missing_secret_waits_on_a_human() {
    let Some(h) = start(Duration::from_secs(3600), true).await else {
        return;
    };
    // No API_KEY yet: the step waits on a human, the task pauses.
    h.open_issue(5, "Secrets").await;
    let task = h.first_task().await;
    let p = h
        .wait_status(
            task,
            |p| p["status"] == "paused",
            "paused on missing secret",
        )
        .await;
    assert!(p["text"].as_str().unwrap().contains("API_KEY"), "{p}");
    assert!(h.runner.started().is_empty());
    h.set_secret("API_KEY", "k").await;
    h.submit(task, "resume", json!({})).await;
    let first = h.started(1).await;

    // The sandbox goes away mid-step: the job retries and the same run is resumed, not restarted.
    h.runner.interrupt_once(&first.run_id);
    let again = h.started(2).await;
    assert_eq!(again.run_id, first.run_id);
    let mut f = finished(&first.run_id, "success");
    f.result = Some(json!({"summary": "ok"}));
    h.runner.finish(f);
    h.wait_status(task, |p| p["reason"] == "awaiting_approval", "approval")
        .await;
    let resumes: i32 = sqlx::query_scalar("SELECT resumes FROM engine.step_runs WHERE run_id = $1")
        .bind(&first.run_id)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(resumes, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quiet_step_stalls_and_pauses_and_bad_results_fail() {
    let Some(h) = start(Duration::from_secs(1), true).await else {
        return;
    };
    h.set_secret("API_KEY", "k").await;
    h.open_issue(6, "Stall").await;
    let task = h.first_task().await;
    let run = h.started(1).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let st = h.engine.tick().await.unwrap();
    assert_eq!(st.stalls, 1);
    assert_eq!(
        h.engine.tick().await.unwrap().stalls,
        0,
        "reported once per quiet period"
    );
    let p = h
        .wait_status(task, |p| p["status"] == "paused", "stall pause")
        .await;
    assert!(p["text"].as_str().unwrap().contains("no activity"));
    // A result that violates its schema fails the step (the step's own result unpauses it).
    let mut f = finished(&run.run_id, "success");
    f.result = Some(json!({"nope": 1}));
    h.runner.finish(f);
    let p = h
        .wait_status(task, |p| p["status"] == "ended", "failed")
        .await;
    assert_eq!(p["end_status"], "failed");
}

/// The user-scope secret `claude_token` comes from the UI-captured per-user Claude credential
/// (`app.claude_credentials`) of the given user, injected by its declared env/file; an explicit
/// `engine.secrets` user row wins; another user's token and other scopes are never used.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claude_token_comes_from_the_users_stored_credential() {
    use engine::claude_credential::ClaudeKeyring;
    use engine::secrets::{ClaudeSource, resolve};
    use enzarb_flow::format::{Secret, SecretScope};
    let Some(db) = TestDb::start().await else {
        return;
    };
    db.install_claude_credentials().await;
    let ring = ClaudeKeyring::new().with_key("k1", [3u8; 32]);
    let ct = ring
        .encrypt_with_nonce("alice", "k1", [1u8; 12], "sk-ant-oat01-alice")
        .unwrap();
    db.put_claude_credential("alice", "k1", &ct).await;
    let src = ClaudeSource {
        name: "claude_token".into(),
        keyring: ring,
    };
    let decls = vec![Secret {
        name: "claude_token".into(),
        scope: SecretScope::User,
        env: Some("CLAUDE_CODE_OAUTH_TOKEN".into()),
        file: Some(".enzarb/token".into()),
    }];
    let r = resolve(&db.pool, None, Some(&src), Some("alice"), "p", &decls)
        .await
        .unwrap();
    assert!(r.missing.is_empty());
    assert_eq!(r.env["CLAUDE_CODE_OAUTH_TOKEN"], "sk-ant-oat01-alice");
    assert_eq!(r.files[".enzarb/token"], "sk-ant-oat01-alice");
    // Another user has no row: missing (never alice's).
    let r = resolve(&db.pool, None, Some(&src), Some("bob"), "p", &decls)
        .await
        .unwrap();
    assert!(r.env.is_empty());
    assert_eq!(r.missing, vec!["user:claude_token".to_string()]);
    // Project scope never reads the user store.
    let mut proj = decls.clone();
    proj[0].scope = SecretScope::Project;
    let r = resolve(&db.pool, None, Some(&src), Some("alice"), "p", &proj)
        .await
        .unwrap();
    assert_eq!(r.missing, vec!["project:claude_token".to_string()]);
    // An explicit user secret wins.
    sqlx::query(
        "INSERT INTO engine.secrets (scope, owner, name, pending) VALUES ('user', 'alice', 'claude_token', 'explicit')",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let r = resolve(&db.pool, None, Some(&src), Some("alice"), "p", &decls)
        .await
        .unwrap();
    assert_eq!(r.env["CLAUDE_CODE_OAUTH_TOKEN"], "explicit");
}

/// Chats recorded by sandbox-proxy (`engine_api.chat_record`/`chat_event`, run `chat-<id>`) show
/// in the UI's chat list and session reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_capture_feeds_chat_v_and_session_v() {
    let Some(db) = TestDb::start().await else {
        return;
    };
    let proxy = db.pool_as("enzarb_sandbox_proxy").await;
    sqlx::query("SELECT engine_api.chat_record('chat-s1', 'p', 'octo', '')")
        .execute(&proxy)
        .await
        .unwrap();
    sqlx::query("SELECT engine_api.chat_record('chat-s1', 'p', 'octo', 'Fix the bug')")
        .execute(&proxy)
        .await
        .unwrap();
    for (k, p) in [
        ("user", json!({"text": "Fix the bug"})),
        ("assistant", json!({"content": {"text": "ok"}})),
    ] {
        sqlx::query("SELECT engine_api.chat_event('chat-s1', $1, $2)")
            .bind(k)
            .bind(p)
            .execute(&proxy)
            .await
            .unwrap();
    }
    // Unknown chats are refused; the proxy may not read tables.
    assert!(
        sqlx::query("SELECT engine_api.chat_event('chat-nope', 'user', '{}')")
            .execute(&proxy)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT 1 FROM engine.chats")
            .execute(&proxy)
            .await
            .is_err()
    );
    let ui = db.pool_as("enzarb_ui").await;
    let (title, events): (String, i64) =
        sqlx::query_as("SELECT title, events FROM engine_api.chat_v('octo')")
            .fetch_one(&ui)
            .await
            .unwrap();
    assert_eq!((title.as_str(), events), ("Fix the bug", 2));
    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM engine_api.session_v('chat-s1', 0, 10)")
            .fetch_all(&ui)
            .await
            .unwrap();
    assert_eq!(kinds, ["user", "assistant"]);
}

/// A step token GitHub refuses (422: permissions not granted to the installation) is not
/// retried: the task pauses naming the step and the permissions, nothing reaches the sandbox,
/// and a resume after a library fix re-resolves and starts the step from the new definition.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refused_step_token_pauses_and_resume_reresolves() {
    let Some(h) = start(Duration::from_secs(3600), true).await else {
        return;
    };
    h.set_secret("API_KEY", "k").await;
    h.gh.fail_step_token(enzarb_connector::Error::Invalid(
        "create installation token: 422 Unprocessable Entity: {\"message\":\"The permissions requested are not granted to this installation.\"}".into(),
    ));
    h.open_issue(3, "Perms").await;
    let task = h.first_task().await;
    let p = h
        .wait_status(task, |p| p["status"] == "paused", "paused on refused token")
        .await;
    let text = p["text"].as_str().unwrap();
    assert!(
        text.contains(
            "the GitHub App installation lacks contents: read requested by step plan/plan"
        ),
        "{text}"
    );
    assert!(h.runner.started().is_empty(), "no step_start sent");
    let (exit, jobs): (String, i64) = sqlx::query_as(
        "SELECT r.exit, (SELECT count(*) FROM engine.jobs j WHERE j.task_id = r.task_id AND j.status <> 'done')
         FROM engine.step_runs r WHERE r.task_id = $1",
    )
    .bind(task)
    .fetch_one(&h.db.pool)
    .await
    .unwrap();
    assert_eq!((exit.as_str(), jobs), ("not_started", 0), "not retried");

    // The library changes the step; resume re-resolves and starts it from the new definition.
    let v2 = "4444444444444444444444444444444444444444";
    let plan = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lib/steps/plan.yaml"),
    )
    .unwrap()
    .replace("Plan {{", "Plan v2 {{");
    load_library_at(&h.gh, v2, &[("steps/plan.yaml", &plan)]);
    h.gh.set_ref(rk(LIB), "v1", v2);
    h.submit(task, "resume", json!({})).await;
    let run = h.started(1).await;
    let StepExec::Agent { prompt, .. } = &run.exec else {
        panic!("agent step")
    };
    assert_eq!(prompt, "Plan v2 Perms");
    let sha: String =
        sqlx::query_scalar("SELECT resolved->'entry'->>'sha' FROM engine.tasks WHERE id = $1")
            .bind(task)
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
    assert_eq!(sha, v2);
    assert_eq!(h.projection(task).await["status"], "running");
}

/// UI cancel is engine-native and writer-only: a non-writer is ignored; a writer's cancel stops
/// the running step and ends the task `cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ui_cancel_stops_the_step_and_ends_the_task() {
    let Some(h) = start(Duration::from_secs(3600), true).await else {
        return;
    };
    h.set_secret("API_KEY", "k").await;
    h.open_issue(4, "Cancel me").await;
    let task = h.first_task().await;
    let run = h.started(1).await;
    sqlx::query("SELECT engine_api.submit($1, '{\"login\": \"bob\"}', 'cancel', '{}')")
        .bind(task)
        .execute(&h.db.pool)
        .await
        .unwrap();
    let note: String = h
        .until("bob refused", || async {
            sqlx::query_scalar(
                "SELECT note FROM engine.events WHERE type = 'ui.cancel' AND processed_at IS NOT NULL",
            )
            .fetch_optional(&h.db.pool)
            .await
            .unwrap()
            .flatten()
        })
        .await;
    assert_eq!(note, "bob may not cancel");
    assert_eq!(h.projection(task).await["status"], "running");
    assert!(h.runner.stops().is_empty());

    h.submit(task, "cancel", json!({})).await;
    let p = h
        .wait_status(task, |p| p["status"] == "ended", "cancelled")
        .await;
    assert_eq!(p["end_status"], "cancelled");
    h.until("step stopped", || async {
        h.runner.stops().contains(&run.run_id).then_some(())
    })
    .await;
}

/// Every step run is recorded under its own task (run ids are unique across tasks), and a
/// read-only workspace step gets a `contents: read` token for its own git calls.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_step_run_is_recorded_and_read_steps_get_a_token() {
    let Some(h) = start(Duration::from_secs(3600), true).await else {
        return;
    };
    h.set_secret("API_KEY", "k").await;
    h.open_issue(1, "One").await;
    h.open_issue(2, "Two").await;
    let a = h.started(1).await;
    let b = h.started(2).await;
    assert_ne!(a.run_id, b.run_id, "run ids are unique across tasks");
    for s in [&a, &b] {
        assert_eq!(s.workspaces[0].mode, enzarb_proto::Mode::Read);
        assert!(!s.workspaces[0].token.expose().is_empty());
        assert!(
            s.github_token
                .as_ref()
                .is_some_and(|t| !t.expose().is_empty())
        );
    }
    let perms: Vec<Value> =
        h.gh.generic_calls("mint_step_token")
            .into_iter()
            .map(|c| c["permissions"].clone())
            .collect();
    assert!(
        perms.iter().all(|p| *p == json!({"contents": "read"})),
        "{perms:?}"
    );
    // Both result-only steps finish and are recorded, one row per task.
    for s in [&a, &b] {
        let mut f = finished(&s.run_id, "success");
        f.result = Some(json!({"summary": "ok"}));
        h.runner.finish(f);
    }
    let rows: Vec<(i64, String, String)> = h
        .until("both recorded", || async {
            let r: Vec<(i64, String, String)> = sqlx::query_as(
                "SELECT task_id, run_id, exit FROM engine.step_runs WHERE exit IS NOT NULL ORDER BY task_id",
            )
            .fetch_all(&h.db.pool)
            .await
            .unwrap();
            (r.len() == 2).then_some(r)
        })
        .await;
    assert_ne!(rows[0].0, rows[1].0);
    assert!(rows.iter().all(|(_, _, e)| e == "success"));
}

// ---------------------------------------------------------------- liveness findings

use engine::findings::Fact;
use enzarb_flow::format::PlatformFactName;

/// An open task waiting on `external` since two hours ago, with the given facts.
async fn waiting_task(h: &H, ci: Value, mergeable: Value, unknown_refreshes: i32) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO engine.tasks (project, repo, workflow_ref, state, resolved, created_at)
         VALUES ('p', $1, 'wf', '{}', '{}', now() - interval '2 hours') RETURNING id",
    )
    .bind(APP)
    .fetch_one(&h.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO engine.projection (task_id, project, repo, workflow, status, phase, node, waiting_on)
         VALUES ($1, 'p', $2, 'wf', 'waiting', 'review', 'review/wait', 'external')",
    )
    .bind(id)
    .bind(APP)
    .execute(&h.db.pool)
    .await
    .unwrap();
    set_fact(h, id, "ci", ci, 0).await;
    set_fact(h, id, "mergeable", mergeable, unknown_refreshes).await;
    id
}

async fn set_fact(h: &H, task: i64, name: &str, value: Value, unknown_refreshes: i32) {
    sqlx::query(
        "INSERT INTO engine.facts (task_id, name, value, source, unknown_refreshes)
         VALUES ($1, $2, $3, 'test', $4)
         ON CONFLICT (task_id, name) DO UPDATE SET value = $3, unknown_refreshes = $4",
    )
    .bind(task)
    .bind(name)
    .bind(value)
    .bind(unknown_refreshes)
    .execute(&h.db.pool)
    .await
    .unwrap();
}

async fn open_findings(h: &H) -> Vec<(String, Option<i64>, String)> {
    sqlx::query_as("SELECT key, task_id, severity FROM engine_api.finding_v ORDER BY key")
        .fetch_all(&h.db.pool)
        .await
        .unwrap()
}

async fn task_v(h: &H, id: i64) -> (Option<String>, Option<String>, Value) {
    sqlx::query_as("SELECT reason, text, findings FROM engine_api.task_v WHERE task_id = $1")
        .bind(id)
        .fetch_one(&h.db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn stuck_task_yields_exactly_one_finding_in_the_projection() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let id = waiting_task(&h, json!({"sha": "abc", "check_runs": []}), Value::Null, 1).await;
    // Raised only once the condition held on two consecutive passes.
    h.engine.findings_pass().await.unwrap();
    assert!(open_findings(&h).await.is_empty());
    for _ in 0..4 {
        h.engine.findings_pass().await.unwrap();
    }
    let open = open_findings(&h).await;
    assert_eq!(
        open,
        [(format!("ci-never-started:{id}"), Some(id), "warning".into())]
    );
    let (reason, text, findings) = task_v(&h, id).await;
    assert_eq!(reason.as_deref(), Some("ci-never-started"));
    assert!(text.unwrap().contains("no check runs"));
    assert_eq!(findings.as_array().unwrap().len(), 1);
    assert_eq!(findings[0]["kind"], "dead_wait");
    // task_detail carries the same.
    let detail: Value = sqlx::query_scalar("SELECT engine_api.task_detail($1)")
        .bind(id)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(detail["findings"].as_array().unwrap().len(), 1);
    // The task itself is untouched.
    let (status, ended): (String, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as("SELECT p.status, t.ended_at FROM engine.projection p JOIN engine.tasks t ON t.id = p.task_id WHERE t.id = $1")
            .bind(id)
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
    assert_eq!((status.as_str(), ended), ("waiting", None));
}

#[tokio::test]
async fn healthy_task_yields_no_finding() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let id = waiting_task(
        &h,
        json!({"sha": "abc", "check_runs": [{"name": "test"}]}),
        json!(true),
        0,
    )
    .await;
    for _ in 0..3 {
        h.engine.findings_pass().await.unwrap();
    }
    assert!(open_findings(&h).await.is_empty());
    let (reason, text, findings) = task_v(&h, id).await;
    assert_eq!((reason, text), (None, None));
    assert_eq!(findings, json!([]));
}

#[tokio::test]
async fn resolved_condition_clears_the_finding() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let id = waiting_task(&h, json!({"check_runs": []}), json!(false), 0).await;
    for _ in 0..2 {
        h.engine.findings_pass().await.unwrap();
    }
    assert_eq!(open_findings(&h).await.len(), 1);
    // Check runs arrive.
    set_fact(&h, id, "ci", json!({"check_runs": [{"name": "test"}]}), 0).await;
    h.engine.findings_pass().await.unwrap();
    assert!(open_findings(&h).await.is_empty());
    let (reason, text, findings) = task_v(&h, id).await;
    assert_eq!((reason, text, findings), (None, None, json!([])));
    // Kept for history.
    let cleared: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM engine.findings WHERE key = $1 AND cleared_at IS NOT NULL",
    )
    .bind(format!("ci-never-started:{id}"))
    .fetch_one(&h.db.pool)
    .await
    .unwrap();
    assert_eq!(cleared, 1);
    // The condition returns: the same key reopens (after two passes), once.
    set_fact(&h, id, "ci", json!({"check_runs": []}), 0).await;
    for _ in 0..2 {
        h.engine.findings_pass().await.unwrap();
    }
    assert_eq!(open_findings(&h).await.len(), 1);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM engine.findings")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn finding_survives_projection_rewrites_and_interpreter_reasons_win() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let id = waiting_task(&h, json!({"check_runs": []}), Value::Null, 1).await;
    for _ in 0..2 {
        h.engine.findings_pass().await.unwrap();
    }
    let before = task_v(&h, id).await;
    assert!(before.1.is_some());
    // A reducer transition rewrites the projection (same upsert as `write_projection`).
    sqlx::query(
        "UPDATE engine.projection SET status = 'waiting', node = 'review/wait', reason = NULL,
             text = NULL, updated_at = now() WHERE task_id = $1",
    )
    .bind(id)
    .execute(&h.db.pool)
    .await
    .unwrap();
    assert_eq!(task_v(&h, id).await, before);
    // The interpreter has a reason: it wins, the finding stays listed.
    sqlx::query("UPDATE engine.projection SET reason = 'awaiting_ci', text = 'Waiting for CI' WHERE task_id = $1")
        .bind(id)
        .execute(&h.db.pool)
        .await
        .unwrap();
    let (reason, text, findings) = task_v(&h, id).await;
    assert_eq!(reason.as_deref(), Some("awaiting_ci"));
    assert_eq!(text.as_deref(), Some("Waiting for CI"));
    assert_eq!(findings.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn two_checks_on_one_task_show_the_highest_severity() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    // `mergeable-unknown` (info) and `ci-never-started` (warning) both match.
    let id = waiting_task(&h, json!({"check_runs": []}), Value::Null, 3).await;
    for _ in 0..2 {
        h.engine.findings_pass().await.unwrap();
    }
    assert_eq!(open_findings(&h).await.len(), 2);
    let (reason, _, findings) = task_v(&h, id).await;
    assert_eq!(reason.as_deref(), Some("ci-never-started"));
    assert_eq!(findings[0]["severity"], "warning");
    assert_eq!(findings[1]["severity"], "info");
}

#[tokio::test]
async fn findings_pass_only_writes_findings() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    waiting_task(&h, json!({"check_runs": []}), Value::Null, 3).await;
    let dump = || async {
        let mut all = Vec::new();
        for t in [
            "tasks",
            "projection",
            "transitions",
            "events",
            "timers",
            "jobs",
            "step_runs",
            "facts",
            "subjects",
            "relations",
        ] {
            let v: Value = sqlx::query_scalar(&format!(
                "SELECT COALESCE(jsonb_agg(to_jsonb(t)), '[]') FROM engine.{t} t"
            ))
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
            all.push((t, v));
        }
        all
    };
    let before = dump().await;
    for _ in 0..3 {
        h.engine.findings_pass().await.unwrap();
    }
    assert_eq!(open_findings(&h).await.len(), 2);
    assert_eq!(dump().await, before);
}

#[tokio::test]
async fn platform_findings_follow_the_platform_facts() {
    let Some(mut h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let facts = engine::FakePlatformFacts::new();
    h.engine.findings = engine::Findings::new(Arc::new(facts.clone()));
    // Unknown: nothing is raised.
    for _ in 0..3 {
        h.engine.findings_pass().await.unwrap();
    }
    assert!(open_findings(&h).await.is_empty());
    facts.set("p", PlatformFactName::SandboxDiskPct, Fact::Num(95.0));
    for _ in 0..3 {
        h.engine.findings_pass().await.unwrap();
    }
    assert_eq!(
        open_findings(&h).await,
        [("sandbox-disk:p".to_string(), None, "critical".to_string())]
    );
    // The source goes away: the finding stays as it is.
    facts.set("p", PlatformFactName::SandboxDiskPct, Fact::Unknown);
    h.engine.findings_pass().await.unwrap();
    assert_eq!(open_findings(&h).await.len(), 1);
    facts.set("p", PlatformFactName::SandboxDiskPct, Fact::Num(40.0));
    h.engine.findings_pass().await.unwrap();
    assert!(open_findings(&h).await.is_empty());
}

#[tokio::test]
async fn ticker_runs_findings_on_its_interval() {
    let Some(mut h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    h.engine.findings = engine::Findings::none().with_interval(Duration::from_secs(3600));
    assert!(h.engine.run_findings().await.unwrap().is_some());
    assert!(h.engine.run_findings().await.unwrap().is_none());
    h.engine.tick().await.unwrap();
}

#[tokio::test]
async fn refreshing_a_fact_counts_unknown_refreshes() {
    let Some(h) = start(Duration::from_secs(3600), false).await else {
        return;
    };
    let id = waiting_task(&h, json!({}), Value::Null, 0).await;
    let refresh = |v: Value| {
        let pool = h.db.pool.clone();
        async move {
            engine::executor::store_fact(&pool, id, "mergeable", &v)
                .await
                .unwrap();
            sqlx::query_scalar::<_, i32>("SELECT unknown_refreshes FROM engine.facts WHERE task_id = $1 AND name = 'mergeable'")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(refresh(Value::Null).await, 1);
    assert_eq!(refresh(Value::Null).await, 2);
    assert_eq!(refresh(json!(true)).await, 0);
    assert_eq!(refresh(Value::Null).await, 1);
}
