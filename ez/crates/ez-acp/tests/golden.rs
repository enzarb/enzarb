//! Golden wire tests.
//!
//! `golden/baseline.json` was captured from the hand-written schema (before the migration to the
//! official `agent-client-protocol-schema` crate). The migrated agent must produce the same JSON,
//! except for the differences listed in [`intended_differences`].
//!
//! `GOLDEN_UPDATE=1 cargo test -p ez-acp --test golden` writes the current output to
//! `golden/current.json` for inspection; `GOLDEN_UPDATE=baseline` overwrites the baseline (only
//! do that when the wire format changes on purpose).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ez_acp::{Handler, RpcError, spawn_in_process};
use ez_core::{Agent, AgentOptions};
use ez_llm::mock::MockProvider;
use ez_llm::{ContentBlock as LlmBlock, Message, Role};
use serde_json::{Value, json};

#[derive(Default)]
struct Recorder {
    log: Mutex<Vec<Value>>,
}

#[async_trait]
impl Handler for Recorder {
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if method == "session/request_permission" {
            self.log
                .lock()
                .unwrap()
                .push(json!({"agent_request": method, "params": params}));
            return Ok(json!({"outcome":{"outcome":"selected","optionId":"allow_once"}}));
        }
        Err(RpcError::method_not_found(method))
    }
    async fn notification(&self, method: &str, params: Value) {
        self.log
            .lock()
            .unwrap()
            .push(json!({"agent_notification": method, "params": params}));
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

fn harness(
    proj: &std::path::Path,
    user: &std::path::Path,
    script: Vec<Message>,
) -> (ez_acp::Connection, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    let agent = Arc::new(Agent::new(AgentOptions {
        provider_override: Some(Arc::new(MockProvider::new(script))),
        user_config_dir: Some(user.to_path_buf()),
        ..Default::default()
    }));
    let _ = proj;
    (spawn_in_process(agent, rec.clone()), rec)
}

/// Replace volatile values (session id, temp dirs, timestamps) with placeholders.
fn normalize(v: &Value, subs: &[(String, &str)]) -> Value {
    let mut s = serde_json::to_string(v).unwrap();
    for (from, to) in subs {
        s = s.replace(from.as_str(), to);
    }
    let mut v: Value = serde_json::from_str(&s).unwrap();
    scrub_timestamps(&mut v);
    v
}

fn scrub_timestamps(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "createdAt" || k == "updatedAt" {
                    *x = json!("TS");
                } else {
                    scrub_timestamps(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub_timestamps),
        _ => {}
    }
}

async fn capture() -> Value {
    let proj = tempfile::tempdir().unwrap();
    let user = tempfile::tempdir().unwrap();
    let cwd: PathBuf = proj.path().canonicalize().unwrap();
    std::fs::create_dir_all(cwd.join(".ez")).unwrap();
    std::fs::write(
        cwd.join(".ez/modes.toml"),
        "default_mode = \"a\"\n[modes.a]\ndescription = \"Mode A\"\n[modes.b]\ndescription = \"\"\n",
    )
    .unwrap();
    let file = cwd.join("out.txt");
    let script = vec![
        tool_use(
            "t1",
            "Write",
            json!({"file_path": file, "content": "new body"}),
        ),
        tool_use(
            "t2",
            "TodoWrite",
            json!({"todos":[
                {"content":"first","status":"in_progress","activeForm":"Doing first"},
                {"content":"second","status":"pending","activeForm":"Doing second"}]}),
        ),
        // Writes nothing and prints nothing: tool_call_update without output or edits.
        tool_use("t3", "Bash", json!({"command":"true"})),
        Message::assistant_text("all done"),
    ];
    let (c, rec) = harness(&cwd, user.path(), script);

    let init = c
        .request(
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true}}}),
        )
        .await
        .unwrap();
    let new = c
        .request("session/new", json!({"cwd":cwd,"mcpServers":[]}))
        .await
        .unwrap();
    let sid = new["sessionId"].as_str().unwrap().to_string();
    let prompt = c
        .request(
            "session/prompt",
            json!({"sessionId":sid,"prompt":[{"type":"text","text":"do things"}]}),
        )
        .await
        .unwrap();
    c.request("session/set_mode", json!({"sessionId":sid,"modeId":"b"}))
        .await
        .unwrap();
    c.request(
        "_ez/set_model",
        json!({"sessionId":sid,"model":"pinned-model"}),
    )
    .await
    .unwrap();
    let list = c
        .request("_ez/list_sessions", json!({"cwd":cwd}))
        .await
        .unwrap();
    let live_events = rec.log.lock().unwrap().clone();

    // Replay: a fresh agent loads the persisted transcript.
    let user2 = tempfile::tempdir().unwrap();
    let (c2, rec2) = harness(&cwd, user2.path(), vec![]);
    c2.request("initialize", json!({"protocolVersion":1}))
        .await
        .unwrap();
    let load = c2
        .request(
            "session/load",
            json!({"sessionId":sid,"cwd":cwd,"mcpServers":[]}),
        )
        .await
        .unwrap();
    let replay_events = rec2.log.lock().unwrap().clone();

    let subs = vec![(sid.clone(), "SID"), (cwd.display().to_string(), "CWD")];
    let n = |v: &Value| normalize(v, &subs);
    json!({
        "initialize": n(&init),
        "session_new": n(&new),
        "prompt_response": n(&prompt),
        "live_events": n(&Value::Array(live_events)),
        "list_sessions_ext": n(&list),
        "session_load": n(&load),
        "replay_events": n(&Value::Array(replay_events)),
    })
}

/// Intended wire differences between the hand-written schema and the official one. Applied to
/// the baseline so the comparison below is exact.
fn intended_differences(mut b: Value) -> Value {
    // 1. initialize: the official schema always serializes `sessionCapabilities` and `auth`;
    //    we additionally advertise `session/list`, `session/resume`, `session/close`.
    b["initialize"]["agentCapabilities"]["sessionCapabilities"] =
        json!({"list": {}, "resume": {}, "close": {}});
    b["initialize"]["agentCapabilities"]["auth"] = json!({});
    // 2. session/new and session/load return `configOptions` with the `model` selector.
    //    (Injected by the caller of this function; see `strip_model_option`.)
    // 3. tool_call_update without output or edits omits `content` instead of sending `[]`
    //    (an empty array would clear the content the client already shows).
    // 4. diffs of new files omit `oldText` instead of sending `"oldText": null`.
    // 5. New notifications `usage_update` (after each model request) and `config_option_update`
    //    (model changes) are stripped before comparing; see agent_server.rs for their tests.
    for key in ["live_events", "replay_events"] {
        for ev in b[key].as_array_mut().unwrap() {
            if ev.get("agent_notification").is_none() {
                continue;
            }
            let upd = &mut ev["params"]["update"];
            // 6. `tool_call` omits `status: "pending"` and `kind: "other"`: the official
            //    serializer skips values equal to the spec defaults.
            if upd["sessionUpdate"] == "tool_call" {
                let m = upd.as_object_mut().unwrap();
                if m.get("status") == Some(&json!("pending")) {
                    m.remove("status");
                }
                if m.get("kind") == Some(&json!("other")) {
                    m.remove("kind");
                }
            }
            if upd["sessionUpdate"] == "tool_call_update" {
                if upd["content"] == json!([]) {
                    upd.as_object_mut().unwrap().remove("content");
                }
                if let Some(arr) = upd.get_mut("content").and_then(Value::as_array_mut) {
                    for c in arr {
                        if c["type"] == "diff" && c["oldText"].is_null() {
                            c.as_object_mut().unwrap().remove("oldText");
                        }
                    }
                }
            }
        }
    }
    b
}

/// Remove the intended additions that are covered by dedicated tests: `configOptions`
/// (session/new, session/load), `usage_update` and `config_option_update` notifications.
fn strip_additions(mut a: Value) -> Value {
    for k in ["session_new", "session_load"] {
        a[k].as_object_mut().unwrap().remove("configOptions");
    }
    for key in ["live_events", "replay_events"] {
        let evs = a[key].as_array_mut().unwrap();
        evs.retain(|e| {
            !matches!(
                e["params"]["update"]["sessionUpdate"].as_str(),
                Some("usage_update" | "config_option_update")
            )
        });
    }
    a
}

#[tokio::test]
async fn wire_matches_baseline_except_intended_differences() {
    let actual = capture().await;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    if std::env::var_os("GOLDEN_UPDATE").is_some() {
        let name = std::env::var("GOLDEN_UPDATE").unwrap();
        let name = if name == "baseline" {
            "baseline"
        } else {
            "current"
        };
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::to_string_pretty(&actual).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    let baseline: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("baseline.json")).unwrap()).unwrap();
    let expected = intended_differences(baseline);
    let actual = strip_additions(actual);
    for key in expected.as_object().unwrap().keys() {
        if let (Some(a), Some(e)) = (actual[key].as_array(), expected[key].as_array()) {
            assert_eq!(
                a.len(),
                e.len(),
                "golden `{key}`: different number of items"
            );
            for (i, (a, e)) in a.iter().zip(e).enumerate() {
                assert_eq!(a, e, "golden mismatch in `{key}[{i}]`");
            }
        }
        assert_eq!(actual[key], expected[key], "golden mismatch in `{key}`");
    }
}
