//! Anthropic Messages API client (streaming, tool use, prompt caching).

use crate::{
    ContentBlock, Message, PartialToolUse, Provider, Request, Response, Role, StopReason,
    StreamEvent, Usage, http_client, sse,
};
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

pub struct Anthropic {
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl Anthropic {
    pub fn new(api_key: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.unwrap_or_else(|| "https://api.anthropic.com".into()),
            client: http_client(),
        }
    }
}

pub(crate) fn build_body(req: &Request) -> Value {
    let mut messages: Vec<Value> = req.messages.iter().map(message_json).collect();
    // Cache the conversation prefix: mark the last block of the final message.
    if let Some(last) = messages.last_mut()
        && let Some(Value::Object(block)) =
            last["content"].as_array_mut().and_then(|c| c.last_mut())
    {
        block.insert("cache_control".into(), json!({"type": "ephemeral"}));
    }
    let mut tools: Vec<Value> = req
        .tools
        .iter()
        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.input_schema}))
        .collect();
    if let Some(Value::Object(last)) = tools.last_mut() {
        last.insert("cache_control".into(), json!({"type": "ephemeral"}));
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "messages": messages,
        "system": [{"type": "text", "text": req.system, "cache_control": {"type": "ephemeral"}}],
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(t) = &req.thinking {
        body["thinking"] = t.clone();
    }
    body
}

fn message_json(m: &Message) -> Value {
    let role = match m.role {
        Role::User => "user",
        Role::Assistant => "assistant",
    };
    let content: Vec<Value> = m
        .content
        .iter()
        .map(|b| match b {
            ContentBlock::Thinking {
                thinking,
                signature,
            } => json!({"type": "thinking", "thinking": thinking, "signature": signature}),
            ContentBlock::RedactedThinking { data } => {
                json!({"type": "redacted_thinking", "data": data})
            }
            ContentBlock::Text { text } => json!({"type": "text", "text": text}),
            ContentBlock::ToolUse { id, name, input } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": input})
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": content,
                "is_error": is_error,
            }),
        })
        .collect();
    json!({"role": role, "content": content})
}

enum Block {
    Thinking(String, Option<String>),
    Redacted(String),
    Text(String),
    Tool(PartialToolUse),
}

#[async_trait]
impl Provider for Anthropic {
    fn name(&self) -> &str {
        "anthropic"
    }

    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        let resp = self
            .client
            .post(format!(
                "{}/v1/messages",
                self.base_url.trim_end_matches('/')
            ))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&build_body(&request))
            .send()
            .await
            .context("sending request to Anthropic")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("Anthropic API error {status}: {text}");
        }

        let mut blocks: Vec<Block> = Vec::new();
        let mut stop_reason = StopReason::Other;
        let mut usage = Usage::default();
        let mut stream = sse::events(resp);
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if ev.data.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&ev.data)
                .with_context(|| format!("invalid SSE payload: {}", ev.data))?;
            match v["type"].as_str().unwrap_or_default() {
                "message_start" => {
                    usage.input_tokens =
                        v["message"]["usage"]["input_tokens"].as_u64().unwrap_or(0)
                            + v["message"]["usage"]["cache_read_input_tokens"]
                                .as_u64()
                                .unwrap_or(0)
                            + v["message"]["usage"]["cache_creation_input_tokens"]
                                .as_u64()
                                .unwrap_or(0);
                }
                "content_block_start" => {
                    let cb = &v["content_block"];
                    match cb["type"].as_str() {
                        Some("tool_use") => {
                            let id = cb["id"].as_str().unwrap_or_default().to_string();
                            let name = cb["name"].as_str().unwrap_or_default().to_string();
                            let _ = events.send(StreamEvent::ToolUseStart {
                                id: id.clone(),
                                name: name.clone(),
                            });
                            blocks.push(Block::Tool(PartialToolUse {
                                id,
                                name,
                                json: String::new(),
                            }));
                        }
                        Some("thinking") => blocks.push(Block::Thinking(String::new(), None)),
                        Some("redacted_thinking") => blocks.push(Block::Redacted(
                            cb["data"].as_str().unwrap_or_default().to_string(),
                        )),
                        _ => blocks.push(Block::Text(String::new())),
                    }
                }
                "content_block_delta" => {
                    let d = &v["delta"];
                    match (d["type"].as_str(), blocks.last_mut()) {
                        (Some("text_delta"), Some(Block::Text(t))) => {
                            let s = d["text"].as_str().unwrap_or_default();
                            t.push_str(s);
                            let _ = events.send(StreamEvent::TextDelta(s.to_string()));
                        }
                        (Some("thinking_delta"), Some(Block::Thinking(t, _))) => {
                            let s = d["thinking"].as_str().unwrap_or_default();
                            t.push_str(s);
                            let _ = events.send(StreamEvent::ThinkingDelta(s.to_string()));
                        }
                        (Some("signature_delta"), Some(Block::Thinking(_, sig))) => {
                            sig.get_or_insert_with(String::new)
                                .push_str(d["signature"].as_str().unwrap_or_default());
                        }
                        (Some("input_json_delta"), Some(Block::Tool(p))) => {
                            p.json
                                .push_str(d["partial_json"].as_str().unwrap_or_default());
                        }
                        _ => {}
                    }
                }
                "message_delta" => {
                    stop_reason = match v["delta"]["stop_reason"].as_str() {
                        Some("end_turn") | Some("stop_sequence") => StopReason::EndTurn,
                        Some("tool_use") => StopReason::ToolUse,
                        Some("max_tokens") => StopReason::MaxTokens,
                        _ => StopReason::Other,
                    };
                    usage.output_tokens = v["usage"]["output_tokens"].as_u64().unwrap_or(0);
                }
                "error" => return Err(anyhow!("Anthropic stream error: {}", v["error"])),
                _ => {}
            }
        }

        let content = blocks
            .into_iter()
            .filter_map(|b| match b {
                Block::Thinking(thinking, signature) => Some(ContentBlock::Thinking {
                    thinking,
                    signature,
                }),
                Block::Redacted(data) => Some(ContentBlock::RedactedThinking { data }),
                Block::Text(t) if t.is_empty() => None,
                Block::Text(text) => Some(ContentBlock::Text { text }),
                Block::Tool(p) => Some(p.finish()),
            })
            .collect();
        Ok(Response {
            message: Message {
                role: Role::Assistant,
                content,
            },
            stop_reason,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolSpec;

    #[test]
    fn body_marks_cache_breakpoints() {
        let req = Request {
            model: "m".into(),
            system: "sys".into(),
            messages: vec![Message::user_text("hi")],
            tools: vec![ToolSpec {
                name: "Read".into(),
                description: "d".into(),
                input_schema: json!({"type": "object"}),
            }],
            max_tokens: 10,
            thinking: None,
        };
        let b = build_body(&req);
        assert_eq!(
            b["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert_eq!(b["tools"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["system"][0]["text"], "sys");
    }
}
