//! End-to-end tests against the real standard library (snapshot in `tests/stdlib`, served as
//! `enzarb/workflows@v1` by the fake connector): binding routing (`if`/`with`, relations,
//! `mentioned`) and the whole `workflows/issue` happy path with fake sandbox results.

use std::sync::Arc;
use std::time::Duration;

use engine::findings::Fact;
use engine::testing::TestDb;
use engine::{
    Engine, EngineConfig, FakeStepRunner, Library, ProjectInfo, StaticPermissions,
    StaticProjectIndex, StaticSandboxResolver,
};
use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{MergeReadiness, Permission, RepoKey, WorkItemKey};
use enzarb_flow::format::PlatformFactName;
use enzarb_proto::{StepFinished, StepGit};
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
    stop: watch::Sender<bool>,
}

impl Drop for H {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

fn load_stdlib(gh: &FakeConnector) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/stdlib");
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

async fn start(background: bool) -> Option<H> {
    let db = TestDb::start().await?;
    let gh = Arc::new(FakeConnector::new());
    load_stdlib(&gh);
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
    let engine = Engine {
        pool: db.pool.clone(),
        index,
        library: Library::new(gh.clone(), &format!("{LIB}@v1"), vec![]),
        permissions: perms,
        runner: Arc::new(runner.clone()),
        sandboxes: Arc::new(StaticSandboxResolver::new()),
        secrets_key: None,
        claude: None,
        findings: engine::Findings::none(),
        cfg: EngineConfig {
            stall: Duration::from_secs(3600),
            poll: Duration::from_millis(50),
            backoff_base: Duration::from_millis(100),
            public_url: "https://enzarb.test".into(),
            ..EngineConfig::default()
        },
    };
    // The Project owner's Claude token (user scope), as the runtime declares it.
    sqlx::query("SELECT engine_api.secret_set('user', 'alice', 'claude_token', 'sk-ant-test')")
        .execute(&db.pool)
        .await
        .unwrap();
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
        stop,
    })
}

impl H {
    async fn ingest(&self, dedupe: &str, subject: Option<String>, ty: &str, payload: Value) {
        sqlx::query("SELECT engine_api.ingest($1, NULL, $2, $3, $4)")
            .bind(dedupe)
            .bind(subject)
            .bind(ty)
            .bind(payload)
            .execute(&self.db.pool)
            .await
            .unwrap();
    }

    async fn open_issue(&self, n: u64, labels: &[&str]) {
        self.ingest(
            &format!("issue-{n}"),
            Some(format!("github:{APP}#{n}")),
            "issue",
            json!({"type": "issue", "action": "opened", "number": n, "title": format!("Issue {n}"),
                   "body": "b", "labels": labels, "repo": APP, "actor": {"login": "alice"},
                   "subject": {"kind": "issue", "number": n}}),
        )
        .await;
    }

    async fn comment(&self, dedupe: &str, n: u64, kind: &str, body: &str, login: &str) {
        let parsed = enzarb_flow::events::Comment::parsed(body, Some("enzarb"));
        self.ingest(
            dedupe,
            Some(format!("github:{APP}#{n}")),
            "comment",
            json!({"type": "comment", "body": body, "command": parsed.command, "args": parsed.args,
                   "repo": APP, "actor": {"login": login}, "subject": {"kind": kind, "number": n}}),
        )
        .await;
    }

    async fn drain(&self) {
        while self.engine.process_next().await.unwrap() {}
    }

    async fn task_of(&self, n: u64) -> Option<(i64, String, Option<String>, Value)> {
        sqlx::query_as(
            "SELECT t.id, t.workflow_ref, t.binding, t.resolved->'params' FROM engine.subjects s
             JOIN engine.tasks t ON t.id = s.task_id WHERE s.key = $1",
        )
        .bind(format!("github:{APP}#{n}"))
        .fetch_optional(&self.db.pool)
        .await
        .unwrap()
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

    /// Waits (up to 20 s) for the projection to satisfy `pred`.
    async fn wait(&self, task: i64, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..400 {
            let p = self.projection(task).await;
            if pred(&p) {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let jobs: Vec<String> = sqlx::query_scalar(
            "SELECT kind || ' ' || status || ' ' || COALESCE(error, '') FROM engine.jobs ORDER BY id",
        )
        .fetch_all(&self.db.pool)
        .await
        .unwrap();
        let state: Value =
            sqlx::query_scalar("SELECT state - 'doc' FROM engine.tasks WHERE id = $1")
                .bind(task)
                .fetch_one(&self.db.pool)
                .await
                .unwrap();
        panic!(
            "timed out waiting for {what}: {:#}\nstate: {state:#}\nevents: {:#?}\njobs: {jobs:#?}",
            self.projection(task).await,
            self.notes().await
        );
    }
}

/// Bindings from the library: a plain issue runs `issue`; an `epic`-labelled one runs it with
/// `epic: true`; a sub-issue the epic created (the epic task related as `parent`) gets
/// `epic_allowed: false`; a mention on an object without a task runs `mention`, a bot's not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn library_bindings_route_by_relations_and_mentions() {
    let Some(h) = start(false).await else {
        return;
    };
    h.open_issue(1, &[]).await;
    h.open_issue(2, &["epic"]).await;
    h.drain().await;
    let (_, wf, binding, params) = h.task_of(1).await.expect("task of #1");
    assert!(wf.ends_with(":workflows/issue.yaml"), "{wf}");
    assert_eq!(binding.as_deref(), Some("issues"));
    assert_eq!(params["epic_allowed"], json!(true), "{params}");
    let (epic, _, binding, params) = h.task_of(2).await.expect("task of #2");
    assert_eq!(binding.as_deref(), Some("epics"));
    assert_eq!(params["epic"], json!(true), "{params}");

    // The epic created #3 and related it as its child (`relate: {kind: parent}` rows).
    sqlx::query(
        "INSERT INTO engine.relations (task_id, kind, target_key, source) VALUES ($1, 'parent', $2, 'workflow')",
    )
    .bind(epic)
    .bind(format!("github:{APP}#3"))
    .execute(&h.db.pool)
    .await
    .unwrap();
    h.open_issue(3, &["epic"]).await;
    h.drain().await;
    let (_, _, binding, params) = h.task_of(3).await.expect("task of #3");
    assert_eq!(
        binding.as_deref(),
        Some("issues"),
        "a sub-issue is never an epic"
    );
    assert_eq!(params["epic_allowed"], json!(false), "{params}");

    // Mentions on an object without a task.
    h.comment("c-bot", 40, "pull_request", "@enzarb hi", "enzarb[bot]")
        .await;
    h.comment("c-plain", 41, "pull_request", "no mention here", "alice")
        .await;
    h.comment(
        "c-mention",
        42,
        "pull_request",
        "@Enzarb please look",
        "alice",
    )
    .await;
    h.drain().await;
    assert!(h.task_of(40).await.is_none());
    assert!(h.task_of(41).await.is_none());
    let (_, wf, binding, _) = h.task_of(42).await.expect("mention task");
    assert_eq!(binding.as_deref(), Some("mentions"));
    assert!(wf.ends_with(":workflows/mention.yaml"), "{wf}");

    // The adopt pass starts open issues without a task; they wait at the start gate.
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
    h.gh.set_open_issues(rk(APP), vec![issue(1), issue(50)]);
    assert_eq!(h.engine.adopt().await.unwrap(), 1);
    h.drain().await;
    let (t50, _, binding, _) = h.task_of(50).await.expect("adopted");
    assert_eq!(binding.as_deref(), Some("adopted_issues"));
    let p = h.projection(t50).await;
    // Waiting (the dependencies refresh job is not run in this foreground test, or at the start
    // gate once `decide` passes on a known-empty fact); never running a step.
    assert_eq!(p["status"], "waiting", "{p:#}");
    assert!(
        ["awaiting_start", "blocked_by_dependencies"].contains(&p["reason"].as_str().unwrap_or("")),
        "{p:#}"
    );
    assert!(
        h.runner.started().is_empty(),
        "adoption never starts an agent"
    );
}

fn finished(run_id: &str) -> StepFinished {
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

/// Fake sandbox: answers each step by its node.
fn spawn_responder(h: &H) -> tokio::task::JoinHandle<()> {
    spawn_responder_with(h, |_, _| {})
}

/// [`spawn_responder`] with a hook that edits each answer (`leaf` is the step's node name).
fn spawn_responder_with(
    h: &H,
    tweak: impl Fn(&str, &mut StepFinished) + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let runner = h.runner.clone();
    let pool = h.db.pool.clone();
    let gh = h.gh.clone();
    tokio::spawn(async move {
        let mut done = 0;
        loop {
            let started = runner.started();
            for s in started.iter().skip(done) {
                let node: String =
                    sqlx::query_scalar("SELECT node FROM engine.step_runs WHERE run_id = $1")
                        .bind(&s.run_id)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                assert_eq!(
                    s.env.get("CLAUDE_CODE_OAUTH_TOKEN").map(|t| t.expose()),
                    matches!(s.exec, enzarb_proto::StepExec::Agent { .. }).then_some("sk-ant-test"),
                    "{node}"
                );
                let mut f = finished(&s.run_id);
                let leaf = node.rsplit('/').next().unwrap_or_default().to_string();
                match leaf.as_str() {
                    "triage" => {
                        f.result = Some(json!({"decision": "ready", "body": "Null config."}))
                    }
                    "plan" => {
                        f.outputs
                            .insert("plan".into(), json!("## Summary\nGuard the config."));
                    }
                    "implement" => {
                        let branch = s.workspaces[0].branch.clone();
                        gh.set_branch_head(rk(APP), &branch, HEAD);
                        f.result = Some(
                            json!({"decision": "done", "title": "fix: no crash", "body": "Closes #7"}),
                        );
                        f.git = vec![StepGit {
                            repo: APP.into(),
                            branch,
                            head_sha: HEAD.into(),
                            pushed: true,
                        }];
                    }
                    "context" => {
                        f.outputs.insert(
                            "context".into(),
                            json!({"merge_base": "m0", "head": HEAD, "affected": ["app"], "changed_files": ["src/main.rs"]}),
                        );
                    }
                    "review" => {
                        f.result = Some(
                            json!({"verdict": "approve", "must_fix": [], "body": "LGTM", "checks": []}),
                        );
                        f.outputs.insert("review_body".into(), json!("LGTM"));
                    }
                    _ => {}
                }
                tweak(&leaf, &mut f);
                runner.finish(f);
            }
            done = started.len();
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
}

/// `workflows/issue` from the library, end to end: dependencies (none) → start gate (/plan) →
/// triage → plan (/approve) → implement (push) → checks → PR → review (approve) → CI green →
/// merge (button) → done.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn library_issue_workflow_happy_path() {
    let Some(h) = start(true).await else {
        return;
    };
    h.open_issue(7, &[]).await;
    let task = loop {
        if let Some((t, ..)) = h.task_of(7).await {
            break t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let p = h
        .wait(task, "start gate", |p| p["reason"] == "awaiting_start")
        .await;
    assert_eq!(p["needs_you"], true, "{p:#}");
    let facts: Value =
        sqlx::query_scalar("SELECT state->'doc'->'facts' FROM engine.tasks WHERE id = $1")
            .bind(task)
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
    assert_eq!(facts["dependencies"], json!([]), "known empty, not unknown");

    let responder = spawn_responder(&h);

    h.comment("plan-bob", 7, "issue", "/plan", "bob").await;
    h.comment("plan", 7, "issue", "/plan", "alice").await;
    let p = h
        .wait(task, "plan approval", |p| {
            p["reason"] == "awaiting_approval"
        })
        .await;
    assert!(p["actions"].to_string().contains("approve"), "{p:#}");

    // Steer at a wait (no step running): it becomes a comment event on the task, offered to
    // the wait like any comment; nothing is steered and the wait holds (it is not `/approve`).
    sqlx::query("SELECT engine_api.submit($1, $2, 'steer', $3)")
        .bind(task)
        .bind(json!({"login": "alice"}))
        .bind(json!({"body": "use a guard"}))
        .execute(&h.db.pool)
        .await
        .unwrap();
    for _ in 0..100 {
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM engine.events WHERE type = 'ui.steer' AND processed_at IS NULL",
        )
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let steers: i64 = sqlx::query_scalar("SELECT count(*) FROM engine.jobs WHERE kind = 'steer'")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(steers, 0);
    assert!(h.runner.steers().is_empty());
    // The library's approval wait treats a plain comment as feedback: the plan is revised.
    for _ in 0..200 {
        let plans: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM engine.step_runs WHERE task_id = $1 AND node LIKE '%plan/plan'",
        )
        .bind(task)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
        if plans >= 2 && h.projection(task).await["reason"] == "awaiting_approval" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    h.wait(task, "revised plan", |p| p["reason"] == "awaiting_approval")
        .await;

    // CI and mergeability of the head the PR will have.
    h.gh.set_ci(rk(APP), HEAD, json!({"state": "success", "failing": []}));
    h.comment("approve", 7, "issue", "/approve", "alice").await;
    let pr = loop {
        if let Some(pr) = h.gh.change_requests().first().map(|p| p.number) {
            break pr;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    h.gh.set_merge_readiness(
        WorkItemKey::new(rk(APP), pr),
        MergeReadiness {
            head_sha: HEAD.into(),
            base_sha: None,
            mergeable: Some(true),
            mergeable_state: "clean".into(),
            ci: enzarb_connector::CiState::Success,
        },
    );
    // A foreign push to the PR head (same verified head here) routes to the task.
    let p = h
        .wait(task, "ready to merge", |p| p["reason"] == "ready_to_merge")
        .await;
    assert_eq!(p["needs_you"], true, "{p:#}");
    h.comment("merge", pr, "pull_request", "/merge", "alice")
        .await;
    let p = h.wait(task, "done", |p| p["status"] == "ended").await;
    responder.abort();
    assert_eq!(p["end_status"], "done", "{p:#}");
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
    let doc: Value = sqlx::query_scalar("SELECT state->'doc' FROM engine.tasks WHERE id = $1")
        .bind(task)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(doc["instance"]["bot"]["handle"], "enzarb");
    assert_eq!(doc["instance"]["url"], "https://enzarb.test");
    assert_eq!(doc["vars"]["merged"], true, "{:#}", doc["vars"]);
}

fn bot_comments(h: &H, key: &WorkItemKey) -> Vec<String> {
    h.gh.calls()
        .iter()
        .filter_map(|c| match &c.call {
            enzarb_connector::fake::Call::PostComment { item, body, .. } if item == key => {
                Some(body.clone())
            }
            _ => None,
        })
        .collect()
}

async fn pr_prompts(h: &H, node_suffix: &str) -> Vec<String> {
    let mut out = Vec::new();
    for s in h.runner.started() {
        let node: String =
            sqlx::query_scalar("SELECT node FROM engine.step_runs WHERE run_id = $1")
                .bind(&s.run_id)
                .fetch_one(&h.db.pool)
                .await
                .unwrap();
        if node.ends_with(node_suffix)
            && let enzarb_proto::StepExec::Agent { prompt, .. } = &s.exec
        {
            out.push(prompt.clone());
        }
    }
    out
}

/// One command model: a command the wait does not take gets exactly one reply naming the
/// accepted ones; a stale `/approve` is neither feedback nor a round; the address round sees
/// only the new, command-free comments.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn library_commands_are_answered_and_feedback_rounds_see_only_new_comments() {
    let Some(h) = start(true).await else {
        return;
    };
    h.open_issue(7, &[]).await;
    let task = loop {
        if let Some((t, ..)) = h.task_of(7).await {
            break t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    h.wait(task, "start gate", |p| p["reason"] == "awaiting_start")
        .await;
    let responder = spawn_responder(&h);
    let issue = WorkItemKey::new(rk(APP), 7);

    // The projection lists what applies now; at the start gate that includes the defaults.
    let p = h.projection(task).await;
    let names: Vec<String> = p["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["retry", "restart", "plan", "cancel"], "{p:#}");

    // An unknown command: one reply, even when GitHub delivers it twice; a bot's gets none.
    h.comment("odd", 7, "issue", "/frobnicate", "alice").await;
    h.comment("odd", 7, "issue", "/frobnicate", "alice").await;
    h.comment("odd-bot", 7, "issue", "/frobnicate", "enzarb[bot]")
        .await;
    h.comment("chat", 7, "issue", "just a thought", "alice")
        .await;
    for _ in 0..200 {
        if !bot_comments(&h, &issue).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let replies = bot_comments(&h, &issue);
    assert_eq!(replies.len(), 1, "{replies:?}");
    for want in [
        "`/frobnicate` does not apply",
        "`/plan`",
        "`/retry`",
        "`/cancel`",
    ] {
        assert!(replies[0].contains(want), "{want} in {}", replies[0]);
    }

    h.comment("plan", 7, "issue", "/plan", "alice").await;
    h.wait(task, "plan approval", |p| {
        p["reason"] == "awaiting_approval"
    })
    .await;
    // `/retry` at the approval wait plans again (the default route is the phase start).
    h.comment("retry", 7, "issue", "/retry", "alice").await;
    let mut plans = 0;
    for _ in 0..300 {
        plans = sqlx::query_scalar(
            "SELECT count(*) FROM engine.step_runs WHERE task_id = $1 AND node LIKE '%plan/plan'",
        )
        .bind(task)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
        if plans >= 2 && h.projection(task).await["reason"] == "awaiting_approval" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(plans, 2_i64);

    // Approve: the pull request opens, is reviewed and waits for CI (unknown here).
    h.comment("approve", 7, "issue", "/approve", "alice").await;
    let pr = loop {
        if let Some(pr) = h.gh.change_requests().first().map(|p| p.number) {
            break pr;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let pr_key = WorkItemKey::new(rk(APP), pr);
    h.wait(task, "waiting for CI", |p| p["reason"] == "awaiting_ci")
        .await;

    // The pull request thread: an old `/approve` and the plan discussion, then new feedback.
    let before = (chrono::Utc::now() - chrono::Duration::hours(1)).to_rfc3339();
    let after = (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339();
    let said = |id: u64, who: &str, body: &str, at: &str| enzarb_connector::ThreadComment {
        author: enzarb_connector::Actor {
            login: who.into(),
            id: 1,
            is_bot: false,
        },
        comment: enzarb_connector::CommentRef {
            id,
            kind: enzarb_connector::CommentKind::Conversation,
            body: body.into(),
            url: String::new(),
        },
        created_at: Some(at.parse().unwrap()),
    };
    h.gh.add_thread(
        pr_key.clone(),
        enzarb_connector::Thread {
            item: enzarb_connector::ItemRef {
                kind: enzarb_connector::ItemKind::ChangeRequest,
                number: pr,
                url: String::new(),
                title: "fix: no crash".into(),
                body: "Closes #7".into(),
                author: "enzarb[bot]".into(),
                head_ref: Some("enzarb/task".into()),
                base_ref: Some("main".into()),
                head_sha: Some(HEAD.into()),
            },
            comments: vec![
                said(1, "alice", "/approve", &before),
                said(2, "alice", "plan note that was answered long ago", &before),
                said(3, "alice", "please rename the helper", &after),
            ],
        },
    );

    // A stale `/approve` on the PR starts no round and gets a reply.
    h.comment("stale", pr, "pull_request", "/approve", "alice")
        .await;
    for _ in 0..200 {
        if bot_comments(&h, &pr_key)
            .iter()
            .any(|b| b.contains("does not apply"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        bot_comments(&h, &pr_key)
            .iter()
            .any(|b| b.contains("`/approve` does not apply")),
        "{:?}",
        bot_comments(&h, &pr_key)
    );
    assert!(pr_prompts(&h, "address_human").await.is_empty());

    // New feedback starts a round whose prompt has that comment and nothing older or command-like.
    let mut ev = serde_json::json!({
        "type": "comment", "body": "please rename the helper", "repo": APP,
        "actor": {"login": "alice"}, "at": after,
        "subject": {"kind": "pull_request", "number": pr}});
    ev["command"] = Value::Null;
    h.ingest(
        "feedback",
        Some(format!("github:{APP}#{pr}")),
        "comment",
        ev,
    )
    .await;
    for _ in 0..300 {
        if !pr_prompts(&h, "address_human").await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let prompts = pr_prompts(&h, "address_human").await;
    assert_eq!(prompts.len(), 1, "one round");
    responder.abort();
    let prompt = &prompts[0];
    let feedback = prompt
        .split("New feedback since the last round")
        .nth(1)
        .expect("feedback section");
    assert!(feedback.contains("please rename the helper"), "{feedback}");
    assert!(!feedback.contains("/approve"), "{feedback}");
    assert!(
        !feedback.contains("plan note that was answered"),
        "{feedback}"
    );
}

/// A task blocked by another issue is told when that issue's task ends (`dependency_changed`),
/// through the `blocked_by` relation recorded with its `dependencies` fact.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dependency_ending_notifies_the_blocked_task() {
    let Some(h) = start(true).await else {
        return;
    };
    h.gh.set_dependencies(
        WorkItemKey::new(rk(APP), 8),
        vec![WorkItemKey::new(rk(APP), 9)],
    );
    h.open_issue(9, &[]).await;
    h.open_issue(8, &[]).await;
    let (a, b) = loop {
        if let (Some((a, ..)), Some((b, ..))) = (h.task_of(8).await, h.task_of(9).await) {
            break (a, b);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    h.wait(a, "blocked", |p| p["reason"] == "blocked_by_dependencies")
        .await;
    // The wait shows first; the fact refresh (which records the relation) runs as a job.
    let mut rel = 0i64;
    for _ in 0..200 {
        rel = sqlx::query_scalar(
            "SELECT count(*) FROM engine.relations WHERE task_id = $1 AND kind = 'blocked_by'
             AND target_key = $2",
        )
        .bind(a)
        .bind(format!("github:{APP}#9"))
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
        if rel > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(rel, 1, "the dependency is recorded as a relation");
    // End the dependency's task.
    sqlx::query("SELECT engine_api.submit($1, '{\"login\": \"alice\"}', 'cancel', '{}')")
        .bind(b)
        .execute(&h.db.pool)
        .await
        .unwrap();
    let mut notified = 0i64;
    for _ in 0..200 {
        notified = sqlx::query_scalar(
            "SELECT count(*) FROM engine.events WHERE task_id = $1 AND type = 'dependency_changed'",
        )
        .bind(a)
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
        if notified > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        notified > 0,
        "the blocked task was not told that its dependency ended"
    );
}

// ---------------------------------------------------------------- dead-wait checks

/// Every shipped check parses, validates and files an issue.
#[test]
fn stdlib_checks_are_valid_and_file_issues() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/stdlib/checks");
    let mut ids = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let text = std::fs::read_to_string(&p).unwrap();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        let c: enzarb_flow::format::CheckFile = enzarb_flow::yaml::SourceFile::parse(&name, &text)
            .unwrap_or_else(|d| panic!("{name}: {}", d.message))
            .typed()
            .unwrap_or_else(|d| panic!("{name}: {}", d.message));
        assert!(c.problems().is_empty(), "{name}: {:?}", c.problems());
        assert_eq!(format!("{}.yaml", c.id), name);
        assert!(c.issue.is_some(), "{name} files no issue");
        ids.push(c.id);
    }
    ids.sort();
    assert_eq!(
        ids,
        [
            "chart-not-deployed",
            "ci-never-started",
            "epic-subissues-without-deps",
            "event-failing",
            "external-idle",
            "mergeable-unknown",
            "platform-wait-long",
            "rebuild-all-failed",
            "release-failed",
            "retry-ignored",
            "sandbox-change-unapplied",
            "sandbox-disk-full",
            "sandbox-disk-high",
            "sandbox-memory-high"
        ]
    );
}

/// Seeds an open task at `node` (`phase/..`) waiting on `waiting_on`, created two hours ago with
/// no event since. `subject` is the issue it owns, when it has one.
async fn seed_task(h: &H, node: &str, waiting_on: &str, subject: Option<u64>) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO engine.tasks (project, repo, workflow_ref, state, resolved, created_at)
         VALUES ('p', $1, 'wf', '{}', '{}', now() - interval '2 hours') RETURNING id",
    )
    .bind(APP)
    .fetch_one(&h.db.pool)
    .await
    .unwrap();
    let phase = node.split('/').next().unwrap();
    sqlx::query(
        "INSERT INTO engine.projection (task_id, project, repo, workflow, status, phase, node, waiting_on)
         VALUES ($1, 'p', $2, 'wf', 'waiting', $3, $4, $5)",
    )
    .bind(id)
    .bind(APP)
    .bind(phase)
    .bind(node)
    .bind(waiting_on)
    .execute(&h.db.pool)
    .await
    .unwrap();
    if let Some(n) = subject {
        sqlx::query("INSERT INTO engine.subjects (task_id, kind, key) VALUES ($1, 'issue', $2)")
            .bind(id)
            .bind(format!("github:{APP}#{n}"))
            .execute(&h.db.pool)
            .await
            .unwrap();
    }
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

/// A command comment routed to the task `minutes` ago.
async fn command_event(h: &H, task: i64, command: &str, minutes: i32) {
    sqlx::query(
        "INSERT INTO engine.events (dedupe_key, project, task_id, type, payload, received_at, processed_at)
         VALUES ($1, 'p', $2, 'comment', $3, now() - make_interval(mins => $4), now())",
    )
    .bind(format!("cmd-{task}-{minutes}"))
    .bind(task)
    .bind(json!({"type": "comment", "body": format!("/{command}"), "command": command,
                 "repo": APP, "actor": {"login": "alice"}}))
    .bind(minutes)
    .execute(&h.db.pool)
    .await
    .unwrap();
}

/// A transition `minutes` ago: `to_node` differs from the old node for a real one; an empty
/// effect list with the same node is what an ignored event leaves behind.
async fn transition(h: &H, task: i64, from_node: &str, to_node: &str, minutes: i32) {
    sqlx::query(
        "INSERT INTO engine.transitions (task_id, from_state, to_state, effects, at)
         VALUES ($1, $2, $3, '[]', now() - make_interval(mins => $4))",
    )
    .bind(task)
    .bind(json!({"status": "waiting", "node": from_node, "pending": null}))
    .bind(json!({"status": "waiting", "node": to_node, "pending": null}))
    .bind(minutes)
    .execute(&h.db.pool)
    .await
    .unwrap();
}

async fn passes(h: &H, n: usize) {
    for _ in 0..n {
        h.engine.findings_pass().await.unwrap();
    }
}

/// Runs every queued job to completion.
async fn run_jobs(h: &H) {
    while let Some(job) = h.engine.claim().await.unwrap() {
        h.engine.execute(job).await;
    }
}

async fn open_checks(h: &H) -> Vec<String> {
    sqlx::query_scalar("SELECT check_id FROM engine_api.finding_v ORDER BY key")
        .fetch_all(&h.db.pool)
        .await
        .unwrap()
}

/// `(idempotency key, outcome)` of every `file_issue` job.
async fn issue_jobs(h: &H) -> Vec<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT idempotency_key, output->>'outcome' FROM engine.jobs WHERE kind = 'file_issue'
         ORDER BY id",
    )
    .fetch_all(&h.db.pool)
    .await
    .unwrap()
}

/// The incident's whole life: nothing on the first pass, exactly one finding and one issue once
/// it persists, nothing more however often the passes repeat, and no second issue when the
/// finding clears (`fix`) and comes back (`restore`).
async fn assert_one_finding_one_issue<F1, F2>(
    h: &H,
    check: &str,
    key: &str,
    title: &str,
    fix: F1,
    restore: F2,
) where
    F1: std::future::Future<Output = ()>,
    F2: std::future::Future<Output = ()>,
{
    passes(h, 1).await;
    assert!(open_checks(h).await.is_empty(), "raised on the first pass");
    passes(h, 1).await;
    assert_eq!(open_checks(h).await, [check]);
    run_jobs(h).await;
    assert_eq!(
        issue_jobs(h).await,
        [(key.to_string(), Some("created".into()))]
    );
    let made = h.gh.created_issues();
    assert_eq!(made.len(), 1, "{made:?}");
    assert_eq!(made[0].1, title);
    assert!(made[0].2.contains("Nothing was cancelled or restarted"));
    // Repeating adds nothing.
    passes(h, 4).await;
    run_jobs(h).await;
    assert_eq!(open_checks(h).await, [check]);
    assert_eq!(issue_jobs(h).await.len(), 1);
    assert_eq!(h.gh.created_issues().len(), 1);
    // Clear, then come back: the same finding row, the same issue.
    fix.await;
    passes(h, 1).await;
    assert!(open_checks(h).await.is_empty());
    restore.await;
    passes(h, 2).await;
    run_jobs(h).await;
    assert_eq!(open_checks(h).await, [check]);
    assert_eq!(issue_jobs(h).await.len(), 1);
    assert_eq!(h.gh.created_issues().len(), 1);
    // The task itself was never touched.
    let jobs: i64 =
        sqlx::query_scalar("SELECT count(*) FROM engine.jobs WHERE kind <> 'file_issue'")
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
    assert_eq!(jobs, 0);
}

/// A conflicting pull request never starts CI and `mergeable` stays unknown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_pr_without_check_runs_files_one_issue() {
    let Some(h) = start(false).await else {
        return;
    };
    let id = seed_task(&h, "review/rounds/checks", "external", None).await;
    set_fact(&h, id, "ci", json!({"sha": "abc", "check_runs": []}), 0).await;
    set_fact(&h, id, "mergeable", Value::Null, 1).await;
    // The check runs arrive and the pull request is mergeable.
    let healthy = async {
        set_fact(&h, id, "ci", json!({"check_runs": [{"name": "test"}]}), 0).await;
        set_fact(&h, id, "mergeable", json!(true), 0).await;
    };
    let broken = async {
        set_fact(&h, id, "ci", json!({"check_runs": []}), 0).await;
        set_fact(&h, id, "mergeable", json!(false), 0).await;
    };
    assert_one_finding_one_issue(
        &h,
        "ci-never-started",
        &format!("finding:ci-never-started:{id}"),
        &format!("Task {id} waits for CI that cannot start"),
        healthy,
        broken,
    )
    .await;
    // The issue goes to the task's repo, with the check's labels.
    let (repo, labels): (String, Value) = sqlx::query_as(
        "SELECT input->>'repo', input->'labels' FROM engine.jobs WHERE kind = 'file_issue'",
    )
    .fetch_one(&h.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (repo.as_str(), labels),
        (APP, json!(["liveness", "dead-wait"]))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthy_ci_files_nothing() {
    let Some(h) = start(false).await else {
        return;
    };
    let id = seed_task(&h, "review/rounds/checks", "external", None).await;
    set_fact(&h, id, "ci", json!({"check_runs": [{"name": "test"}]}), 0).await;
    set_fact(&h, id, "mergeable", json!(true), 0).await;
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert!(open_checks(&h).await.is_empty());
    assert!(issue_jobs(&h).await.is_empty());
    assert!(h.gh.created_issues().is_empty());
}

/// `/retry` reaches a node that does not act on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ignored_retry_files_one_issue() {
    let Some(h) = start(false).await else {
        return;
    };
    let node = "review/rounds/checks";
    let id = seed_task(&h, node, "human", None).await;
    command_event(&h, id, "retry", 20).await;
    // The event was processed and left a transition without a change and without effects.
    transition(&h, id, node, node, 19).await;
    assert_one_finding_one_issue(
        &h,
        "retry-ignored",
        &format!("finding:retry-ignored:{id}"),
        &format!("Task {id} ignored a command at {node}"),
        // The task moved on.
        transition(&h, id, node, "review/rounds/done", 18),
        // A newer command that is ignored again.
        command_event(&h, id, "retry", 16),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retry_that_advanced_or_is_recent_files_nothing() {
    let Some(h) = start(false).await else {
        return;
    };
    let node = "review/rounds/checks";
    // The retry restarted the node.
    let moved = seed_task(&h, node, "human", None).await;
    command_event(&h, moved, "retry", 20).await;
    transition(&h, moved, node, "review/rounds/context", 19).await;
    // A retry that only produced effects (answered).
    let answered = seed_task(&h, node, "human", None).await;
    command_event(&h, answered, "retry", 20).await;
    sqlx::query(
        "INSERT INTO engine.transitions (task_id, from_state, to_state, effects, at)
         VALUES ($1, '{}', '{}', '[{\"kind\": \"call_action\"}]', now() - interval '19 minutes')",
    )
    .bind(answered)
    .execute(&h.db.pool)
    .await
    .unwrap();
    // Too recent to tell.
    let recent = seed_task(&h, node, "human", None).await;
    command_event(&h, recent, "retry", 2).await;
    // Another node, however old the command.
    let elsewhere = seed_task(&h, "review/rounds/human", "human", None).await;
    command_event(&h, elsewhere, "retry", 20).await;
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert!(open_checks(&h).await.is_empty());
    assert!(issue_jobs(&h).await.is_empty());
}

/// Sub-issues an epic created without dependencies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn epic_subissue_without_dependencies_files_one_issue() {
    let Some(h) = start(false).await else {
        return;
    };
    let epic = seed_task(&h, "breakdown/wait", "human", Some(1)).await;
    let sub = seed_task(&h, "start/wait", "human", Some(2)).await;
    // The breakdown related the epic as `parent` of the sub-issue.
    sqlx::query(
        "INSERT INTO engine.relations (task_id, kind, target_key, source)
         VALUES ($1, 'parent', $2, 'workflow')",
    )
    .bind(epic)
    .bind(format!("github:{APP}#2"))
    .execute(&h.db.pool)
    .await
    .unwrap();
    let depend = async {
        sqlx::query(
            "INSERT INTO engine.relations (task_id, kind, target_key, source)
             VALUES ($1, 'blocked_by', $2, 'dependencies')",
        )
        .bind(sub)
        .bind(format!("github:{APP}#3"))
        .execute(&h.db.pool)
        .await
        .unwrap();
    };
    let undepend = async {
        sqlx::query("DELETE FROM engine.relations WHERE task_id = $1 AND kind = 'blocked_by'")
            .bind(sub)
            .execute(&h.db.pool)
            .await
            .unwrap();
    };
    assert_one_finding_one_issue(
        &h,
        "epic-subissues-without-deps",
        &format!("finding:epic-subissues-without-deps:{sub}"),
        &format!("Sub-issue task {sub} was created without dependencies"),
        depend,
        undepend,
    )
    .await;
    // The epic itself, and an ordinary issue, never match.
    let tasks: Vec<Option<i64>> = sqlx::query_scalar("SELECT task_id FROM engine.findings")
        .fetch_all(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(tasks, [Some(sub)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subissues_with_dependencies_and_plain_issues_file_nothing() {
    let Some(h) = start(false).await else {
        return;
    };
    let epic = seed_task(&h, "breakdown/wait", "human", Some(1)).await;
    let a = seed_task(&h, "start/wait", "human", Some(2)).await;
    let b = seed_task(&h, "start/wait", "human", Some(3)).await;
    let plain = seed_task(&h, "start/wait", "human", Some(4)).await;
    for n in [2, 3] {
        sqlx::query(
            "INSERT INTO engine.relations (task_id, kind, target_key) VALUES ($1, 'parent', $2)",
        )
        .bind(epic)
        .bind(format!("github:{APP}#{n}"))
        .execute(&h.db.pool)
        .await
        .unwrap();
    }
    // Both sub-issues have their dependencies recorded; the plain issue is no sub-issue.
    sqlx::query(
        "INSERT INTO engine.relations (task_id, kind, target_key) VALUES ($1, 'blocked_by', $2)",
    )
    .bind(a)
    .bind(format!("github:{APP}#3"))
    .execute(&h.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO engine.relations (task_id, kind, target_key) VALUES ($1, 'blocked_by', $2)",
    )
    .bind(b)
    .bind(format!("github:{APP}#4"))
    .execute(&h.db.pool)
    .await
    .unwrap();
    assert!(plain > 0);
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert!(open_checks(&h).await.is_empty());
    assert!(issue_jobs(&h).await.is_empty());
}

/// An external wait with no event and no step for a day; a younger one is fine.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_external_wait_files_one_issue() {
    let Some(h) = start(false).await else {
        return;
    };
    let idle = seed_task(&h, "merge/wait", "external", None).await;
    sqlx::query("UPDATE engine.tasks SET created_at = now() - interval '2 days' WHERE id = $1")
        .bind(idle)
        .execute(&h.db.pool)
        .await
        .unwrap();
    let young = seed_task(&h, "merge/wait", "external", None).await;
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert_eq!(open_checks(&h).await, ["external-idle"]);
    let jobs = issue_jobs(&h).await;
    assert_eq!(
        jobs,
        [(
            format!("finding:external-idle:{idle}"),
            Some("created".into())
        )]
    );
    assert_ne!(young, idle);
}

/// Sets the body of the task's source issue, as the reducer stores it.
async fn set_issue_body(h: &H, task: i64, body: &str) {
    sqlx::query("UPDATE engine.tasks SET state = $2 WHERE id = $1")
        .bind(task)
        .bind(json!({"doc": {"subjects": {"issue": {"body": body}}}}))
        .execute(&h.db.pool)
        .await
        .unwrap();
}

async fn age_task(h: &H, task: i64) {
    sqlx::query("UPDATE engine.tasks SET created_at = now() - interval '2 days' WHERE id = $1")
        .bind(task)
        .execute(&h.db.pool)
        .await
        .unwrap();
}

/// The task born from a finding's issue gets its own finding, but no second issue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finding_issue_does_not_file_another() {
    let Some(h) = start(false).await else {
        return;
    };
    let idle = seed_task(&h, "merge/wait", "external", None).await;
    age_task(&h, idle).await;
    passes(&h, 4).await;
    run_jobs(&h).await;
    let first = format!("finding:external-idle:{idle}");
    assert_eq!(issue_jobs(&h).await.len(), 1);
    let child = seed_task(&h, "merge/wait", "external", None).await;
    age_task(&h, child).await;
    let marker = enzarb_connector::file_issue::file_issue_key(APP, &first);
    set_issue_body(
        &h,
        child,
        &format!("Idle.\n\n<!-- enzarb:idem:{marker} -->"),
    )
    .await;
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert_eq!(open_checks(&h).await, ["external-idle", "external-idle"]);
    assert_eq!(
        issue_jobs(&h).await,
        [(first, Some("created".into()))],
        "no issue for task {child}"
    );
}

/// Control: an issue without the `file_issue` marker (none, or an epic's) still files.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issue_not_from_a_finding_still_files() {
    let Some(h) = start(false).await else {
        return;
    };
    let a = seed_task(&h, "merge/wait", "external", None).await;
    let b = seed_task(&h, "merge/wait", "external", None).await;
    let c = seed_task(&h, "merge/wait", "external", None).await;
    for t in [a, b, c] {
        age_task(&h, t).await;
    }
    set_issue_body(&h, b, "A plain issue").await;
    set_issue_body(&h, c, "<!-- enzarb:idem:epic-1-a -->").await;
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert_eq!(issue_jobs(&h).await.len(), 3);
}

/// A fact that stays unknown across refreshes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fact_unknown_across_refreshes_files_one_issue() {
    let Some(h) = start(false).await else {
        return;
    };
    let id = seed_task(&h, "merge/wait", "human", None).await;
    set_fact(&h, id, "mergeable", Value::Null, 2).await;
    passes(&h, 3).await;
    assert!(
        open_checks(&h).await.is_empty(),
        "two refreshes are not enough"
    );
    set_fact(&h, id, "mergeable", Value::Null, 3).await;
    passes(&h, 3).await;
    run_jobs(&h).await;
    assert_eq!(open_checks(&h).await, ["mergeable-unknown"]);
    assert_eq!(
        issue_jobs(&h).await,
        [(
            format!("finding:mergeable-unknown:{id}"),
            Some("created".into())
        )]
    );
}

// ---------------------------------------------------------------- platform checks

use engine::findings::{DbPlatformFacts, FakePlatformFacts};
use enzarb_flow::format::PlatformFactName as F;

const RELEASE: &str = "enzarb/enzarb:release.yml";
const REBUILD: &str = "enzarb/enzarb:rebuild-all.yml";

/// A harness whose platform facts are `facts` (all unknown until set).
async fn platform_harness() -> Option<(H, FakePlatformFacts)> {
    let mut h = start(false).await?;
    let facts = FakePlatformFacts::new();
    h.engine.findings = engine::Findings::new(Arc::new(facts.clone()));
    Some((h, facts))
}

fn text(s: &str) -> Fact {
    Fact::Text(s.into())
}

/// Checks only read: besides library reads (recorded as generic calls), the one write the whole
/// run made was the issue upsert.
fn assert_observation_only(h: &H) {
    use enzarb_connector::fake::Call;
    for c in h.gh.applied_calls() {
        assert!(
            matches!(
                c,
                Call::UpsertIssue { .. } | Call::Generic { .. } | Call::GetFile { .. }
            ),
            "{c:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_release_run_files_one_issue() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set_arg("p", F::WorkflowRunStatus, RELEASE, text("failure"));
    f.set_arg("p", F::WorkflowRunStatus, REBUILD, text("success"));
    let (f1, f2) = (f.clone(), f.clone());
    assert_one_finding_one_issue(
        &h,
        "release-failed",
        "finding:release-failed:p",
        "The release workflow failed on main",
        async move { f1.set_arg("p", F::WorkflowRunStatus, RELEASE, text("success")) },
        async move { f2.set_arg("p", F::WorkflowRunStatus, RELEASE, text("failure")) },
    )
    .await;
    let repo: String =
        sqlx::query_scalar("SELECT input->>'repo' FROM engine.jobs WHERE kind = 'file_issue'")
            .fetch_one(&h.db.pool)
            .await
            .unwrap();
    assert_eq!(repo, "enzarb/enzarb");
    assert_observation_only(&h);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_rebuild_all_run_files_one_issue() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set_arg("p", F::WorkflowRunStatus, RELEASE, text("success"));
    f.set_arg("p", F::WorkflowRunStatus, REBUILD, text("failure"));
    let (f1, f2) = (f.clone(), f.clone());
    assert_one_finding_one_issue(
        &h,
        "rebuild-all-failed",
        "finding:rebuild-all-failed:p",
        "The rebuild-all workflow failed on main",
        async move { f1.set_arg("p", F::WorkflowRunStatus, REBUILD, text("success")) },
        async move { f2.set_arg("p", F::WorkflowRunStatus, REBUILD, text("failure")) },
    )
    .await;
    assert_observation_only(&h);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_sandbox_workspace_files_one_critical_issue() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set("p", F::SandboxDiskPct, Fact::Num(100.0));
    let (f1, f2) = (f.clone(), f.clone());
    assert_one_finding_one_issue(
        &h,
        "sandbox-disk-full",
        "finding:sandbox-disk-full:p",
        "Sandbox of project p has a full workspace",
        async move { f1.set("p", F::SandboxDiskPct, Fact::Num(40.0)) },
        async move { f2.set("p", F::SandboxDiskPct, Fact::Num(100.0)) },
    )
    .await;
    let sev: String = sqlx::query_scalar("SELECT severity FROM engine_api.finding_v")
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
    assert_eq!(sev, "critical");
    assert_observation_only(&h);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nearly_full_workspace_and_high_memory_are_separate_warnings() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set("p", F::SandboxDiskPct, Fact::Num(93.0));
    f.set("p", F::SandboxMemWorkingSetPct, Fact::Num(95.0));
    passes(&h, 2).await;
    run_jobs(&h).await;
    assert_eq!(
        open_checks(&h).await,
        ["sandbox-disk-high", "sandbox-memory-high"]
    );
    assert_eq!(h.gh.created_issues().len(), 2);
    // At 100% only the full check holds; the high check clears.
    f.set("p", F::SandboxDiskPct, Fact::Num(100.0));
    passes(&h, 2).await;
    assert_eq!(
        open_checks(&h).await,
        ["sandbox-disk-full", "sandbox-memory-high"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undeployed_chart_files_one_issue() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set("p", F::ChartDeployState, text("stuck"));
    let (f1, f2) = (f.clone(), f.clone());
    assert_one_finding_one_issue(
        &h,
        "chart-not-deployed",
        "finding:chart-not-deployed:p",
        "A released chart version did not deploy",
        async move { f1.set("p", F::ChartDeployState, text("deployed")) },
        async move { f2.set("p", F::ChartDeployState, text("stuck")) },
    )
    .await;
    assert_observation_only(&h);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unapplied_sandbox_change_files_one_issue() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set("p", F::ProjectPendingChange, Fact::Bool(true));
    let (f1, f2) = (f.clone(), f.clone());
    assert_one_finding_one_issue(
        &h,
        "sandbox-change-unapplied",
        "finding:sandbox-change-unapplied:p",
        "Sandbox of project p has not applied a pending change",
        async move { f1.set("p", F::ProjectPendingChange, Fact::Bool(false)) },
        async move { f2.set("p", F::ProjectPendingChange, Fact::Bool(true)) },
    )
    .await;
    assert_observation_only(&h);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthy_platform_facts_file_nothing() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    f.set_arg("p", F::WorkflowRunStatus, RELEASE, text("success"));
    f.set_arg("p", F::WorkflowRunStatus, REBUILD, text("success"));
    // A rollout in progress is normal: `pending` files nothing.
    f.set("p", F::ChartDeployState, text("pending"));
    f.set("p", F::SandboxDiskPct, Fact::Num(35.0));
    f.set("p", F::SandboxMemWorkingSetPct, Fact::Num(50.0));
    f.set("p", F::ProjectPendingChange, Fact::Bool(false));
    passes(&h, 4).await;
    run_jobs(&h).await;
    assert!(open_checks(&h).await.is_empty());
    assert!(issue_jobs(&h).await.is_empty());
    assert!(h.gh.created_issues().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_platform_facts_leave_findings_alone_and_file_nothing() {
    let Some((h, f)) = platform_harness().await else {
        return;
    };
    // Nothing is known: nothing is raised.
    passes(&h, 3).await;
    assert!(open_checks(&h).await.is_empty());
    // An open finding stays open while its fact is unknown, and files nothing more.
    f.set("p", F::ChartDeployState, text("stuck"));
    passes(&h, 2).await;
    run_jobs(&h).await;
    assert_eq!(open_checks(&h).await, ["chart-not-deployed"]);
    f.set("p", F::ChartDeployState, Fact::Unknown);
    passes(&h, 3).await;
    run_jobs(&h).await;
    assert_eq!(open_checks(&h).await, ["chart-not-deployed"]);
    assert_eq!(h.gh.created_issues().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_facts_read_the_workflow_run_and_the_memory_percentage() {
    let Some(h) = start(false).await else {
        return;
    };
    // The run is read from the repository's default branch, not a hard-coded `main`.
    h.gh.set_default_branch(rk("enzarb/enzarb"), "trunk");
    h.gh.set_workflow_run(
        rk("enzarb/enzarb"),
        "release.yml",
        "trunk",
        enzarb_connector::WorkflowRunInfo {
            id: 5,
            conclusion: "failure".into(),
            url: None,
        },
    );
    let facts = DbPlatformFacts::new(h.db.pool.clone(), Arc::new(StaticSandboxResolver::new()))
        .with_connector(h.gh.clone());
    use engine::findings::PlatformFacts;
    assert_eq!(
        facts.get_arg("p", F::WorkflowRunStatus, RELEASE).await,
        text("failure")
    );
    // No run, an unsupported workflow and a malformed argument are unknown.
    assert_eq!(
        facts.get_arg("p", F::WorkflowRunStatus, REBUILD).await,
        Fact::Unknown
    );
    assert_eq!(
        facts.get_arg("p", F::WorkflowRunStatus, "nonsense").await,
        Fact::Unknown
    );
    // One connector read per workflow however often (or for however many Projects) it is asked.
    facts.get_arg("q", F::WorkflowRunStatus, RELEASE).await;
    assert_eq!(h.gh.generic_calls("latest_workflow_run").len(), 2);

    let stat = |ts_ms, mem, limit| enzarb_proto::SandboxStats {
        ts_ms,
        memory_bytes: mem,
        memory_limit_bytes: limit,
        ..Default::default()
    };
    let now = chrono::Utc::now().timestamp_millis() as u64;
    // Unknown without a limit (older sandbox, or `max`), and with a zero limit.
    engine::db::insert_sandbox_stats(&h.db.pool, "p", "pod", &stat(now - 3000, Some(50), None))
        .await
        .unwrap();
    assert_eq!(
        facts.get("p", F::SandboxMemWorkingSetPct).await,
        Fact::Unknown
    );
    engine::db::insert_sandbox_stats(&h.db.pool, "p", "pod", &stat(now - 2000, Some(50), Some(0)))
        .await
        .unwrap();
    assert_eq!(
        facts.get("p", F::SandboxMemWorkingSetPct).await,
        Fact::Unknown
    );
    engine::db::insert_sandbox_stats(
        &h.db.pool,
        "p",
        "pod",
        &stat(now - 1000, Some(900), Some(1000)),
    )
    .await
    .unwrap();
    assert_eq!(
        facts.get("p", F::SandboxMemWorkingSetPct).await,
        Fact::Num(90.0)
    );
    // Page cache (`inactive_file`) is not counted: 900 used, 800 of it reclaimable, of 1000.
    engine::db::insert_sandbox_stats(
        &h.db.pool,
        "p",
        "pod",
        &enzarb_proto::SandboxStats {
            ts_ms: now,
            memory_bytes: Some(900),
            memory_limit_bytes: Some(1000),
            memory_inactive_file_bytes: Some(800),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        facts.get("p", F::SandboxMemWorkingSetPct).await,
        Fact::Num(10.0)
    );
}

// ------------------------------------------------ environment failures (ADR-0026, issue #300)

/// One way to reach a library agent step with the issue workflow.
#[derive(Clone, Copy)]
struct Reach {
    /// The step's node name (`implement`, `review`, `address`, `ci_fix`, `resolve`).
    leaf: &'static str,
    /// The review asks for changes (reaches `address`).
    must_fix: bool,
    /// CI is red on the head (reaches `ci_fix`).
    ci_failing: bool,
    /// The head conflicts with the base (reaches `resolve`).
    conflict: bool,
    /// Where a failure with no `environment` ends up (reason of the wait).
    code_reason: &'static str,
}

const REACH: [Reach; 5] = [
    Reach {
        leaf: "implement",
        must_fix: false,
        ci_failing: false,
        conflict: false,
        code_reason: "implement_blocked",
    },
    Reach {
        leaf: "review",
        must_fix: false,
        ci_failing: false,
        conflict: false,
        code_reason: "awaiting_review",
    },
    Reach {
        leaf: "address",
        must_fix: true,
        ci_failing: false,
        conflict: false,
        code_reason: "awaiting_review",
    },
    Reach {
        leaf: "ci_fix",
        must_fix: false,
        ci_failing: true,
        conflict: false,
        code_reason: "ci_failed",
    },
    Reach {
        leaf: "resolve",
        must_fix: false,
        ci_failing: false,
        conflict: true,
        code_reason: "conflict_parked",
    },
];

fn failure_of(kind: Option<enzarb_proto::EnvironmentKind>, f: &mut StepFinished) {
    f.exit = "failure".into();
    f.result = None;
    f.environment = kind.map(|kind| enzarb_proto::StepEnvironment {
        kind,
        detail: "test".into(),
    });
}

/// Runs the issue workflow until the agent step `r.leaf` fails with `kind` (no `environment`
/// for a code failure). Returns the task and the run id of the failed step.
async fn fail_step(h: &H, r: Reach, kind: Option<enzarb_proto::EnvironmentKind>) -> (i64, String) {
    h.open_issue(7, &[]).await;
    let task = loop {
        if let Some((t, ..)) = h.task_of(7).await {
            break t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    h.wait(task, "start gate", |p| p["reason"] == "awaiting_start")
        .await;
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _responder = spawn_responder_with(h, move |leaf, f| {
        if leaf == "review" && r.must_fix && f.exit == "success" {
            f.result = Some(json!({"verdict": "request_changes", "must_fix": ["a bug"],
                                   "body": "Fix it", "checks": []}));
        }
        if leaf == r.leaf {
            let n = hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            // An environment failure happens once (the resume succeeds); a code failure
            // repeats, as `resolve` retries until its attempts run out.
            if kind.is_none() || n == 0 {
                failure_of(kind, f);
            }
        }
    });
    h.comment("plan", 7, "issue", "/plan", "alice").await;
    h.wait(task, "plan approval", |p| {
        p["reason"] == "awaiting_approval"
    })
    .await;
    let ci = if r.ci_failing {
        json!({"state": "failure", "failing": [{"name": "build",
               "url": format!("https://github.com/{APP}/actions/runs/1")}]})
    } else {
        json!({"state": "success", "failing": []})
    };
    h.gh.set_ci(rk(APP), HEAD, ci);
    h.comment("approve", 7, "issue", "/approve", "alice").await;
    if r.leaf != "implement" {
        let pr = loop {
            if let Some(pr) = h.gh.change_requests().first().map(|p| p.number) {
                break pr;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        h.gh.set_merge_readiness(
            WorkItemKey::new(rk(APP), pr),
            MergeReadiness {
                head_sha: HEAD.into(),
                base_sha: None,
                mergeable: Some(!r.conflict),
                mergeable_state: if r.conflict { "dirty" } else { "clean" }.into(),
                ci: if r.ci_failing {
                    enzarb_connector::CiState::Failure
                } else {
                    enzarb_connector::CiState::Success
                },
            },
        );
    }
    let run_id = loop {
        let run: Option<String> = sqlx::query_scalar(
            "SELECT run_id FROM engine.step_runs WHERE task_id = $1 AND node LIKE $2
             AND ended_at IS NOT NULL ORDER BY started_at LIMIT 1",
        )
        .bind(task)
        .bind(format!("%/{}", r.leaf))
        .fetch_optional(&h.db.pool)
        .await
        .unwrap();
        if let Some(run) = run {
            break run;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    (task, run_id)
}

async fn vars_of(h: &H, task: i64) -> Value {
    sqlx::query_scalar("SELECT state->'doc'->'vars' FROM engine.tasks WHERE id = $1")
        .bind(task)
        .fetch_one(&h.db.pool)
        .await
        .unwrap()
}

/// The environment failure of every agent step on the review and implement paths goes to the
/// platform wait: no fix round, no `human`, no `blocked`; the node, `fix_rounds` and
/// `reviewed_sha` stay as they were. When the condition clears, the same run resumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn library_environment_failure_waits_on_the_review_and_implement_paths() {
    use enzarb_proto::EnvironmentKind::{DiskFull, Oom};
    for (r, kind) in REACH
        .into_iter()
        .zip([DiskFull, Oom, DiskFull, Oom, DiskFull])
    {
        let Some(mut h) = start(true).await else {
            return;
        };
        let facts = engine::FakePlatformFacts::new();
        h.engine.findings = engine::Findings::new(Arc::new(facts.clone()));
        let (task, run_id) = fail_step(&h, r, Some(kind)).await;
        let p = h
            .wait(task, r.leaf, |p| p["reason"] == "platform_wait")
            .await;
        let leaf = r.leaf;
        assert_eq!(p["node"], leaf, "node did not move: {p:#}");
        assert_eq!(p["status"], "running", "{leaf}: {p:#}");
        assert_eq!(p["waiting_on"], "external", "{leaf}: {p:#}");
        assert_eq!(p["needs_you"], false, "{leaf}: {p:#}");
        let vars = vars_of(&h, task).await;
        assert_eq!(
            vars["fix_rounds"].as_i64().unwrap_or(0),
            0,
            "{leaf}: {vars:#}"
        );
        assert!(
            vars["reviewed_sha"].is_null() || leaf != "review",
            "{leaf}: {vars:#}"
        );
        let runs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM engine.step_runs WHERE task_id = $1 AND node LIKE $2",
        )
        .bind(task)
        .bind(format!("%/{leaf}"))
        .fetch_one(&h.db.pool)
        .await
        .unwrap();
        assert_eq!(runs, 1, "{leaf}: no new run while waiting");

        // The condition clears: the same run resumes and the flow moves on.
        let fact = match kind {
            DiskFull => PlatformFactName::SandboxDiskPct,
            _ => PlatformFactName::SandboxHealthy,
        };
        facts.set(
            "p",
            fact,
            match kind {
                DiskFull => Fact::Num(40.0),
                _ => Fact::Bool(true),
            },
        );
        h.engine.wake_platform_waits().await.unwrap();
        h.engine.wake_platform_waits().await.unwrap();
        for _ in 0..200 {
            let again = h
                .runner
                .started()
                .iter()
                .filter(|s| s.run_id == run_id)
                .count();
            if again == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let resumed = h
            .runner
            .started()
            .iter()
            .filter(|s| s.run_id == run_id)
            .count();
        assert_eq!(resumed, 2, "{leaf}: same run, started again once");
        h.wait(task, leaf, |p| p["reason"] != "platform_wait").await;
    }
}

/// A failure with no `environment` keeps the routes of the library: `human`, `blocked`,
/// `ci_failed`, `conflict_parked`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn library_code_failure_keeps_its_routes() {
    for r in REACH {
        let Some(h) = start(true).await else {
            return;
        };
        let (task, _) = fail_step(&h, r, None).await;
        let p = h.wait(task, r.leaf, |p| p["reason"] == r.code_reason).await;
        assert_ne!(p["reason"], "platform_wait", "{p:#}");
        assert_eq!(p["needs_you"], true, "{}: {p:#}", r.leaf);
    }
}

/// `toolchain_missing`, `network`, `load` and `unknown` do not wait: they follow the edges.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn library_other_environment_kinds_keep_their_routes() {
    use enzarb_proto::EnvironmentKind::{Load, Network, ToolchainMissing, Unknown};
    for kind in [ToolchainMissing, Network, Load, Unknown] {
        for r in [REACH[0], REACH[1]] {
            let Some(h) = start(true).await else {
                return;
            };
            let (task, _) = fail_step(&h, r, Some(kind)).await;
            let p = h.wait(task, r.leaf, |p| p["reason"] == r.code_reason).await;
            assert_eq!(p["needs_you"], true, "{}: {p:#}", r.leaf);
        }
    }
}
