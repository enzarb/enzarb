//! The claude-code driver against a fake claude-agent-acp (tests/fixtures/fake_claude_acp.py):
//! ez's prompt and tools reach the child, and the child's tool calls run through ez.

use async_trait::async_trait;
use ez_core::agent::{
    Agent, AgentOptions, Client, PermissionDecision, PermissionRequest, SessionEvent, StopReason,
};
use ez_core::transcript::{Record, TranscriptReader};
use ez_tools::{Answer, Question, UserBridge};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

#[derive(Default)]
struct TestClient {
    events: Mutex<Vec<SessionEvent>>,
}

#[async_trait]
impl UserBridge for TestClient {
    async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
        anyhow::bail!("no user")
    }
}

#[async_trait]
impl Client for TestClient {
    async fn request_permission(&self, _: &str, _: PermissionRequest) -> PermissionDecision {
        PermissionDecision::AllowOnce
    }
    fn send_event(&self, _: &str, event: SessionEvent) {
        self.events.lock().unwrap().push(event);
    }
}

const CONFIG: &str = r#"
default_profile = "cc"
[profiles.cc]
provider = "claude-code"
model = "sonnet"
context_window = 200000
"#;

const MODES: &str = r#"
default_mode = "build"
[modes.build]
description = "build things"
prompt = "MODE-BUILD-PROMPT"
tools = ["Read", "Write"]
"#;

fn use_fixture() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_claude_acp.py");
        // SAFETY: set once, before any driver is started, and only read by ez-claude.
        unsafe {
            std::env::set_var(
                ez_claude::COMMAND_ENV,
                format!("python3 {}", script.display()),
            )
        };
    });
}

fn setup() -> (tempfile::TempDir, tempfile::TempDir, Agent) {
    use_fixture();
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
    std::fs::write(proj.path().join(".ez/config.toml"), CONFIG).unwrap();
    std::fs::write(proj.path().join(".ez/modes.toml"), MODES).unwrap();
    let agent = Agent::new(AgentOptions {
        user_config_dir: Some(user.path().to_path_buf()),
        ..Default::default()
    });
    (proj, user, agent)
}

fn records(cwd: &Path, id: &str) -> Vec<Record> {
    TranscriptReader::load(&ez_core::transcript::session_path(cwd, id))
        .unwrap()
        .records
        .into_iter()
        .map(|r| r.record)
        .collect()
}

fn agent_text(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::AgentText(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn runs_ez_tools_for_the_claude_code_child() {
    let (proj, _user, agent) = setup();
    let cwd: PathBuf = proj.path().to_path_buf();
    let info = agent.new_session(cwd.clone(), None).await.unwrap();
    let client = std::sync::Arc::new(TestClient::default());

    let stop = agent
        .prompt(&info.id, "write out.txt".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(stop, StopReason::EndTurn);
    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
        "hello from claude"
    );

    let events = client.events.lock().unwrap().clone();
    let text = agent_text(&events);
    // The fixture reports what it saw: ez's prompt, ez's tools, and each call's outcome.
    assert!(text.contains("prompt=ok"), "{text}");
    assert!(text.contains("tools=Read,Write"), "{text}");
    assert!(text.contains("Write=ok"), "{text}");
    // Bash isn't in the mode's tool list, so ez refuses it even though the child asked.
    assert!(text.contains("Bash=error"), "{text}");
    assert!(events.contains(&SessionEvent::Usage {
        input_tokens: 1234,
        output_tokens: 0,
        context_window: 200000,
    }));
    assert!(
        events.iter().any(
            |e| matches!(e, SessionEvent::ToolCall { title, .. } if title.starts_with("Write "))
        )
    );

    let recs = records(&cwd, &info.id);
    let tools: Vec<(&str, bool)> = recs
        .iter()
        .filter_map(|r| match r {
            Record::ToolResult { name, is_error, .. } => Some((name.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(tools, [("Write", false), ("Bash", true)]);
    let assistant = recs.iter().find_map(|r| match r {
        Record::AssistantMessage { model, usage, .. } => Some((model.clone(), *usage)),
        _ => None,
    });
    let (model, usage) = assistant.expect("assistant message recorded");
    assert_eq!(model, "sonnet");
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 7);

    // The same child serves the next prompt (one Claude Code session per ez session).
    let stop = agent
        .prompt(&info.id, "again".into(), client.clone())
        .await
        .unwrap();
    assert_eq!(stop, StopReason::EndTurn);
    let text = agent_text(&client.events.lock().unwrap());
    assert!(text.contains("prompts=2"), "{text}");
}

#[tokio::test]
async fn reports_a_child_that_rejects_the_session() {
    let (proj, _user, agent) = setup();
    std::fs::write(proj.path().join("FAIL_SESSION_NEW"), "").unwrap();
    let info = agent
        .new_session(proj.path().to_path_buf(), None)
        .await
        .unwrap();
    let client = std::sync::Arc::new(TestClient::default());
    let err = agent
        .prompt(&info.id, "hi".into(), client)
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("session/new"), "{msg}");
    assert!(msg.contains("refusing session"), "{msg}");
}
