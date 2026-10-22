use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ez_acp::schema::*;
use ez_acp::{Connection, Handler, RpcError, spawn_in_process};
use ez_core::{Agent, AgentOptions};
use ez_llm::mock::MockProvider;
use ez_llm::{ContentBlock as LlmBlock, Message, Provider, Role};
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Default)]
struct TestClient {
    updates: Mutex<Vec<SessionNotification>>,
    permission_reqs: Mutex<Vec<RequestPermissionRequest>>,
    /// Option ids to select, in order; `None` entry or empty queue means cancelled.
    selections: Mutex<VecDeque<Option<String>>>,
    ask_reqs: Mutex<Vec<Value>>,
    ask_reply: Mutex<Option<Value>>,
}

impl TestClient {
    fn updates(&self) -> Vec<SessionUpdate> {
        self.updates
            .lock()
            .unwrap()
            .iter()
            .map(|n| n.update.clone())
            .collect()
    }
}

#[async_trait]
impl Handler for TestClient {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            SESSION_REQUEST_PERMISSION => {
                self.permission_reqs
                    .lock()
                    .unwrap()
                    .push(serde_json::from_value(params).unwrap());
                let sel = self.selections.lock().unwrap().pop_front().flatten();
                Ok(match sel {
                    Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
                    None => json!({"outcome":{"outcome":"cancelled"}}),
                })
            }
            ASK_USER_QUESTION => {
                self.ask_reqs.lock().unwrap().push(params);
                Ok(self.ask_reply.lock().unwrap().clone().unwrap())
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }
    async fn notification(&self, method: &str, params: Value) {
        if method == SESSION_UPDATE {
            self.updates
                .lock()
                .unwrap()
                .push(serde_json::from_value(params).unwrap());
        }
    }
}

struct Env {
    proj: TempDir,
    user: TempDir,
    client: Arc<TestClient>,
}

impl Env {
    fn new() -> Self {
        Self {
            proj: tempfile::tempdir().unwrap(),
            user: tempfile::tempdir().unwrap(),
            client: Arc::new(TestClient::default()),
        }
    }
    fn cwd(&self) -> PathBuf {
        self.proj.path().to_path_buf()
    }
    fn agent(&self, p: Arc<dyn Provider>) -> Arc<Agent> {
        Arc::new(Agent::new(AgentOptions {
            provider_override: Some(p),
            user_config_dir: Some(self.user.path().to_path_buf()),
            ..Default::default()
        }))
    }
    fn connect(&self, p: Arc<dyn Provider>) -> Connection {
        spawn_in_process(self.agent(p), self.client.clone())
    }
}

fn tool_use(id: &str, name: &str, input: Value) -> Message {
    Message {
        role: Role::Assistant,
        content: vec![LlmBlock::ToolUse {
            id: id.into(),
            name: name.into(),
            input,
        }],
    }
}

fn mock(msgs: impl IntoIterator<Item = Message>) -> Arc<MockProvider> {
    Arc::new(MockProvider::new(msgs))
}

async fn init(c: &Connection, ask: bool) -> Value {
    let caps = if ask {
        json!({"_meta":{"ez":{"askUserQuestion":true}}})
    } else {
        json!({})
    };
    c.request(
        INITIALIZE,
        json!({"protocolVersion":1,"clientCapabilities":caps}),
    )
    .await
    .unwrap()
}

async fn new_session(c: &Connection, env: &Env) -> String {
    let r = c
        .request(SESSION_NEW, json!({"cwd":env.cwd(),"mcpServers":[]}))
        .await
        .unwrap();
    r["sessionId"].as_str().unwrap().to_string()
}

async fn prompt(c: &Connection, sid: &str, text: &str) -> Value {
    c.request(
        SESSION_PROMPT,
        json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]}),
    )
    .await
    .unwrap()
}

/// Notifications sent before a response are always handled before the request
/// future resolves, so no polling is needed.
async fn wait_for(mut f: impl FnMut() -> bool) {
    assert!(f(), "condition not met");
}

fn agent_text(u: &[SessionUpdate]) -> String {
    u.iter()
        .filter_map(|u| match u {
            SessionUpdate::AgentMessageChunk(ContentChunk {
                content: ContentBlock::Text(t),
                ..
            }) => Some(t.text.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn initialize_and_new_session() {
    let env = Env::new();
    let c = env.connect(mock([]));
    let r = init(&c, true).await;
    assert_eq!(r["protocolVersion"], 1);
    assert_eq!(r["agentCapabilities"]["loadSession"], true);
    assert_eq!(
        r["agentCapabilities"]["promptCapabilities"]["embeddedContext"],
        true
    );
    assert_eq!(r["agentCapabilities"]["promptCapabilities"]["image"], false);
    assert_eq!(r["authMethods"], json!([]));
    assert_eq!(r["agentInfo"]["name"], "ez");
    let v = c
        .request(INITIALIZE, json!({"protocolVersion":99}))
        .await
        .unwrap();
    assert_eq!(v["protocolVersion"], 1);
    assert_eq!(c.request(AUTHENTICATE, json!({})).await.unwrap(), json!({}));

    let r = c
        .request(SESSION_NEW, json!({"cwd":env.cwd()}))
        .await
        .unwrap();
    assert!(!r["sessionId"].as_str().unwrap().is_empty());
    assert_eq!(r["modes"]["currentModeId"], "default");
    assert_eq!(r["modes"]["availableModes"][0]["id"], "default");

    assert_eq!(c.request("nope", json!({})).await.unwrap_err().code, -32601);
    assert_eq!(
        c.request("_ez/nope", json!({})).await.unwrap_err().code,
        -32601
    );
}

#[tokio::test]
async fn text_prompt_streams() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("hello there")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    let r = prompt(&c, &sid, "hi").await;
    assert_eq!(r["stopReason"], "end_turn");
    wait_for(|| agent_text(&env.client.updates()) == "hello there").await;
    assert!(
        env.client
            .updates
            .lock()
            .unwrap()
            .iter()
            .all(|n| *n.session_id.0 == *sid)
    );

    let l = c
        .request(LIST_SESSIONS, json!({"cwd":env.cwd()}))
        .await
        .unwrap();
    assert_eq!(l["sessions"][0]["id"], sid);
    assert_eq!(l["sessions"][0]["firstPrompt"], "hi");
    assert_eq!(l["sessions"][0]["mode"], "default");
}

#[tokio::test]
async fn write_tool_requests_permission_and_reports_diff() {
    let env = Env::new();
    let file = env.cwd().join("out.txt");
    let c = env.connect(mock([
        tool_use(
            "t1",
            "Write",
            json!({"file_path": file, "content": "new body"}),
        ),
        Message::assistant_text("done"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .push_back(Some("allow_once".into()));
    let r = prompt(&c, &sid, "write it").await;
    assert_eq!(r["stopReason"], "end_turn");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new body");

    let reqs = env.client.permission_reqs.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(&*reqs[0].tool_call.tool_call_id.0, "t1");
    assert_eq!(reqs[0].tool_call.fields.kind, Some(ToolKind::Edit));
    assert_eq!(
        reqs[0].tool_call.fields.status,
        Some(ToolCallStatus::Pending)
    );
    let kinds: Vec<_> = reqs[0].options.iter().map(|o| o.kind).collect();
    assert_eq!(kinds[0], PermissionOptionKind::AllowOnce);
    assert_eq!(*kinds.last().unwrap(), PermissionOptionKind::RejectOnce);

    wait_for(|| agent_text(&env.client.updates()) == "done").await;
    let u = env.client.updates();
    let call = u
        .iter()
        .position(|u| matches!(u, SessionUpdate::ToolCall(t) if &*t.tool_call_id.0 == "t1"))
        .expect("tool_call");
    let upd = u
        .iter()
        .position(|u| {
            matches!(u, SessionUpdate::ToolCallUpdate(t) if &*t.tool_call_id.0 == "t1"
            && t.fields.status == Some(ToolCallStatus::Completed))
        })
        .expect("tool_call_update");
    assert!(call < upd);
    let SessionUpdate::ToolCall(tc) = &u[call] else {
        unreachable!()
    };
    assert_eq!(tc.status, ToolCallStatus::Pending);
    assert_eq!(tc.kind, ToolKind::Edit);
    assert!(tc.raw_input.is_some());
    let SessionUpdate::ToolCallUpdate(tu) = &u[upd] else {
        unreachable!()
    };
    assert!(tu.fields.content.as_ref().unwrap().iter().any(|c| matches!(c,
        ToolCallContent::Diff(d) if d.path == file && d.old_text.is_none() && d.new_text == "new body")));
}

#[tokio::test]
async fn write_rejected_and_cancelled() {
    let env = Env::new();
    let file = env.cwd().join("x.txt");
    let c = env.connect(mock([
        tool_use("t1", "Write", json!({"file_path": file, "content": "a"})),
        Message::assistant_text("ok"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .push_back(Some("reject_once".into()));
    let r = prompt(&c, &sid, "w").await;
    assert_eq!(r["stopReason"], "end_turn");
    assert!(!file.exists());
}

#[tokio::test]
async fn ask_user_question_via_extension() {
    let env = Env::new();
    let q = json!({"questions":[{"question":"Which DB?","header":"DB","multiSelect":false,
        "options":[{"label":"sqlite","description":""},{"label":"postgres","description":""}]}]});
    let c = env.connect(mock([
        tool_use("t1", "AskUserQuestion", q),
        Message::assistant_text("thanks"),
    ]));
    init(&c, true).await;
    *env.client.ask_reply.lock().unwrap() = Some(json!({
        "answers":[{"question":"Which DB?","selected":["sqlite"]}],"cancelled":false}));
    let sid = new_session(&c, &env).await;
    let r = prompt(&c, &sid, "go").await;
    assert_eq!(r["stopReason"], "end_turn");
    let asks = env.client.ask_reqs.lock().unwrap().clone();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0]["sessionId"], sid.as_str());
    assert_eq!(asks[0]["questions"][0]["question"], "Which DB?");
    assert_eq!(asks[0]["questions"][0]["options"][1]["label"], "postgres");
    assert!(env.client.permission_reqs.lock().unwrap().is_empty());
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u,
            SessionUpdate::ToolCallUpdate(t) if t.fields.status == Some(ToolCallStatus::Completed)
            && format!("{:?}", t.fields.content).contains("sqlite"))
        })
    })
    .await;
}

#[tokio::test]
async fn ask_user_question_fallback_and_cancel() {
    let env = Env::new();
    let q = json!({"questions":[{"question":"Which DB?","options":[{"label":"sqlite"},{"label":"postgres"}]}]});
    let c = env.connect(mock([
        tool_use("t1", "AskUserQuestion", q.clone()),
        Message::assistant_text("one"),
        tool_use("t2", "AskUserQuestion", q),
        Message::assistant_text("two"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .extend([Some("opt-1".to_string()), None]);
    prompt(&c, &sid, "go").await;
    let reqs = env.client.permission_reqs.lock().unwrap().clone();
    assert_eq!(reqs[0].tool_call.fields.title.as_deref(), Some("Which DB?"));
    assert_eq!(reqs[0].tool_call.fields.kind, Some(ToolKind::Other));
    let names: Vec<_> = reqs[0].options.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["sqlite", "postgres"]);
    assert_eq!(&*reqs[0].options[1].option_id.0, "opt-1");
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u,
            SessionUpdate::ToolCallUpdate(t) if format!("{:?}", t.fields.content).contains("postgres"))
        })
    })
    .await;
    // Second call is cancelled -> tool error, turn still completes.
    let r = prompt(&c, &sid, "again").await;
    assert_eq!(r["stopReason"], "end_turn");
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u,
            SessionUpdate::ToolCallUpdate(t) if t.fields.status == Some(ToolCallStatus::Failed))
        })
    })
    .await;
}

#[tokio::test]
async fn set_mode_emits_update() {
    let env = Env::new();
    std::fs::create_dir_all(env.cwd().join(".ez")).unwrap();
    std::fs::write(
        env.cwd().join(".ez/modes.toml"),
        "default_mode = \"a\"\n[modes.a]\ndescription = \"A\"\ntools = [\"Read\"]\n[modes.b]\ndescription = \"B\"\ntools = [\"Read\"]\n",
    )
    .unwrap();
    let c = env.connect(mock([]));
    init(&c, false).await;
    let r = c
        .request(SESSION_NEW, json!({"cwd":env.cwd()}))
        .await
        .unwrap();
    assert_eq!(r["modes"]["currentModeId"], "a");
    assert_eq!(r["modes"]["availableModes"].as_array().unwrap().len(), 2);
    let sid = r["sessionId"].as_str().unwrap();
    c.request(SESSION_SET_MODE, json!({"sessionId":sid,"modeId":"b"}))
        .await
        .unwrap();
    wait_for(|| {
        env.client.updates().iter().any(
            |u| matches!(u, SessionUpdate::CurrentModeUpdate(m) if &*m.current_mode_id.0 == "b"),
        )
    })
    .await;
    let e = c
        .request(SESSION_SET_MODE, json!({"sessionId":sid,"modeId":"zzz"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, -32603);
}

#[tokio::test]
async fn load_session_replays() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("first answer")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    prompt(&c, &sid, "question one").await;
    wait_for(|| agent_text(&env.client.updates()) == "first answer").await;

    // New agent instance, same project dir, fresh client.
    let env2 = Env {
        proj: env.proj,
        user: tempfile::tempdir().unwrap(),
        client: Arc::new(TestClient::default()),
    };
    let c2 = env2.connect(mock([]));
    init(&c2, false).await;
    let r = c2
        .request(
            SESSION_LOAD,
            json!({"sessionId":sid,"cwd":env2.cwd(),"mcpServers":[]}),
        )
        .await
        .unwrap();
    assert_eq!(r["modes"]["currentModeId"], "default");
    wait_for(|| agent_text(&env2.client.updates()) == "first answer").await;
    let u = env2.client.updates();
    let user = u
        .iter()
        .position(|u| matches!(u, SessionUpdate::UserMessageChunk(ContentChunk { content: ContentBlock::Text(t), .. }) if t.text == "question one"))
        .expect("user chunk");
    let agent = u
        .iter()
        .position(|u| matches!(u, SessionUpdate::AgentMessageChunk(_)))
        .unwrap();
    assert!(user < agent);
}

struct Slow;
#[async_trait]
impl Provider for Slow {
    fn name(&self) -> &str {
        "slow"
    }
    async fn complete(
        &self,
        _: ez_llm::Request,
        _: tokio::sync::mpsc::UnboundedSender<ez_llm::StreamEvent>,
    ) -> anyhow::Result<ez_llm::Response> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        unreachable!()
    }
}

#[tokio::test]
async fn cancel_notification() {
    let env = Env::new();
    let c = env.connect(Arc::new(Slow));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    let c2 = c.clone();
    let sid2 = sid.clone();
    let task = tokio::spawn(async move { prompt(&c2, &sid2, "go").await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    c.notify(SESSION_CANCEL, json!({"sessionId":sid})).unwrap();
    let r = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopReason"], "cancelled");
}

#[tokio::test]
async fn set_model_extension_pins_and_notifies() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("ok")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    c.request(
        "_ez/set_model",
        json!({"sessionId": sid, "model": "pinned-model"}),
    )
    .await
    .unwrap();
    wait_for(|| agent_text(&env.client.updates()).contains("model set to pinned-model")).await;
    let e = c
        .request("_ez/set_model", json!({"model": "x"}))
        .await
        .unwrap_err();
    assert_eq!(e.code, -32602);
    let e = c
        .request("_ez/set_model", json!({"sessionId": "nope", "model": "x"}))
        .await;
    assert!(e.is_err());
}

// ---- standard methods, capabilities and leniency ----------------------------------------------

async fn init_with(c: &Connection, caps: Value) -> Value {
    c.request(
        INITIALIZE,
        json!({"protocolVersion":1,"clientCapabilities":caps}),
    )
    .await
    .unwrap()
}

fn notice_texts(u: &[SessionUpdate]) -> Vec<String> {
    u.iter()
        .filter_map(|u| match u {
            SessionUpdate::Notice(n) => Some(n.title.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn initialize_advertises_accurate_capabilities() {
    let env = Env::new();
    let c = env.connect(mock([]));
    let r = init(&c, false).await;
    let caps = &r["agentCapabilities"];
    assert_eq!(caps["loadSession"], true);
    assert_eq!(caps["promptCapabilities"]["embeddedContext"], true);
    assert_eq!(caps["promptCapabilities"]["image"], false);
    assert_eq!(caps["promptCapabilities"]["audio"], false);
    assert_eq!(caps["mcpCapabilities"], json!({"http": true, "sse": false}));
    assert_eq!(
        caps["sessionCapabilities"],
        json!({"list": {}, "resume": {}, "close": {}})
    );
    // A bare initialize (no version, no capabilities) is accepted as v1.
    let r = c.request(INITIALIZE, json!({})).await.unwrap();
    assert_eq!(r["protocolVersion"], 1);
}

#[tokio::test]
async fn new_session_exposes_model_config_option() {
    let env = Env::new();
    let c = env.connect(mock([]));
    init(&c, false).await;
    let r = c
        .request(SESSION_NEW, json!({"cwd": env.cwd()}))
        .await
        .unwrap();
    let opt = &r["configOptions"][0];
    assert_eq!(opt["id"], "model");
    assert_eq!(opt["category"], "model");
    assert_eq!(opt["type"], "select");
    let current = opt["currentValue"].as_str().unwrap();
    assert!(!current.is_empty());
    assert!(
        opt["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["value"] == current)
    );
}

#[tokio::test]
async fn set_config_option_pins_model() {
    let env = Env::new();
    let c = env.connect(mock([]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    let r = c
        .request(
            SESSION_SET_CONFIG_OPTION,
            json!({"sessionId": sid, "configId": "model", "value": "pinned-model"}),
        )
        .await
        .unwrap();
    assert_eq!(r["configOptions"][0]["currentValue"], "pinned-model");
    wait_for(|| agent_text(&env.client.updates()).contains("model set to pinned-model")).await;
    // The effective model change is also pushed as config_option_update.
    wait_for(|| {
        env.client.updates().iter().any(|u| {
            matches!(u, SessionUpdate::ConfigOptionUpdate(c)
                if format!("{:?}", c.config_options).contains("pinned-model"))
        })
    })
    .await;
    for bad in [
        json!({"sessionId": sid, "configId": "nope", "value": "x"}),
        json!({"sessionId": sid, "configId": "model", "type": "boolean", "value": true}),
        json!({"sessionId": "missing", "configId": "model", "value": "x"}),
    ] {
        assert!(c.request(SESSION_SET_CONFIG_OPTION, bad).await.is_err());
    }
}

#[tokio::test]
async fn standard_session_list_and_legacy_alias() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("hi")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    prompt(&c, &sid, "first line\nsecond line").await;
    let l = c
        .request(SESSION_LIST, json!({"cwd": env.cwd()}))
        .await
        .unwrap();
    let s = &l["sessions"][0];
    assert_eq!(s["sessionId"], sid.as_str());
    assert_eq!(s["cwd"], json!(env.cwd()));
    assert_eq!(s["title"], "first line");
    assert!(s["updatedAt"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(s["_meta"]["ez"]["mode"], "default");
    assert!(l.get("nextCursor").is_none());
    // Deprecated alias keeps its original shape.
    let old = c
        .request(LIST_SESSIONS, json!({"cwd": env.cwd()}))
        .await
        .unwrap();
    assert_eq!(old["sessions"][0]["id"], sid.as_str());
    assert_eq!(old["sessions"][0]["firstPrompt"], "first line\nsecond line");
}

#[tokio::test]
async fn resume_restores_without_replay_and_close_frees_session() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("first answer")]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    prompt(&c, &sid, "question one").await;

    // Close: further use of the session fails, the transcript stays.
    c.request(SESSION_CLOSE, json!({"sessionId": sid}))
        .await
        .unwrap();
    assert!(
        c.request(
            SESSION_PROMPT,
            json!({"sessionId": sid, "prompt": [{"type":"text","text":"x"}]})
        )
        .await
        .is_err()
    );
    assert!(
        c.request(SESSION_CLOSE, json!({"sessionId": sid}))
            .await
            .is_err()
    );

    // Resume on a fresh client: no history is replayed, the session works again.
    let env2 = Env {
        proj: env.proj,
        user: tempfile::tempdir().unwrap(),
        client: Arc::new(TestClient::default()),
    };
    let c2 = env2.connect(mock([Message::assistant_text("second answer")]));
    init(&c2, false).await;
    let r = c2
        .request(SESSION_RESUME, json!({"sessionId": sid, "cwd": env2.cwd()}))
        .await
        .unwrap();
    assert_eq!(r["modes"]["currentModeId"], "default");
    assert!(r["configOptions"][0]["id"] == "model");
    assert!(env2.client.updates().is_empty(), "resume must not replay");
    let p = prompt(&c2, &sid, "question two").await;
    assert_eq!(p["stopReason"], "end_turn");
    assert_eq!(agent_text(&env2.client.updates()), "second answer");
}

#[tokio::test]
async fn lenient_inbound_parsing() {
    let env = Env::new();
    let c = env.connect(mock([Message::assistant_text("ok")]));
    init(&c, false).await;
    // No mcpServers; a malformed / unsupported one is skipped, not fatal.
    let r = c
        .request(SESSION_NEW, json!({"cwd": env.cwd()}))
        .await
        .unwrap();
    assert!(r["sessionId"].is_string());
    let r = c
        .request(
            SESSION_NEW,
            json!({"cwd": env.cwd(), "mcpServers": [{"type":"sse","name":"s","url":"http://x"}, 42]}),
        )
        .await
        .unwrap();
    let sid = r["sessionId"].as_str().unwrap();
    // Unknown content block types are skipped; the known ones still reach the model.
    let p = c
        .request(
            SESSION_PROMPT,
            json!({"sessionId": sid, "prompt": [
                {"type":"hologram","x":1},
                {"type":"text","text":"hello"},
                {"type":"audio","data":"AA==","mimeType":"audio/wav"}]}),
        )
        .await
        .unwrap();
    assert_eq!(p["stopReason"], "end_turn");
    // A request without sessionId is invalid params.
    let e = c
        .request(SESSION_PROMPT, json!({"prompt": []}))
        .await
        .unwrap_err();
    assert_eq!(e.code, -32602);
}

#[tokio::test]
async fn notices_use_standard_update_only_for_capable_clients() {
    for (caps, expect_notice) in [
        (json!({}), false),
        (json!({"session": {"notices": {}}}), true),
        (json!({"_meta": {"ez": {"notices": true}}}), true),
    ] {
        let env = Env::new();
        let c = env.connect(mock([]));
        init_with(&c, caps).await;
        let sid = new_session(&c, &env).await;
        c.request(
            SET_MODEL,
            json!({"sessionId": sid, "model": "pinned-model"}),
        )
        .await
        .unwrap();
        let u = env.client.updates();
        if expect_notice {
            assert_eq!(
                notice_texts(&u),
                ["model set to pinned-model for this session"]
            );
            assert!(agent_text(&u).is_empty());
        } else {
            assert!(notice_texts(&u).is_empty());
            assert!(agent_text(&u).contains("model set to pinned-model"));
        }
    }
}

struct UsageProvider;
#[async_trait]
impl Provider for UsageProvider {
    fn name(&self) -> &str {
        "usage"
    }
    async fn complete(
        &self,
        _: ez_llm::Request,
        _: tokio::sync::mpsc::UnboundedSender<ez_llm::StreamEvent>,
    ) -> anyhow::Result<ez_llm::Response> {
        Ok(ez_llm::Response {
            message: Message::assistant_text("counted"),
            stop_reason: ez_llm::StopReason::EndTurn,
            usage: ez_llm::Usage {
                input_tokens: 1200,
                output_tokens: 34,
            },
        })
    }
}

#[tokio::test]
async fn usage_update_follows_model_request() {
    let env = Env::new();
    let c = env.connect(Arc::new(UsageProvider));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    prompt(&c, &sid, "count").await;
    let u = env.client.updates();
    let usage: Vec<_> = u
        .iter()
        .filter_map(|u| match u {
            SessionUpdate::UsageUpdate(x) => Some((x.used, x.size)),
            _ => None,
        })
        .collect();
    assert_eq!(usage, [(1234, 200_000)]);
}

#[tokio::test]
async fn tool_call_update_omits_empty_content() {
    let env = Env::new();
    let c = env.connect(mock([
        tool_use("t1", "Bash", json!({"command": "true"})),
        Message::assistant_text("done"),
    ]));
    init(&c, false).await;
    let sid = new_session(&c, &env).await;
    env.client
        .selections
        .lock()
        .unwrap()
        .push_back(Some("allow_once".into()));
    prompt(&c, &sid, "run").await;
    let u = env.client.updates();
    let progress = u
        .iter()
        .find_map(|u| match u {
            SessionUpdate::ToolCallUpdate(t)
                if t.fields.status == Some(ToolCallStatus::InProgress) =>
            {
                Some(t.clone())
            }
            _ => None,
        })
        .expect("in_progress update");
    // An empty `content` array would clear what the client already shows.
    assert!(progress.fields.content.is_none());
}

#[tokio::test]
async fn raw_in_process_relays_json_lines() {
    let env = Env::new();
    let (tx, mut rx) = ez_acp::spawn_in_process_raw(env.agent(mock([])));
    tx.send(
        json!({"jsonrpc":"2.0","id":1,"method":INITIALIZE,"params":{"protocolVersion":1}})
            .to_string(),
    )
    .unwrap();
    let line = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timeout")
        .expect("closed");
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], 1);
    assert_eq!(v["result"]["protocolVersion"], 1);
    drop(tx);
    let end = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timeout");
    assert!(end.is_none());
}
