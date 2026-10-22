//! OpenAI-compatible Chat Completions client (OpenAI, Ollama, OpenRouter, vLLM, ...).

use crate::{
    ContentBlock, Message, PartialToolUse, Provider, Request, Response, Role, StopReason,
    StreamEvent, Usage, http_client, sse,
};
use anyhow::{Context, bail};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::mpsc::UnboundedSender;

pub struct OpenAi {
    api_key: Option<String>,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAi {
    pub fn new(api_key: Option<String>, base_url: Option<String>) -> Self {
        Self {
            api_key,
            base_url: base_url.unwrap_or_else(|| "https://api.openai.com/v1".into()),
            client: http_client(),
        }
    }
}

pub(crate) fn build_body(req: &Request) -> Value {
    let mut messages = vec![json!({"role": "system", "content": req.system})];
    for m in &req.messages {
        messages.extend(message_json(m));
    }
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages,
    });
    if let Some(effort) = &req.thinking {
        body["reasoning_effort"] = effort.clone();
    }
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({"type": "function", "function": {
                    "name": t.name, "description": t.description, "parameters": t.input_schema,
                }})
            })
            .collect();
    }
    body
}

/// One internal message can become several OpenAI messages (tool results are separate messages).
fn message_json(m: &Message) -> Vec<Value> {
    match m.role {
        Role::Assistant => {
            let text = m.text();
            let calls: Vec<Value> = m
                .tool_uses()
                .map(|(id, name, input)| {
                    json!({"id": id, "type": "function",
                           "function": {"name": name, "arguments": input.to_string()}})
                })
                .collect();
            let mut v = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { text.into() }});
            if !calls.is_empty() {
                v["tool_calls"] = Value::Array(calls);
            }
            vec![v]
        }
        Role::User => {
            let mut out = Vec::new();
            let mut text = String::new();
            for b in &m.content {
                match b {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let content = if *is_error {
                            format!("ERROR: {content}")
                        } else {
                            content.clone()
                        };
                        out.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}));
                    }
                    ContentBlock::Text { text: t } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                    _ => {}
                }
            }
            if !text.is_empty() {
                out.push(json!({"role": "user", "content": text}));
            }
            out
        }
    }
}

#[async_trait]
impl Provider for OpenAi {
    fn name(&self) -> &str {
        "openai"
    }

    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response> {
        let mut rb = self
            .client
            .post(format!(
                "{}/chat/completions",
                self.base_url.trim_end_matches('/')
            ))
            .json(&build_body(&request));
        if let Some(key) = &self.api_key {
            rb = rb.bearer_auth(key);
        }
        let resp = rb
            .send()
            .await
            .context("sending request to OpenAI-compatible API")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            bail!("OpenAI-compatible API error {status}: {text}");
        }

        let mut text = String::new();
        let mut reasoning = String::new();
        let mut tools: BTreeMap<u64, PartialToolUse> = BTreeMap::new();
        let mut stop_reason = StopReason::Other;
        let mut usage = Usage::default();
        let mut stream = sse::events(resp);
        while let Some(ev) = stream.next().await {
            let ev = ev?;
            if ev.data.trim() == "[DONE]" {
                break;
            }
            if ev.data.is_empty() {
                continue;
            }
            let v: Value = serde_json::from_str(&ev.data)
                .with_context(|| format!("invalid SSE payload: {}", ev.data))?;
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                usage.input_tokens = u["prompt_tokens"].as_u64().unwrap_or(0);
                usage.output_tokens = u["completion_tokens"].as_u64().unwrap_or(0);
            }
            let Some(choice) = v["choices"].get(0) else {
                continue;
            };
            let delta = &choice["delta"];
            if let Some(s) = delta["content"].as_str()
                && !s.is_empty()
            {
                text.push_str(s);
                let _ = events.send(StreamEvent::TextDelta(s.to_string()));
            }
            // Reasoning text from providers that expose it (DeepSeek, vLLM, OpenRouter, ...).
            if let Some(s) = delta["reasoning_content"]
                .as_str()
                .or_else(|| delta["reasoning"].as_str())
                && !s.is_empty()
            {
                reasoning.push_str(s);
                let _ = events.send(StreamEvent::ThinkingDelta(s.to_string()));
            }
            if let Some(calls) = delta["tool_calls"].as_array() {
                for c in calls {
                    let idx = c["index"].as_u64().unwrap_or(0);
                    let p = tools.entry(idx).or_default();
                    if let Some(id) = c["id"].as_str() {
                        p.id = id.to_string();
                    }
                    if let Some(name) = c["function"]["name"].as_str() {
                        p.name.push_str(name);
                        let _ = events.send(StreamEvent::ToolUseStart {
                            id: p.id.clone(),
                            name: p.name.clone(),
                        });
                    }
                    if let Some(args) = c["function"]["arguments"].as_str() {
                        p.json.push_str(args);
                    }
                }
            }
            if let Some(fr) = choice["finish_reason"].as_str() {
                stop_reason = match fr {
                    "stop" => StopReason::EndTurn,
                    "tool_calls" | "function_call" => StopReason::ToolUse,
                    "length" => StopReason::MaxTokens,
                    _ => StopReason::Other,
                };
            }
        }

        let mut content = Vec::new();
        if !reasoning.is_empty() {
            content.push(ContentBlock::Thinking {
                thinking: reasoning,
                signature: None,
            });
        }
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
        for (i, mut p) in tools {
            if p.id.is_empty() {
                p.id = format!("call_{i}");
            }
            content.push(p.finish());
        }
        if stop_reason == StopReason::Other
            && content
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
        {
            stop_reason = StopReason::ToolUse;
        }
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

    #[test]
    fn tool_results_become_tool_messages() {
        let req = Request {
            model: "m".into(),
            system: "sys".into(),
            messages: vec![
                Message::user_text("hi"),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::ToolUse {
                        id: "c1".into(),
                        name: "Read".into(),
                        input: json!({"path": "a"}),
                    }],
                },
                Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: "c1".into(),
                        content: "data".into(),
                        is_error: false,
                    }],
                },
            ],
            tools: vec![],
            max_tokens: 10,
            thinking: None,
        };
        let b = build_body(&req);
        let msgs = b["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "Read");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(msgs[3]["tool_call_id"], "c1");
    }
}
