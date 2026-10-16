//! Chat capture (ADR-0014 chats, contracts-v2 "Chat"): the proxy relays the browser's `acp`
//! channel, so it records each chat as `engine.chats` plus `session_events` of run
//! `chat-<ACP sessionId>` through `engine_api.chat_record` / `engine_api.chat_event`. Never
//! alters or delays frames: parsing is best effort and writes are queued to one writer task.

use std::sync::Arc;

use async_trait::async_trait;
use enzarb_proto::{FrameHeader, FrameType, HEADER_LEN};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc;

/// Where chat records go.
#[async_trait]
pub trait ChatSink: Send + Sync {
    async fn record(
        &self,
        run_id: &str,
        project: &str,
        user: &str,
        title: &str,
    ) -> anyhow::Result<()>;
    async fn event(&self, run_id: &str, kind: &str, payload: &Value) -> anyhow::Result<()>;
}

/// Postgres sink (role `enzarb_sandbox_proxy`, EXECUTE on the two functions).
pub struct PgChats {
    pub pool: PgPool,
}

#[async_trait]
impl ChatSink for PgChats {
    async fn record(
        &self,
        run_id: &str,
        project: &str,
        user: &str,
        title: &str,
    ) -> anyhow::Result<()> {
        sqlx::query("SELECT engine_api.chat_record($1, $2, $3, $4)")
            .bind(run_id)
            .bind(project)
            .bind(user)
            .bind(title)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    async fn event(&self, run_id: &str, kind: &str, payload: &Value) -> anyhow::Result<()> {
        sqlx::query("SELECT engine_api.chat_event($1, $2, $3)")
            .bind(run_id)
            .bind(kind)
            .bind(payload)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[derive(Debug)]
enum Op {
    Record {
        run_id: String,
        title: String,
    },
    Event {
        run_id: String,
        kind: String,
        payload: Value,
    },
}

/// Per-socket recorder. `observe` is cheap and synchronous.
pub struct ChatRecorder {
    tx: mpsc::UnboundedSender<Op>,
    up: Vec<u8>,
    down: Vec<u8>,
    /// Ids of `session/new` requests awaiting their response.
    pending_new: Vec<Value>,
    session: Option<String>,
    titled: bool,
}

impl ChatRecorder {
    /// Starts the writer task for one socket.
    pub fn start(sink: Arc<dyn ChatSink>, project: String, user: String) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Op>();
        tokio::spawn(async move {
            while let Some(op) = rx.recv().await {
                let r = match &op {
                    Op::Record { run_id, title } => {
                        sink.record(run_id, &project, &user, title).await
                    }
                    Op::Event {
                        run_id,
                        kind,
                        payload,
                    } => sink.event(run_id, kind, payload).await,
                };
                if let Err(e) = r {
                    tracing::warn!(project, error = %e, "chat capture write failed");
                }
            }
        });
        Self {
            tx,
            up: Vec::new(),
            down: Vec::new(),
            pending_new: Vec::new(),
            session: None,
            titled: false,
        }
    }

    /// One mux message in either direction; only data frames of `acp_channel` are read.
    pub fn observe(&mut self, from_browser: bool, msg: &[u8], acp_channel: Option<u32>) {
        let Some(ch) = acp_channel else { return };
        let Ok(h) = FrameHeader::parse(msg) else {
            return;
        };
        if h.channel != ch || !matches!(h.frame_type, FrameType::DataBin | FrameType::DataJson) {
            return;
        }
        let Some(payload) = msg.get(HEADER_LEN..HEADER_LEN + h.len) else {
            return;
        };
        let buf = if from_browser {
            &mut self.up
        } else {
            &mut self.down
        };
        buf.extend_from_slice(payload);
        // Bounded: a runaway line without newline is dropped.
        if buf.len() > 8 << 20 {
            buf.clear();
            return;
        }
        let mut lines = Vec::new();
        while let Some(i) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=i).collect();
            lines.push(line);
        }
        for line in lines {
            if let Ok(v) = serde_json::from_slice::<Value>(&line) {
                self.message(from_browser, v);
            }
        }
    }

    fn message(&mut self, from_browser: bool, v: Value) {
        let method = v.get("method").and_then(Value::as_str).unwrap_or("");
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        if from_browser {
            match method {
                "session/new" => {
                    if let Some(id) = v.get("id") {
                        self.pending_new.push(id.clone());
                    }
                }
                "session/load" => {
                    if let Some(s) = params.get("sessionId").and_then(Value::as_str) {
                        self.start_session(s);
                    }
                }
                "session/prompt" => {
                    if let Some(s) = params.get("sessionId").and_then(Value::as_str) {
                        self.start_session(s);
                    }
                    let text = redact(&prompt_text(&params));
                    if !self.titled && !text.is_empty() {
                        self.titled = true;
                        if let Some(run_id) = self.run_id() {
                            let title: String = text
                                .lines()
                                .next()
                                .unwrap_or("")
                                .chars()
                                .take(120)
                                .collect();
                            let _ = self.tx.send(Op::Record { run_id, title });
                        }
                    }
                    self.emit("user", json!({"text": text}));
                }
                "session/cancel" => self.emit("system", json!({"event": "cancel"})),
                _ => {}
            }
        } else if method == "session/update" {
            let update = params.get("update").cloned().unwrap_or(Value::Null);
            let kind = match update.get("sessionUpdate").and_then(Value::as_str) {
                Some("agent_message_chunk") => "assistant",
                Some("agent_thought_chunk") => "thinking",
                Some("user_message_chunk") => "user",
                Some("tool_call") => "tool_call",
                Some("tool_call_update") => "tool_result",
                _ => "system",
            };
            self.emit(kind, redact_value(update));
        } else if method.is_empty()
            && let Some(id) = v.get("id")
            && let Some(i) = self.pending_new.iter().position(|p| p == id)
        {
            self.pending_new.remove(i);
            if let Some(s) = v.pointer("/result/sessionId").and_then(Value::as_str) {
                self.start_session(s);
            }
        }
    }

    fn run_id(&self) -> Option<String> {
        self.session.as_ref().map(|s| format!("chat-{s}"))
    }

    fn start_session(&mut self, id: &str) {
        if self.session.as_deref() == Some(id) {
            return;
        }
        self.session = Some(id.to_string());
        self.titled = false;
        let _ = self.tx.send(Op::Record {
            run_id: format!("chat-{id}"),
            title: String::new(),
        });
    }

    fn emit(&self, kind: &str, payload: Value) {
        if let Some(run_id) = self.run_id() {
            let _ = self.tx.send(Op::Event {
                run_id,
                kind: kind.into(),
                payload,
            });
        }
    }
}

fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Claude tokens never reach the store.
fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("sk-ant-") {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(tail.len());
        out.push_str("[redacted]");
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

fn redact_value(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(redact(&s)),
        Value::Array(a) => Value::Array(a.into_iter().map(redact_value).collect()),
        Value::Object(o) => {
            Value::Object(o.into_iter().map(|(k, v)| (k, redact_value(v))).collect())
        }
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Mem(Mutex<Vec<String>>);

    #[async_trait]
    impl ChatSink for Mem {
        async fn record(
            &self,
            run_id: &str,
            project: &str,
            user: &str,
            title: &str,
        ) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(format!("record {run_id} {project} {user} {title}"));
            Ok(())
        }
        async fn event(&self, run_id: &str, kind: &str, payload: &Value) -> anyhow::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push(format!("event {run_id} {kind} {payload}"));
            Ok(())
        }
    }

    fn frame(ch: u32, s: &str) -> Vec<u8> {
        let mut b = bytes::BytesMut::new();
        enzarb_proto::Frame {
            frame_type: FrameType::DataBin,
            flags: 0,
            channel: ch,
            payload: bytes::Bytes::copy_from_slice(s.as_bytes()),
        }
        .encode(&mut b)
        .unwrap();
        b.to_vec()
    }

    #[tokio::test]
    async fn records_a_chat_from_the_acp_channel() {
        let sink = Arc::new(Mem::default());
        let mut r = ChatRecorder::start(sink.clone(), "web".into(), "octo".into());
        r.observe(
            true,
            &frame(
                3,
                "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"session/new\",\"params\":{}}\n",
            ),
            Some(3),
        );
        // Split across frames.
        r.observe(
            false,
            &frame(3, "{\"jsonrpc\":\"2.0\",\"id\":1,\"res"),
            Some(3),
        );
        r.observe(false, &frame(3, "ult\":{\"sessionId\":\"s1\"}}\n"), Some(3));
        r.observe(true, &frame(3, "{\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"s1\",\"prompt\":[{\"type\":\"text\",\"text\":\"Fix the bug sk-ant-oat01-SECRET\"}]}}\n"), Some(3));
        r.observe(false, &frame(3, "{\"method\":\"session/update\",\"params\":{\"sessionId\":\"s1\",\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{\"type\":\"text\",\"text\":\"ok\"}}}}\n"), Some(3));
        // Other channels are ignored.
        r.observe(
            false,
            &frame(4, "{\"method\":\"session/update\",\"params\":{}}\n"),
            Some(3),
        );
        drop(r);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let got = sink.0.lock().unwrap().clone();
        assert_eq!(got[0], "record chat-s1 web octo ");
        assert!(
            got[1].starts_with("record chat-s1 web octo Fix the bug [redacted]"),
            "{got:?}"
        );
        assert!(got[2].starts_with("event chat-s1 user"), "{got:?}");
        assert!(!got[2].contains("SECRET"));
        assert!(got[3].starts_with("event chat-s1 assistant"), "{got:?}");
        assert_eq!(got.len(), 4);
    }
}
