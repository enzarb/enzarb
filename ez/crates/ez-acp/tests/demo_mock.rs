//! End-to-end: a real Agent driven by the demo mock model (`MockProvider::demo()`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ez_acp::schema::*;
use ez_acp::{Connection, Handler, RpcError, spawn_in_process};
use ez_core::{Agent, AgentOptions};
use ez_llm::mock::MockProvider;
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Default)]
struct Client {
    updates: Mutex<Vec<SessionNotification>>,
    perms: Mutex<Vec<RequestPermissionRequest>>,
    /// Pick the first option of this kind (None = cancel).
    pick: Mutex<Option<PermissionOptionKind>>,
    /// For fallback questions: option index per question.
    ask_pick: Mutex<Vec<usize>>,
    ask_reqs: Mutex<Vec<Value>>,
}

#[async_trait]
impl Handler for Client {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            SESSION_REQUEST_PERMISSION => {
                let req: RequestPermissionRequest = serde_json::from_value(params).unwrap();
                let is_question = req.tool_call.tool_call_id.0.starts_with("ask-");
                let chosen = if is_question {
                    let mut p = self.ask_pick.lock().unwrap();
                    let i = if p.is_empty() { 0 } else { p.remove(0) };
                    req.options.get(i).map(|o| o.option_id.to_string())
                } else {
                    let kind = *self.pick.lock().unwrap();
                    kind.and_then(|k| {
                        req.options
                            .iter()
                            .find(|o| o.kind == k)
                            .map(|o| o.option_id.to_string())
                    })
                };
                self.perms.lock().unwrap().push(req);
                Ok(match chosen {
                    Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
                    None => json!({"outcome":{"outcome":"cancelled"}}),
                })
            }
            ASK_USER_QUESTION => {
                self.ask_reqs.lock().unwrap().push(params.clone());
                let qs = &params["questions"];
                Ok(json!({"cancelled": false, "answers": [
                    {"question": qs[0]["question"], "selected": [qs[0]["options"][1]["label"]]},
                    {"question": qs[1]["question"],
                     "selected": [qs[1]["options"][0]["label"], qs[1]["options"][2]["label"]],
                     "other": "custom note"},
                ]}))
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
    client: Arc<Client>,
    conn: Connection,
}

impl Env {
    async fn new(ask_cap: bool) -> (Self, String) {
        let proj = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let client = Arc::new(Client::default());
        let agent = Arc::new(Agent::new(AgentOptions {
            provider_override: Some(Arc::new(MockProvider::demo())),
            user_config_dir: Some(user.path().to_path_buf()),
            ..Default::default()
        }));
        let conn = spawn_in_process(agent, client.clone());
        let caps = if ask_cap {
            json!({"_meta":{"ez":{"askUserQuestion":true}}})
        } else {
            json!({})
        };
        conn.request(
            INITIALIZE,
            json!({"protocolVersion":1,"clientCapabilities":caps}),
        )
        .await
        .unwrap();
        let cwd: PathBuf = proj.path().to_path_buf();
        let r = conn
            .request(SESSION_NEW, json!({"cwd":cwd,"mcpServers":[]}))
            .await
            .unwrap();
        let sid = r["sessionId"].as_str().unwrap().to_string();
        (
            Self {
                proj,
                user,
                client,
                conn,
            },
            sid,
        )
    }

    async fn prompt(&self, sid: &str, text: &str) -> String {
        let before = self.client.updates.lock().unwrap().len();
        let r = self
            .conn
            .request(
                SESSION_PROMPT,
                json!({"sessionId":sid,"prompt":[{"type":"text","text":text}]}),
            )
            .await
            .unwrap();
        assert_eq!(r["stopReason"], "end_turn");
        self.client.updates.lock().unwrap()[before..]
            .iter()
            .filter_map(|n| match &n.update {
                SessionUpdate::AgentMessageChunk(ContentChunk {
                    content: ContentBlock::Text(t),
                    ..
                }) => Some(t.text.clone()),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test]
async fn ask_via_extension() {
    let (env, sid) = Env::new(true).await;
    let text = env.prompt(&sid, "mock:ask").await;
    let asks = env.client.ask_reqs.lock().unwrap().clone();
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0]["questions"].as_array().unwrap().len(), 2);
    assert!(env.client.perms.lock().unwrap().is_empty());
    assert!(text.starts_with("You chose:"), "{text}");
    assert!(text.contains("Rewrite"), "{text}");
    assert!(
        text.contains("Unit tests") && text.contains("Formatting"),
        "{text}"
    );
    assert!(text.contains("custom note"), "{text}");
    let _ = (&env.proj, &env.user);
}

#[tokio::test]
async fn ask_fallback_uses_permission_requests() {
    let (env, sid) = Env::new(false).await;
    *env.client.ask_pick.lock().unwrap() = vec![1, 2];
    let text = env.prompt(&sid, "ask me something").await;
    assert!(env.client.ask_reqs.lock().unwrap().is_empty());
    let perms = env.client.perms.lock().unwrap().clone();
    assert_eq!(perms.len(), 2);
    assert!(text.starts_with("You chose:"), "{text}");
    assert!(text.contains("Rewrite"), "{text}");
}

#[tokio::test]
async fn write_permission_approve_and_reject() {
    let (env, sid) = Env::new(true).await;
    *env.client.pick.lock().unwrap() = Some(PermissionOptionKind::AllowOnce);
    let text = env.prompt(&sid, "mock:write ok.txt hello").await;
    assert_eq!(env.client.perms.lock().unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(env.proj.path().join("ok.txt")).unwrap(),
        "hello"
    );
    assert!(text.contains("Tool `Write` returned"), "{text}");

    *env.client.pick.lock().unwrap() = Some(PermissionOptionKind::RejectOnce);
    let text = env.prompt(&sid, "mock:write no.txt nope").await;
    assert_eq!(env.client.perms.lock().unwrap().len(), 2);
    assert!(!env.proj.path().join("no.txt").exists());
    assert!(text.starts_with("Tool failed:"), "{text}");
}

#[tokio::test]
async fn plain_prompt_and_unavailable_tool() {
    let (env, sid) = Env::new(true).await;
    let text = env.prompt(&sid, "hello").await;
    assert!(text.contains("You said: hello"));
    let text = env.prompt(&sid, "mock:mode nonexistent").await;
    assert!(!text.is_empty());
}
