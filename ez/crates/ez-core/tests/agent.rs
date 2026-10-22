use async_trait::async_trait;
use ez_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
    ToolStatus,
};
use ez_core::transcript::{PermissionOutcome, Record, TranscriptReader, UserSource};
use ez_llm::mock::MockProvider;
use ez_llm::{
    ContentBlock, Message, Provider, Request, Response, Role, StopReason as LlmStop, StreamEvent,
    Usage,
};
use ez_tools::{Answer, Question, UserBridge};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
    decisions: Mutex<VecDeque<PermissionDecision>>,
    requests: Mutex<Vec<PermissionRequest>>,
}

impl TestClient {
    fn with(decisions: impl IntoIterator<Item = PermissionDecision>) -> Arc<Self> {
        let c = Self::default();
        c.decisions.lock().unwrap().extend(decisions);
        Arc::new(c)
    }
    fn events(&self) -> Vec<SessionEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait]
impl UserBridge for TestClient {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

#[async_trait]
impl Client for TestClient {
    async fn request_permission(&self, _: &str, req: PermissionRequest) -> PermissionDecision {
        self.requests.lock().unwrap().push(req);
        self.decisions
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PermissionDecision::AllowOnce)
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

struct Env {
    proj: TempDir,
    _user: TempDir,
    user_path: PathBuf,
    mock: Arc<MockProvider>,
}

impl Env {
    fn new(modes: Option<&str>, config: Option<&str>) -> Self {
        let proj = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
        if let Some(m) = modes {
            std::fs::write(proj.path().join(".ez/modes.toml"), m).unwrap();
        }
        if let Some(c) = config {
            std::fs::write(proj.path().join(".ez/config.toml"), c).unwrap();
        }
        Self {
            user_path: user.path().to_path_buf(),
            proj,
            _user: user,
            mock: Arc::new(MockProvider::default()),
        }
    }

    fn agent(&self) -> Agent {
        self.agent_with(self.mock.clone())
    }

    fn agent_with(&self, p: Arc<dyn Provider>) -> Agent {
        Agent::new(AgentOptions {
            provider_override: Some(p),
            user_config_dir: Some(self.user_path.clone()),
            ..Default::default()
        })
    }

    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }

    fn records(&self, id: &str) -> Vec<Record> {
        TranscriptReader::load(&ez_core::transcript::session_path(&self.cwd(), id))
            .unwrap()
            .records
            .into_iter()
            .map(|r| r.record)
            .collect()
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

fn last_user_text(r: &Request) -> String {
    let m = r.messages.last().unwrap();
    assert_eq!(m.role, Role::User);
    m.content
        .iter()
        .map(|b| match b {
            ContentBlock::Text { text } => text.clone(),
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => String::new(),
        })
        .collect()
}

fn tool_names(r: &Request) -> Vec<String> {
    r.tools.iter().map(|t| t.name.clone()).collect()
}

const MODES: &str = r#"
default_mode = "a"
[modes.a]
description = "mode a"
tools = ["Read", "Write"]
next = ["b"]
[modes.b]
description = "mode b"
tools = ["Read", "Grep"]
"#;

#[tokio::test]
async fn simple_text_reply() {
    let env = Env::new(None, None);
    env.mock.push(Message::assistant_text("hello there"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    assert_eq!(info.current_mode, "default");
    let client = TestClient::with([]);
    let r = agent
        .prompt(&info.id, "hi".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    assert!(
        client
            .events()
            .contains(&SessionEvent::AgentText("hello there".into()))
    );

    let recs = env.records(&info.id);
    let types: Vec<&str> = recs
        .iter()
        .map(|r| match r {
            Record::SessionStart(_) => "session_start",
            Record::UserMessage { .. } => "user_message",
            Record::AssistantMessage { .. } => "assistant_message",
            Record::TurnEnd { .. } => "turn_end",
            _ => "other",
        })
        .collect();
    assert_eq!(
        types,
        [
            "session_start",
            "user_message",
            "assistant_message",
            "turn_end"
        ]
    );
    let Record::AssistantMessage {
        metrics,
        request_id,
        ..
    } = &recs[2]
    else {
        panic!()
    };
    assert_eq!(*request_id, 1);
    assert!(metrics.time_to_first_token_ms.is_some());
    let Record::TurnEnd {
        model_requests,
        stop_reason,
        ..
    } = &recs[3]
    else {
        panic!()
    };
    assert_eq!((*model_requests, stop_reason.as_str()), (1, "end_turn"));
    let raw =
        std::fs::read_to_string(ez_core::transcript::session_path(&env.cwd(), &info.id)).unwrap();
    assert!(
        raw.lines()
            .all(|l| l.contains("\"ts\":\"") && l.contains("\"type\":\""))
    );
    let sys = &env.mock.requests.lock().unwrap()[0].system;
    assert!(sys.contains("Current mode: Default"));
}

#[tokio::test]
async fn write_tool_asks_and_creates_file() {
    let env = Env::new(None, None);
    env.mock.push(tool_use(
        "t1",
        "Write",
        json!({"file_path": "a.txt", "content": "hi\n"}),
    ));
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::AllowOnce]);
    let r = agent
        .prompt(&info.id, "write".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);
    assert_eq!(
        std::fs::read_to_string(env.proj.path().join("a.txt")).unwrap(),
        "hi\n"
    );
    assert_eq!(client.requests.lock().unwrap().len(), 1);

    let ev = client.events();
    assert!(
        ev.iter()
            .any(|e| matches!(e, SessionEvent::ToolCall { id, .. } if id == "t1"))
    );
    let done = ev
        .iter()
        .find_map(|e| match e {
            SessionEvent::ToolCallUpdate {
                status: ToolStatus::Completed,
                edits,
                ..
            } => Some(edits),
            _ => None,
        })
        .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].new_text, "hi\n");

    let recs = env.records(&info.id);
    let result = recs
        .iter()
        .find_map(|r| match r {
            Record::ToolResult {
                permission, edits, ..
            } => Some((permission, edits)),
            _ => None,
        })
        .unwrap();
    assert_eq!(*result.0, PermissionOutcome::AskApproved);
    assert!(result.1[0].diff.contains("+hi"));
    assert!(recs.iter().any(|r| matches!(
        r,
        Record::UserMessage {
            source: UserSource::ToolResults,
            ..
        }
    )));
    // second request carries the tool result back to the model
    assert_eq!(env.mock.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn allow_always_skips_second_prompt() {
    let env = Env::new(None, None);
    for i in 0..2 {
        env.mock.push(tool_use(
            &format!("t{i}"),
            "Write",
            json!({"file_path": format!("{i}.txt"), "content": "x"}),
        ));
    }
    env.mock.push(Message::assistant_text("done"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::AllowAlways]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    assert!(env.proj.path().join("1.txt").exists());
}

#[tokio::test]
async fn deny_by_mode_policy_and_user_reject() {
    let modes = r#"
[modes.a]
tools = ["Write", "Bash"]
[modes.a.permissions]
Write = "deny"
"#;
    let env = Env::new(Some(modes), None);
    env.mock.push(tool_use(
        "t1",
        "Write",
        json!({"file_path": "no.txt", "content": "x"}),
    ));
    env.mock
        .push(tool_use("t2", "Bash", json!({"command": "touch nope"})));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Reject]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert!(!env.proj.path().join("no.txt").exists());
    assert!(!env.proj.path().join("nope").exists());
    // only Bash asked
    assert_eq!(client.requests.lock().unwrap().len(), 1);
    let outcomes: Vec<(PermissionOutcome, String)> = env
        .records(&info.id)
        .into_iter()
        .filter_map(|r| match r {
            Record::ToolResult {
                permission,
                output,
                is_error: true,
                ..
            } => Some((permission, output)),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes[0],
        (PermissionOutcome::Denied, "denied by policy".into())
    );
    assert_eq!(
        outcomes[1],
        (PermissionOutcome::Rejected, "user rejected".into())
    );
}

#[tokio::test]
async fn post_edit_fix_turn() {
    let modes = r#"
[modes.a]
tools = ["Write"]
[modes.a.permissions]
Write = "allow"
[[modes.a.hooks.post_edit]]
run = "test -f ok.txt"
on_failure = "fix_turn"
"#;
    let env = Env::new(Some(modes), None);
    env.mock.push(tool_use(
        "t1",
        "Write",
        json!({"file_path": "a.txt", "content": "x"}),
    ));
    env.mock.push(Message::assistant_text("first attempt done"));
    env.mock.push(tool_use(
        "t2",
        "Write",
        json!({"file_path": "ok.txt", "content": "y"}),
    ));
    env.mock.push(Message::assistant_text("all good"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    let r = agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(r, StopReason::EndTurn);

    let reqs = env.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    let fix = last_user_text(&reqs[2]);
    assert!(
        fix.contains("hook commands failed") && fix.contains("test -f ok.txt"),
        "{fix}"
    );
    drop(reqs);
    assert!(
        client
            .events()
            .iter()
            .any(|e| matches!(e, SessionEvent::Notice(n) if n.contains("starting fix turn 1/2")))
    );

    let recs = env.records(&info.id);
    let hooks: Vec<bool> = recs
        .iter()
        .filter_map(|r| match r {
            Record::HookRun { event, success, .. } if event == "post_edit" => Some(*success),
            _ => None,
        })
        .collect();
    assert_eq!(hooks, [false, true]);
    assert!(recs.iter().any(|r| matches!(
        r,
        Record::UserMessage {
            source: UserSource::FixTurn,
            ..
        }
    )));
}

#[tokio::test]
async fn switch_mode_with_approval_and_auto_advance() {
    let env = Env::new(Some(MODES), None);
    env.mock.push(tool_use(
        "t1",
        "SwitchMode",
        json!({"mode_id": "b", "reason": "done here"}),
    ));
    env.mock.push(Message::assistant_text("now in b"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::AllowOnce]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();

    let req = client.requests.lock().unwrap()[0].clone();
    assert_eq!(req.title, "Switch to B mode: done here");
    assert!(!req.allow_always);
    assert!(
        client
            .events()
            .contains(&SessionEvent::ModeChanged("b".into()))
    );
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "b");
    {
        let reqs = env.mock.requests.lock().unwrap();
        assert_eq!(tool_names(&reqs[0]), ["Read", "Write", "SwitchMode"]);
        assert_eq!(tool_names(&reqs[1]), ["Read", "Grep", "SwitchMode"]);
        assert!(reqs[1].system.contains("Current mode: B"));
    }
    assert!(env.records(&info.id).iter().any(|r| matches!(r,
        Record::ModeChange { from, to, .. } if from == "a" && to == "b")));

    // auto-advance skips confirmation
    let auto = MODES.replace(
        "next = [\"b\"]",
        "next = [\"b\"]\nauto_advance_to = [\"b\"]",
    );
    let env = Env::new(Some(&auto), None);
    env.mock.push(tool_use(
        "t1",
        "SwitchMode",
        json!({"mode_id": "b", "reason": "r"}),
    ));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Reject]);
    agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert!(client.requests.lock().unwrap().is_empty());
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "b");

    // user-initiated switch back
    agent.set_mode(&info.id, "a", client.clone()).await.unwrap();
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "a");
    assert!(agent.set_mode(&info.id, "zzz", client).await.is_err());
    assert!(env.records(&info.id).iter().any(|r| matches!(r,
        Record::ModeChange { to, initiated_by: ez_core::transcript::Initiator::User, .. } if to == "a")));
}

#[tokio::test]
async fn switch_mode_rejected_stays() {
    let env = Env::new(Some(MODES), None);
    env.mock.push(tool_use(
        "t1",
        "SwitchMode",
        json!({"mode_id": "b", "reason": "r"}),
    ));
    env.mock.push(Message::assistant_text("ok"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Reject]);
    agent.prompt(&info.id, "go".into(), client).await.unwrap();
    assert_eq!(agent.session_modes(&info.id).unwrap().current_mode, "a");
}

#[tokio::test]
async fn resume_restores_history_and_repairs_dangling_tool_use() {
    let env = Env::new(None, None);
    let thinking = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Thinking {
                thinking: "pondering".into(),
                signature: Some("sig1".into()),
            },
            ContentBlock::Text {
                text: "answer".into(),
            },
        ],
    };
    env.mock.push(thinking.clone());
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    agent
        .prompt(&info.id, "first".into(), client)
        .await
        .unwrap();

    // Simulate a crash after a tool_use was recorded.
    let path = ez_core::transcript::session_path(&env.cwd(), &info.id);
    let mut w = ez_core::transcript::TranscriptWriter::open(&path).unwrap();
    w.append(&Record::UserMessage {
        message: Message::user_text("second"),
        source: UserSource::User,
    })
    .unwrap();
    w.append(&Record::AssistantMessage {
        message: tool_use("dangling", "Read", json!({"file_path": "x"})),
        request_id: 2,
        model: "m".into(),
        stop_reason: LlmStop::ToolUse,
        usage: Usage::default(),
        metrics: ez_core::transcript::Metrics {
            duration_ms: 1,
            time_to_first_token_ms: None,
            output_tokens_per_sec: None,
        },
    })
    .unwrap();
    drop(w);

    let env2_mock = Arc::new(MockProvider::new([Message::assistant_text("resumed")]));
    let agent2 = env.agent_with(env2_mock.clone());
    let client2 = TestClient::with([]);
    let info2 = agent2
        .load_session(&info.id, env.cwd(), client2.clone())
        .await
        .unwrap();
    assert_eq!(info2.id, info.id);
    let ev = client2.events();
    assert!(ev.contains(&SessionEvent::UserText("first".into())));
    assert!(ev.contains(&SessionEvent::AgentThought("pondering".into())));
    assert!(ev.contains(&SessionEvent::AgentText("answer".into())));

    agent2
        .prompt(&info.id, "third".into(), client2)
        .await
        .unwrap();
    let reqs = env2_mock.requests.lock().unwrap();
    let msgs = &reqs[0].messages;
    assert_eq!(msgs[1], thinking);
    let ContentBlock::ToolResult {
        tool_use_id,
        is_error,
        content,
    } = &msgs[4].content[0]
    else {
        panic!("{:?}", msgs[4])
    };
    assert_eq!((tool_use_id.as_str(), *is_error), ("dangling", true));
    assert!(content.contains("interrupted"));
    assert_eq!(msgs.last().unwrap().text(), "third");
    drop(reqs);

    let recs = env.records(&info.id);
    assert!(recs.iter().any(|r| matches!(r, Record::SessionResumed {})));
    let listed = agent2.list_sessions(&env.cwd()).unwrap();
    assert_eq!(listed[0].first_prompt, "first");
}

struct Slow;

#[async_trait]
impl Provider for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    async fn complete(
        &self,
        _: Request,
        _: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        unreachable!()
    }
}

#[tokio::test]
async fn cancellation_during_model_request() {
    let env = Env::new(None, None);
    let agent = Arc::new(env.agent_with(Arc::new(Slow)));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let a = agent.clone();
    let id = info.id.clone();
    let task = tokio::spawn(async move { a.prompt(&id, "go".into(), TestClient::with([])).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    agent.cancel(&info.id);
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(r, StopReason::Cancelled);
    assert!(env.records(&info.id).iter().any(|r| matches!(r,
        Record::TurnEnd { stop_reason, .. } if stop_reason == "cancelled")));
}

#[tokio::test]
async fn permission_cancel_aborts_with_valid_history() {
    let env = Env::new(None, None);
    let two = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Write".into(),
                input: json!({"file_path": "a", "content": "1"}),
            },
            ContentBlock::ToolUse {
                id: "b".into(),
                name: "Write".into(),
                input: json!({"file_path": "b", "content": "2"}),
            },
        ],
    };
    env.mock.push(two);
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([PermissionDecision::Cancelled]);
    let r = agent.prompt(&info.id, "go".into(), client).await.unwrap();
    assert_eq!(r, StopReason::Cancelled);
    let loaded =
        TranscriptReader::load(&ez_core::transcript::session_path(&env.cwd(), &info.id)).unwrap();
    let last = loaded.messages.iter().rev().nth(1).unwrap();
    assert_eq!(last.content.len(), 2);
    assert_eq!(loaded.messages.last().unwrap().role, Role::Assistant);
    assert!(loaded.dangling_tool_uses().is_empty());
}

#[tokio::test]
async fn compaction_replaces_history() {
    let env = Env::new(None, Some("compaction_threshold = 0.0001"));
    env.mock.push(Message::assistant_text("r1"));
    env.mock.push(Message::assistant_text("r2"));
    env.mock.push(Message::assistant_text("THE SUMMARY"));
    env.mock.push(Message::assistant_text("r3"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    for p in ["p1", "p2", "p3"] {
        agent
            .prompt(&info.id, p.into(), TestClient::with([]))
            .await
            .unwrap();
    }
    let reqs = env.mock.requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert!(reqs[2].tools.is_empty());
    let m = &reqs[3].messages;
    // Summary and the new prompt are merged into one user message.
    assert_eq!(m.len(), 1);
    assert!(m[0].text().contains("THE SUMMARY") && m[0].text().contains("p3"));
    drop(reqs);
    let loaded =
        TranscriptReader::load(&ez_core::transcript::session_path(&env.cwd(), &info.id)).unwrap();
    assert_eq!(loaded.messages.len(), 3);
    assert!(loaded.records.iter().any(|r| matches!(
        &r.record,
        Record::Compaction {
            replaced_messages: 4,
            ..
        }
    )));
}

/// First call sleeps (so tests can interleave), the rest are served by a mock.
struct DelayedFirst {
    delay_ms: u64,
    first: std::sync::atomic::AtomicBool,
    inner: Arc<MockProvider>,
}

#[async_trait]
impl Provider for DelayedFirst {
    fn name(&self) -> &str {
        "delayed"
    }
    async fn complete(
        &self,
        r: Request,
        tx: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        if !self.first.swap(true, std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        }
        self.inner.complete(r, tx).await
    }
}

fn delayed(env: &Env, delay_ms: u64, hang: bool) -> Arc<dyn Provider> {
    if hang {
        return Arc::new(Slow);
    }
    Arc::new(DelayedFirst {
        delay_ms,
        first: Default::default(),
        inner: env.mock.clone(),
    })
}

fn assert_alternating(r: &Request) {
    assert_eq!(r.messages.first().unwrap().role, Role::User);
    for w in r.messages.windows(2) {
        assert_ne!(w[0].role, w[1].role, "{:?}", r.messages);
    }
}

#[tokio::test]
async fn cancel_then_next_prompt_keeps_alternation() {
    let env = Env::new(None, None);
    let agent = Arc::new(env.agent_with(delayed(&env, 0, true)));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let (a, id) = (agent.clone(), info.id.clone());
    let task = tokio::spawn(async move { a.prompt(&id, "one".into(), TestClient::with([])).await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    agent.cancel(&info.id);
    assert_eq!(task.await.unwrap().unwrap(), StopReason::Cancelled);
    let recs = env.records(&info.id);
    assert!(recs.iter().any(|r| matches!(r,
        Record::AssistantMessage { message, .. } if message.text() == "[Request interrupted by user]")));

    // Same session state, but a working provider: a fresh agent resumes from the transcript.
    env.mock.push(Message::assistant_text("ok"));
    let agent2 = env.agent();
    agent2
        .load_session(&info.id, env.cwd(), TestClient::with([]))
        .await
        .unwrap();
    agent2
        .prompt(&info.id, "two".into(), TestClient::with([]))
        .await
        .unwrap();
    assert_alternating(&env.mock.requests.lock().unwrap()[0]);
}

#[test]
fn trailing_user_message_is_normalized_on_resume() {
    // A transcript that ends in a user message (e.g. crash) resumes with alternation.
    let env = Env::new(None, None);
    env.mock.push(Message::assistant_text("ok"));
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let agent = env.agent();
        let info = agent.new_session(env.cwd(), None).await.unwrap();
        agent
            .prompt(&info.id, "one".into(), TestClient::with([]))
            .await
            .unwrap();
        let path = ez_core::transcript::session_path(&env.cwd(), &info.id);
        let line = serde_json::to_string(&json!({
            "ts": "t", "type": "user_message", "source": "user",
            "message": Message::user_text("orphan"),
        }))
        .unwrap();
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{line}").unwrap();
        env.mock.push(Message::assistant_text("ok2"));
        let agent2 = env.agent();
        agent2
            .load_session(&info.id, env.cwd(), TestClient::with([]))
            .await
            .unwrap();
        agent2
            .prompt(&info.id, "two".into(), TestClient::with([]))
            .await
            .unwrap();
    });
    let reqs = env.mock.requests.lock().unwrap();
    assert_alternating(reqs.last().unwrap());
}

#[tokio::test]
async fn set_mode_during_prompt_does_not_block() {
    let env = Env::new(Some(MODES), None);
    env.mock
        .push(tool_use("t1", "Read", json!({"file_path": "nope"})));
    env.mock.push(Message::assistant_text("done"));
    let agent = Arc::new(env.agent_with(delayed(&env, 400, false)));
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    let (a, id, c) = (agent.clone(), info.id.clone(), client.clone());
    let task = tokio::spawn(async move { a.prompt(&id, "go".into(), c).await });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let t = std::time::Instant::now();
    agent.set_mode(&info.id, "b", client.clone()).await.unwrap();
    assert!(t.elapsed() < std::time::Duration::from_millis(200));
    assert!(
        agent
            .set_mode(&info.id, "zzz", client.clone())
            .await
            .is_err()
    );
    task.await.unwrap().unwrap();
    let reqs = env.mock.requests.lock().unwrap();
    assert!(tool_names(&reqs[0]).contains(&"Write".to_string()));
    assert!(!tool_names(&reqs[1]).contains(&"Write".to_string()));
    drop(reqs);
    assert!(
        client
            .events()
            .iter()
            .any(|e| matches!(e, SessionEvent::ModeChanged(m) if m == "b"))
    );
    let changes = env
        .records(&info.id)
        .into_iter()
        .filter(|r| matches!(r, Record::ModeChange { to, .. } if to == "b"))
        .count();
    assert_eq!(changes, 1);
}

const EDIT_MODE: &str = r#"
[modes.a]
tools = ["Write"]
[modes.a.permissions]
Write = "allow"
[[modes.a.hooks.post_edit]]
run = "sleep 0.2; touch hook-ran"
"#;

fn write_use(id: &str, f: &str) -> Message {
    tool_use(id, "Write", json!({"file_path": f, "content": "x"}))
}

#[tokio::test]
async fn post_edit_runs_on_max_turn_requests_with_duration() {
    let env = Env::new(Some(EDIT_MODE), Some("max_turn_requests = 1"));
    env.mock.push(write_use("t1", "a.txt"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let r = agent
        .prompt(&info.id, "go".into(), TestClient::with([]))
        .await
        .unwrap();
    assert_eq!(r, StopReason::MaxTurnRequests);
    assert!(env.cwd().join("hook-ran").exists());
    let d = env
        .records(&info.id)
        .iter()
        .find_map(|r| match r {
            Record::HookRun {
                event, duration_ms, ..
            } if event == "post_edit" => Some(*duration_ms),
            _ => None,
        })
        .unwrap();
    assert!((150..5000).contains(&d), "{d}");
}

#[tokio::test]
async fn cancelled_turn_edits_are_formatted_next_turn() {
    let env = Env::new(
        Some(&EDIT_MODE.replace("[\"Write\"]", "[\"Write\", \"Bash\"]")),
        None,
    );
    let two = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Write".into(),
                input: json!({"file_path": "a.txt", "content": "1"}),
            },
            ContentBlock::ToolUse {
                id: "b".into(),
                name: "Bash".into(),
                input: json!({"command": "true"}),
            },
        ],
    };
    env.mock.push(two);
    env.mock.push(Message::assistant_text("fine"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let r = agent
        .prompt(
            &info.id,
            "go".into(),
            TestClient::with([PermissionDecision::Cancelled]),
        )
        .await
        .unwrap();
    assert_eq!(r, StopReason::Cancelled);
    assert!(!env.cwd().join("hook-ran").exists());
    agent
        .prompt(&info.id, "again".into(), TestClient::with([]))
        .await
        .unwrap();
    assert!(env.cwd().join("hook-ran").exists());
}

const MODEL_CFG: &str = r#"
default_profile = "p1"
[profiles.p1]
provider = "mock"
model = "m1"
[profiles.p2]
provider = "mock"
model = "m2"
max_tokens = 77
"#;

async fn model_used(modes: &str, cli_model: Option<&str>) -> (String, u32) {
    let env = Env::new(Some(modes), Some(MODEL_CFG));
    env.mock.push(Message::assistant_text("hi"));
    let agent = Agent::new(AgentOptions {
        provider_override: Some(env.mock.clone()),
        user_config_dir: Some(env.user_path.clone()),
        model_override: cli_model.map(String::from),
        ..Default::default()
    });
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    agent
        .prompt(&info.id, "x".into(), TestClient::with([]))
        .await
        .unwrap();
    let r = &env.mock.requests.lock().unwrap()[0];
    (r.model.clone(), r.max_tokens)
}

#[tokio::test]
async fn mode_model_and_profile_resolution() {
    let (m, _) = model_used("[modes.a]\nmodel = \"custom\"", None).await;
    assert_eq!(m, "custom");
    let (m, mt) = model_used("[modes.a]\nprofile = \"p2\"", None).await;
    assert_eq!((m.as_str(), mt), ("m2", 77));
    // legacy: model naming a profile
    let (m, _) = model_used("[modes.a]\nmodel = \"p2\"", None).await;
    assert_eq!(m, "m2");
    // CLI --model applies only when the mode sets no model
    let (m, _) = model_used("[modes.a]\ndescription = \"x\"", Some("cli")).await;
    assert_eq!(m, "cli");
    let (m, _) = model_used("[modes.a]\nmodel = \"custom\"", Some("cli")).await;
    assert_eq!(m, "custom");
}

#[tokio::test]
async fn missing_mode_profile_warns_once_and_falls_back() {
    let env = Env::new(
        Some(
            "[modes.a]\nprofile = \"openrouter\"\nmodel = \"vendor/x\"\n[modes.b]\nprofile = \"p2\"",
        ),
        Some(MODEL_CFG),
    );
    env.mock.push(Message::assistant_text("one"));
    env.mock.push(Message::assistant_text("two"));
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    agent
        .prompt(&info.id, "x".into(), client.clone())
        .await
        .unwrap();
    agent
        .prompt(&info.id, "y".into(), client.clone())
        .await
        .unwrap();
    // Falls back to the default profile and its model, without erroring.
    assert_eq!(env.mock.requests.lock().unwrap()[0].model, "m1");
    let notices: Vec<String> = client
        .events()
        .into_iter()
        .filter_map(|e| match e {
            SessionEvent::Notice(n) if n.contains("isn't configured") => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("mode `a`") && notices[0].contains("`openrouter`"));
    assert!(notices[0].contains("ez setup"));
}

#[tokio::test]
async fn set_session_model_pins_records_and_restores() {
    let env = Env::new(Some("[modes.a]\nmodel = \"custom\""), Some(MODEL_CFG));
    for t in ["1", "2", "3"] {
        env.mock.push(Message::assistant_text(t));
    }
    let agent = env.agent();
    let info = agent.new_session(env.cwd(), None).await.unwrap();
    let client = TestClient::with([]);
    agent
        .set_session_model(&info.id, Some("pinned".into()), client.clone())
        .await
        .unwrap();
    agent
        .prompt(&info.id, "x".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(env.mock.requests.lock().unwrap()[0].model, "pinned");
    assert!(env.records(&info.id).iter().any(|r| matches!(
        r,
        Record::ModelChange { to: Some(m), .. } if m == "pinned"
    )));
    // A resumed session keeps the pin.
    let agent2 = env.agent();
    agent2
        .load_session(&info.id, env.cwd(), client.clone())
        .await
        .unwrap();
    agent2
        .prompt(&info.id, "y".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(env.mock.requests.lock().unwrap()[1].model, "pinned");
    // Clearing falls back to the mode's model.
    agent2
        .set_session_model(&info.id, None, client.clone())
        .await
        .unwrap();
    agent2
        .prompt(&info.id, "z".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(env.mock.requests.lock().unwrap()[2].model, "custom");
}
