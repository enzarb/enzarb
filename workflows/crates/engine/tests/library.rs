//! End-to-end tests against the real standard library (snapshot in `tests/stdlib`, served as
//! `enzarb/workflows@v1` by the fake connector): binding routing (`if`/`with`, relations,
//! `mentioned`) and the whole `workflows/issue` happy path with fake sandbox results.

use std::sync::Arc;
use std::time::Duration;

use engine::testing::TestDb;
use engine::{
    Engine, EngineConfig, FakeStepRunner, Library, ProjectInfo, StaticPermissions,
    StaticProjectIndex, StaticSandboxResolver,
};
use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{MergeReadiness, Permission, RepoKey, WorkItemKey};
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
        error: None,
    }
}

/// Fake sandbox: answers each step by its node.
fn spawn_responder(h: &H) -> tokio::task::JoinHandle<()> {
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
