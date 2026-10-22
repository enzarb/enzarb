//! Provider-neutral LLM types plus Anthropic and OpenAI-compatible clients.

pub mod anthropic;
pub mod mock;
pub mod models;
pub mod openai;
mod sse;

pub use models::{ApiKind, ModelInfo, list_models};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Model reasoning. `signature` must be round-tripped for Anthropic extended thinking.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    RedactedThinking {
        data: String,
    },
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: text.into() }],
        }
    }

    /// Concatenated text of all text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub model: String,
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
    /// Provider-specific reasoning configuration, passed through as-is
    /// (Anthropic: the `thinking` object; OpenAI-compatible: `reasoning_effort` string).
    pub thinking: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub message: Message,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

/// Incremental output while a response streams.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    TextDelta(String),
    ThinkingDelta(String),
    ToolUseStart { id: String, name: String },
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;

    /// Send a request, forwarding streaming events to `events`, and return the final message.
    async fn complete(
        &self,
        request: Request,
        events: UnboundedSender<StreamEvent>,
    ) -> anyhow::Result<Response>;
}

/// Accumulates a partially-streamed tool call.
#[derive(Default)]
pub(crate) struct PartialToolUse {
    pub id: String,
    pub name: String,
    pub json: String,
}

impl PartialToolUse {
    pub fn finish(self) -> ContentBlock {
        let input = if self.json.trim().is_empty() {
            Value::Object(Default::default())
        } else {
            serde_json::from_str(&self.json)
                .unwrap_or_else(|_| serde_json::json!({ "_invalid_json": self.json }))
        };
        ContentBlock::ToolUse {
            id: self.id,
            name: self.name,
            input,
        }
    }
}

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("ez/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}
