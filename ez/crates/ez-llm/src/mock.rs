//! Mock provider for tests and offline demos.
//!
//! Two modes:
//!
//! * **Scripted** ([`MockProvider::new`]): returns queued assistant messages in order and
//!   records every request. When the queue is empty it answers
//!   `(mock: no more scripted responses)`.
//! * **Demo** ([`MockProvider::demo`], used by `provider = "mock"` profiles): a deterministic
//!   model that reacts to the *last user message*, so interactive flows (questions, permission
//!   prompts, todos, mode switches) can be exercised without an API key.
//!
//! Demo commands (case-insensitive prefix `mock:`; the phrase "ask me" also works):
//!
//! | Prompt                         | Reply                                                      |
//! |--------------------------------|------------------------------------------------------------|
//! | `mock:ask` / "ask me"          | `AskUserQuestion` with a single-select and a multi-select  |
//! | `mock:write <path> <text>`     | `Write` tool call                                          |
//! | `mock:edit <path> <old> => <new>` | `Edit` tool call                                        |
//! | `mock:bash <cmd>`              | `Bash` tool call                                           |
//! | `mock:todo`                    | `TodoWrite` with 3 items                                   |
//! | `mock:mode <id>`               | `SwitchMode` tool call with a reason                       |
//! | `mock:think`                   | a thinking block (signature `mock`) plus text              |
//! | `mock:tool <Name> <json>`      | arbitrary tool call                                        |
//! | anything else                  | help text echoing the prompt, streamed in chunks           |
//!
//! A tool is only called when the request offers it; otherwise the reply is text saying so.
//! After tool results arrive the model summarizes them ("You chose: ...",
//! "Tool `X` returned: ...", "Tool failed: ...") and ends the turn.
//! `EZ_MOCK_DELAY_MS` (default 0) delays each streamed chunk.

use crate::{
    ContentBlock, Message, Provider, Request, Response, Role, StopReason, StreamEvent, Usage,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// Returns queued assistant messages in order (or demo replies) and records every request.
#[derive(Default)]
pub struct MockProvider {
    responses: Mutex<VecDeque<Message>>,
    pub requests: Mutex<Vec<Request>>,
    demo: bool,
    counter: AtomicU64,
}

impl MockProvider {
    pub fn new(responses: impl IntoIterator<Item = Message>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            ..Default::default()
        }
    }

    /// Deterministic demo model; see the module docs for the command syntax.
    pub fn demo() -> Self {
        Self {
            demo: true,
            ..Default::default()
        }
    }

    pub fn push(&self, m: Message) {
        self.responses.lock().unwrap().push_back(m);
    }

    fn next_id(&self) -> String {
        format!("mock_{}", self.counter.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn tool(&self, request: &Request, name: &str, input: Value) -> Message {
        if !request.tools.iter().any(|t| t.name == name) {
            return Message::assistant_text(format!(
                "The `{name}` tool is not available in this mode, so I can't demo it here."
            ));
        }
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: self.next_id(),
                name: name.into(),
                input,
            }],
        }
    }

    fn demo_reply(&self, request: &Request) -> Message {
        let Some(last) = request.messages.iter().rev().find(|m| m.role == Role::User) else {
            return Message::assistant_text(help_text(""));
        };
        let results: Vec<&ContentBlock> = last
            .content
            .iter()
            .filter(|b| matches!(b, ContentBlock::ToolResult { .. }))
            .collect();
        if !results.is_empty() {
            return Message::assistant_text(summarize_results(request, &results));
        }
        let prompt = last.text();
        let trimmed = prompt.trim();
        let (cmd, rest) = match strip_prefix_ci(trimmed, "mock:") {
            Some(r) => {
                let r = r.trim_start();
                match r.split_once(char::is_whitespace) {
                    Some((c, a)) => (c.to_ascii_lowercase(), a.trim()),
                    None => (r.to_ascii_lowercase(), ""),
                }
            }
            None if trimmed.to_ascii_lowercase().contains("ask me") => ("ask".into(), ""),
            None => (String::new(), ""),
        };
        match cmd.as_str() {
            "ask" => self.tool(request, "AskUserQuestion", ask_input()),
            "write" => match rest.split_once(char::is_whitespace) {
                Some((path, text)) => self.tool(
                    request,
                    "Write",
                    json!({"file_path": path, "content": text.trim()}),
                ),
                None => Message::assistant_text("usage: mock:write <path> <text>"),
            },
            "edit" => match rest
                .split_once(char::is_whitespace)
                .and_then(|(p, r)| r.split_once("=>").map(|(o, n)| (p, o.trim(), n.trim())))
            {
                Some((path, old, new)) => self.tool(
                    request,
                    "Edit",
                    json!({"file_path": path, "old_string": old, "new_string": new}),
                ),
                None => Message::assistant_text("usage: mock:edit <path> <old> => <new>"),
            },
            "bash" if !rest.is_empty() => self.tool(request, "Bash", json!({"command": rest})),
            "bash" => Message::assistant_text("usage: mock:bash <cmd>"),
            "todo" => self.tool(
                request,
                "TodoWrite",
                json!({"todos": [
                    {"content": "Read the code", "status": "completed"},
                    {"content": "Make the change", "status": "in_progress"},
                    {"content": "Run the tests", "status": "pending"},
                ]}),
            ),
            "mode" if !rest.is_empty() => self.tool(
                request,
                "SwitchMode",
                json!({"mode_id": rest, "reason": "The mock model was asked to switch modes."}),
            ),
            "mode" => Message::assistant_text("usage: mock:mode <id>"),
            "think" => Message {
                role: Role::Assistant,
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "Let me think about this for a moment. The user wants to see a thinking block.".into(),
                        signature: Some("mock".into()),
                    },
                    ContentBlock::Text {
                        text: "I thought about it, and here is my answer.".into(),
                    },
                ],
            },
            "tool" => {
                let (name, json_src) = match rest.split_once(char::is_whitespace) {
                    Some((n, j)) => (n, j.trim()),
                    None => (rest, "{}"),
                };
                if name.is_empty() {
                    return Message::assistant_text("usage: mock:tool <Name> <json>");
                }
                match serde_json::from_str::<Value>(json_src) {
                    Ok(v) => self.tool(request, name, v),
                    Err(e) => Message::assistant_text(format!("invalid JSON for `{name}`: {e}")),
                }
            }
            _ => Message::assistant_text(help_text(trimmed)),
        }
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len())
        .filter(|h| h.eq_ignore_ascii_case(prefix))
        .map(|_| &s[prefix.len()..])
}

fn ask_input() -> Value {
    json!({"questions": [
        {
            "question": "Which approach should I take?",
            "header": "Approach",
            "multiSelect": false,
            "options": [
                {"label": "Incremental (Recommended)", "description": "Small, reviewable steps"},
                {"label": "Rewrite", "description": "Start over with a clean design"},
                {"label": "Leave as is", "description": "Make no changes"},
            ]
        },
        {
            "question": "Which checks should I run?",
            "header": "Checks",
            "multiSelect": true,
            "options": [
                {"label": "Unit tests", "description": "cargo test"},
                {"label": "Lints", "description": "cargo clippy"},
                {"label": "Formatting", "description": "cargo fmt --check"},
            ]
        },
    ]})
}

fn help_text(prompt: &str) -> String {
    let mut s = String::new();
    if !prompt.is_empty() {
        s.push_str(&format!(
            "You said: {prompt}

"
        ));
    }
    s.push_str(
        "I am the ez mock model. Try one of these commands:\n\
         - mock:ask (or \"ask me\"): ask you two questions\n\
         - mock:write <path> <text>: write a file (permission prompt)\n\
         - mock:edit <path> <old> => <new>: edit a file\n\
         - mock:bash <cmd>: run a shell command\n\
         - mock:todo: write a todo list\n\
         - mock:mode <id>: propose a mode switch\n\
         - mock:think: show a thinking block\n\
         - mock:tool <Name> <json>: call any tool",
    );
    s
}

fn summarize_results(request: &Request, results: &[&ContentBlock]) -> String {
    let tool_name = |id: &str| -> String {
        request
            .messages
            .iter()
            .flat_map(|m| m.tool_uses())
            .find(|(i, _, _)| *i == id)
            .map(|(_, n, _)| n.to_string())
            .unwrap_or_else(|| "?".into())
    };
    results
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some(if *is_error {
                format!("Tool failed: {content}")
            } else {
                let name = tool_name(tool_use_id);
                match (name.as_str(), content.strip_prefix("The user answered:")) {
                    ("AskUserQuestion", Some(rest)) => format!("You chose:{rest}"),
                    _ => format!("Tool `{name}` returned: {content}"),
                }
            }),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split into a few chunks on char boundaries (about 4 pieces, at least 12 chars each).
fn chunks(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let size = (chars.len() / 4).max(12);
    chars.chunks(size).map(|c| c.iter().collect()).collect()
}

fn delay() -> Duration {
    Duration::from_millis(
        std::env::var("EZ_MOCK_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    )
}

#[async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        let demo_msg = self.demo.then(|| self.demo_reply(&request));
        self.requests.lock().unwrap().push(request);
        let message = demo_msg.unwrap_or_else(|| {
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Message::assistant_text("(mock: no more scripted responses)"))
        });
        assert_eq!(message.role, Role::Assistant);
        if self.demo {
            let d = delay();
            for b in &message.content {
                match b {
                    ContentBlock::Thinking { thinking, .. } => {
                        for c in chunks(thinking) {
                            let _ = events.send(StreamEvent::ThinkingDelta(c));
                            tokio::time::sleep(d).await;
                        }
                    }
                    ContentBlock::Text { text } => {
                        for c in chunks(text) {
                            let _ = events.send(StreamEvent::TextDelta(c));
                            tokio::time::sleep(d).await;
                        }
                    }
                    ContentBlock::ToolUse { id, name, .. } => {
                        let _ = events.send(StreamEvent::ToolUseStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                    }
                    _ => {}
                }
            }
        } else {
            let text = message.text();
            if !text.is_empty() {
                let _ = events.send(StreamEvent::TextDelta(text));
            }
        }
        let stop_reason = if message.tool_uses().next().is_some() {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(Response {
            message,
            stop_reason,
            usage: Usage::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> crate::ToolSpec {
        crate::ToolSpec {
            name: name.into(),
            description: String::new(),
            input_schema: json!({}),
        }
    }

    fn req(msgs: Vec<Message>, tools: &[&str]) -> Request {
        Request {
            model: "m".into(),
            system: String::new(),
            messages: msgs,
            tools: tools.iter().map(|t| spec(t)).collect(),
            max_tokens: 100,
            thinking: None,
        }
    }

    async fn run(p: &MockProvider, r: Request) -> (Response, Vec<StreamEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let resp = p.complete(r, tx).await.unwrap();
        let mut ev = vec![];
        while let Ok(e) = rx.try_recv() {
            ev.push(e);
        }
        (resp, ev)
    }

    async fn ask(prompt: &str, tools: &[&str]) -> Response {
        run(
            &MockProvider::demo(),
            req(vec![Message::user_text(prompt)], tools),
        )
        .await
        .0
    }

    fn only_tool(r: &Response) -> (String, Value) {
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        let (_, n, i) = r.message.tool_uses().next().unwrap();
        (n.to_string(), i.clone())
    }

    #[tokio::test]
    async fn scripted_mode_unchanged() {
        let p = MockProvider::new([Message::assistant_text("one")]);
        let (r, ev) = run(&p, req(vec![Message::user_text("mock:ask")], &[])).await;
        assert_eq!(r.message.text(), "one");
        assert_eq!(ev, vec![StreamEvent::TextDelta("one".into())]);
        let (r, _) = run(&p, req(vec![Message::user_text("x")], &[])).await;
        assert_eq!(r.message.text(), "(mock: no more scripted responses)");
    }

    #[tokio::test]
    async fn ask_command() {
        for prompt in ["mock:ask", "MOCK:ASK", "please ask me something"] {
            let r = ask(prompt, &["AskUserQuestion"]).await;
            let (name, input) = only_tool(&r);
            assert_eq!(name, "AskUserQuestion");
            let qs = input["questions"].as_array().unwrap();
            assert_eq!(qs.len(), 2);
            assert_eq!(qs[0]["multiSelect"], false);
            assert_eq!(qs[1]["multiSelect"], true);
            for q in qs {
                assert_eq!(q["options"].as_array().unwrap().len(), 3);
                assert!(q["header"].as_str().unwrap().chars().count() <= 12);
            }
            assert!(
                qs[0]["options"][0]["label"]
                    .as_str()
                    .unwrap()
                    .ends_with("(Recommended)")
            );
        }
        let r = ask("mock:ask", &["Write"]).await;
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        assert!(r.message.text().contains("not available"));
    }

    #[tokio::test]
    async fn tool_commands() {
        let all = ["Write", "Edit", "Bash", "TodoWrite", "SwitchMode", "Read"];
        let r = ask("mock:write a/b.txt hello world", &all).await;
        assert_eq!(
            only_tool(&r),
            (
                "Write".into(),
                json!({"file_path":"a/b.txt","content":"hello world"})
            )
        );
        let r = ask("mock:edit f.txt foo => bar baz", &all).await;
        assert_eq!(
            only_tool(&r).1,
            json!({"file_path":"f.txt","old_string":"foo","new_string":"bar baz"})
        );
        let r = ask("mock:bash echo hi", &all).await;
        assert_eq!(only_tool(&r), ("Bash".into(), json!({"command":"echo hi"})));
        let r = ask("mock:todo", &all).await;
        let (n, i) = only_tool(&r);
        assert_eq!(n, "TodoWrite");
        assert_eq!(i["todos"].as_array().unwrap().len(), 3);
        let r = ask("mock:mode build", &all).await;
        let (n, i) = only_tool(&r);
        assert_eq!(n, "SwitchMode");
        assert_eq!(i["mode_id"], "build");
        assert!(!i["reason"].as_str().unwrap().is_empty());
        let r = ask("mock:tool Read {\"file_path\":\"x\"}", &all).await;
        assert_eq!(only_tool(&r), ("Read".into(), json!({"file_path":"x"})));
        // Unavailable tool and bad input degrade to text.
        assert!(
            ask("mock:bash ls", &["Read"])
                .await
                .message
                .text()
                .contains("not available")
        );
        assert!(
            ask("mock:tool Read {oops", &all)
                .await
                .message
                .text()
                .contains("invalid JSON")
        );
        assert!(
            ask("mock:write", &all)
                .await
                .message
                .text()
                .contains("usage")
        );
    }

    #[tokio::test]
    async fn tool_ids_unique() {
        let p = MockProvider::demo();
        let mut ids = vec![];
        for _ in 0..3 {
            let (r, _) = run(&p, req(vec![Message::user_text("mock:bash ls")], &["Bash"])).await;
            ids.push(r.message.tool_uses().next().unwrap().0.to_string());
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 3);
    }

    #[tokio::test]
    async fn think_command() {
        let (r, ev) = run(
            &MockProvider::demo(),
            req(vec![Message::user_text("mock:think")], &[]),
        )
        .await;
        assert!(matches!(&r.message.content[0],
            ContentBlock::Thinking { signature, .. } if signature.as_deref() == Some("mock")));
        assert!(!r.message.text().is_empty());
        assert!(
            ev.iter()
                .any(|e| matches!(e, StreamEvent::ThinkingDelta(_)))
        );
        assert!(ev.iter().any(|e| matches!(e, StreamEvent::TextDelta(_))));
    }

    #[tokio::test]
    async fn tool_results_are_summarized() {
        let p = MockProvider::demo();
        let (r, _) = run(
            &p,
            req(
                vec![Message::user_text("mock:ask")],
                &["AskUserQuestion", "Bash"],
            ),
        )
        .await;
        let id = r.message.tool_uses().next().unwrap().0.to_string();
        let msgs = |content: &str, is_error| {
            vec![
                Message::user_text("mock:ask"),
                r.message.clone(),
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: content.into(),
                        is_error,
                    }],
                },
            ]
        };
        let (a, _) = run(&p, req(msgs("The user answered:\n- Q: A", false), &[])).await;
        assert_eq!(a.stop_reason, StopReason::EndTurn);
        assert!(a.message.text().starts_with("You chose:"));
        assert!(a.message.text().contains("- Q: A"));
        let (a, _) = run(&p, req(msgs("boom", true), &[])).await;
        assert_eq!(a.message.text(), "Tool failed: boom");
        // A tool result for a non-ask tool.
        let mut m = msgs("out", false);
        if let ContentBlock::ToolUse { name, .. } = &mut m[1].content[0] {
            *name = "Bash".into();
        }
        let (a, _) = run(&p, req(m, &[])).await;
        assert_eq!(a.message.text(), "Tool `Bash` returned: out");
    }

    #[tokio::test]
    async fn fallback_streams_in_chunks() {
        let (r, ev) = run(
            &MockProvider::demo(),
            req(vec![Message::user_text("hello there")], &[]),
        )
        .await;
        let text = r.message.text();
        assert!(text.contains("You said: hello there"));
        assert!(text.contains("mock:ask"));
        let deltas: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                StreamEvent::TextDelta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(deltas.len() >= 3);
        assert_eq!(deltas.concat(), text);
    }
}
