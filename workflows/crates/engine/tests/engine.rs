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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lib");
    fn walk(gh: &FakeConnector, root: &std::path::Path, dir: &std::path::Path) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(gh, root, &p);
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().to_string();
                gh.set_file(
                    rk(LIB),
                    LIB_SHA,
                    &rel,
                    &std::fs::read_to_string(&p).unwrap(),
                );
            }
        }
    }
    walk(gh, &root, &root);
    gh.set_ref(rk(LIB), "v1", LIB_SHA);
    gh.set_ref(rk(APP), "main", APP_SHA);
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
        library: Library::new(gh.clone(), &format!("{LIB}@v1"), vec![]),
        permissions: perms.clone(),
        runner: Arc::new(runner.clone()),
        sandboxes: Arc::new(StaticSandboxResolver::new()),
        secrets_key: Some(key),
        claude: None,
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
