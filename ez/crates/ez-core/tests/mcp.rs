use async_trait::async_trait;
use ez_core::McpServerConfig;
use ez_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ez_core::transcript::{Record, TranscriptReader, session_path};
use ez_llm::mock::MockProvider;
use ez_llm::{ContentBlock, Message, Request, Role};
use ez_tools::{Answer, Question, UserBridge};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
    requests: Mutex<Vec<PermissionRequest>>,
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
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

fn fake_script() -> String {
    format!(
        "{}/../ez-mcp/tests/fake_mcp_server.py",
        env!("CARGO_MANIFEST_DIR")
    )
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

fn tool_names(r: &Request) -> Vec<String> {
    r.tools.iter().map(|t| t.name.clone()).collect()
}

fn result_text(r: &Request) -> String {
    r.messages
        .last()
        .unwrap()
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::ToolResult { content, .. } => content.clone(),
            _ => String::new(),
        })
        .collect()
}

const MODES: &str = r#"
[modes.a]
mcp_tools = ["mcp__fake__*"]
next = ["b"]
[modes.b]
"#;

#[tokio::test]
async fn mcp_tools_end_to_end() {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
    std::fs::write(proj.path().join(".ez/modes.toml"), MODES).unwrap();
    std::fs::write(
        proj.path().join(".mcp.json"),
        json!({"mcpServers": {
            "fake": {"command": "python3", "args": [fake_script(), "0"]},
            "broken": {"command": "/nonexistent/mcp-server"}
        }})
        .to_string(),
    )
    .unwrap();

    let mock = Arc::new(MockProvider::new([
        tool_use("1", "mcp__fake__echo", json!({"text": "hi"})),
        tool_use("2", "mcp__fake__fail", json!({})),
        tool_use("3", "mcp__extra__echo", json!({"text": "acp"})),
        Message::assistant_text("done"),
    ]));
    let agent = Agent::new(AgentOptions {
        provider_override: Some(mock.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let mut extra = BTreeMap::new();
    extra.insert(
        "extra".to_string(),
        McpServerConfig::stdio("python3", &[&fake_script(), "0"]),
    );
    let info = agent
        .new_session_with(proj.path().to_path_buf(), None, extra)
        .await
        .unwrap();
    let client = Arc::new(TestClient::default());
    let stop = agent
        .prompt(&info.id, "go".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(stop, StopReason::EndTurn);

    let reqs = mock.requests.lock().unwrap();
    let names = tool_names(&reqs[0]);
    assert!(names.contains(&"mcp__fake__echo".to_string()));
    // Mode `a` selects only mcp__fake__*; the ACP-provided `extra` server is not selected.
    assert!(!names.iter().any(|n| n.starts_with("mcp__extra__")));
    assert_eq!(result_text(&reqs[1]), "echo: hi");
    assert_eq!(result_text(&reqs[2]), "boom");
    assert!(result_text(&reqs[3]).contains("unknown tool"));

    // Read-only annotated tool runs without a prompt; the other asks.
    let asked: Vec<_> = client
        .requests
        .lock()
        .unwrap()
        .iter()
        .map(|r| r.title.clone())
        .collect();
    assert_eq!(asked, vec!["fake: fail"]);

    let events = client.events.lock().unwrap();
    assert!(events.iter().any(|e| matches!(e,
        SessionEvent::Notice(n) if n.contains("`broken`") && n.contains("failed to start"))));

    let records: Vec<_> = TranscriptReader::load(&session_path(proj.path(), &info.id))
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.record)
        .collect();
    let servers = records
        .iter()
        .find_map(|r| match r {
            Record::McpServers { servers } => Some(servers.clone()),
            _ => None,
        })
        .expect("mcp_servers record");
    assert_eq!(servers.len(), 3);
    let by = |n: &str| servers.iter().find(|s| s.name == n).unwrap();
    assert!(by("fake").ok && by("fake").tools == 5);
    assert!(by("extra").ok);
    assert!(!by("broken").ok && by("broken").error.is_some());
}

#[tokio::test]
async fn mode_without_mcp_tools_sees_none_and_default_mode_sees_all() {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
    std::fs::write(
        proj.path().join(".ez/config.toml"),
        format!(
            "[mcp]\nlazy_threshold = 3\n[mcp_servers.fake]\ncommand = \"python3\"\nargs = [{:?}, \"0\"]\n",
            fake_script()
        ),
    )
    .unwrap();

    // Implicit default mode selects mcp__*; 5 tools > threshold 3 so lazy meta-tools appear.
    let mock = Arc::new(MockProvider::new([
        tool_use(
            "1",
            "McpCall",
            json!({"tool": "mcp__fake__echo", "arguments": {"text": "lazy"}}),
        ),
        Message::assistant_text("ok"),
    ]));
    let agent = Agent::new(AgentOptions {
        provider_override: Some(mock.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    agent
        .prompt(&info.id, "go".into(), Arc::new(TestClient::default()))
        .await
        .unwrap();
    {
        let reqs = mock.requests.lock().unwrap();
        let names = tool_names(&reqs[0]);
        assert!(
            names.contains(&"McpListTools".to_string()) && names.contains(&"McpCall".to_string())
        );
        assert!(!names.contains(&"mcp__fake__echo".to_string()));
        assert_eq!(result_text(&reqs[1]), "echo: lazy");
    }

    // A mode with no mcp_tools gets nothing.
    std::fs::write(proj.path().join(".ez/modes.toml"), "[modes.b]\n").unwrap();
    let mock = Arc::new(MockProvider::new([Message::assistant_text("ok")]));
    let agent = Agent::new(AgentOptions {
        provider_override: Some(mock.clone()),
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    agent
        .prompt(&info.id, "go".into(), Arc::new(TestClient::default()))
        .await
        .unwrap();
    let names = tool_names(&mock.requests.lock().unwrap()[0]);
    assert!(
        !names
            .iter()
            .any(|n| n.contains("mcp") || n.starts_with("Mcp"))
    );
}
