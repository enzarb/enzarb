//! AskUserQuestion: structured multiple-choice questions for the user.

use crate::{Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

/// The user's answer: one or more selected labels, or free text ("Other").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub question: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

pub struct AskUserQuestion;

#[async_trait]
impl Tool for AskUserQuestion {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn description(&self) -> String {
        "Ask the user 1-4 multiple-choice questions when blocked on a decision only they can make. \
         The user can always answer with free text instead of an option. Put a recommended option \
         first and suffix its label with \"(Recommended)\"."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {"type": "string"},
                            "header": {"type": "string", "description": "Very short label (max 12 chars)"},
                            "multiSelect": {"type": "boolean", "default": false},
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {"type": "string"},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label"]
                                }
                            }
                        },
                        "required": ["question", "options"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn title(&self, _: &Value) -> String {
        "Ask user".into()
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let questions: Vec<Question> = match serde_json::from_value(input["questions"].clone()) {
            Ok(q) => q,
            Err(e) => return Ok(ToolOutput::error(format!("invalid questions: {e}"))),
        };
        if questions.is_empty() {
            return Ok(ToolOutput::error("at least one question is required"));
        }
        match ctx.user.ask_user(&ctx.session_id, questions).await {
            Ok(answers) => Ok(ToolOutput::ok(format_answers(&answers))),
            Err(e) => Ok(ToolOutput::error(format!(
                "Could not ask the user interactively ({e}). Ask your question in plain text and end your turn."
            ))),
        }
    }
}

pub fn format_answers(answers: &[Answer]) -> String {
    let mut s = String::from("The user answered:\n");
    for a in answers {
        let mut parts = a.selected.clone();
        if let Some(o) = &a.other {
            parts.push(format!("Other: {o}"));
        }
        let ans = if parts.is_empty() {
            "(no answer)".to_string()
        } else {
            parts.join(", ")
        };
        s.push_str(&format!("- \"{}\" = \"{}\"\n", a.question, ans));
    }
    s
}
