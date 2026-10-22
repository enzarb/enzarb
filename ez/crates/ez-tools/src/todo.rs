use crate::{Tool, ToolContext, ToolKind, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

pub struct TodoWrite;

#[async_trait]
impl Tool for TodoWrite {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn description(&self) -> String {
        "Replace the session's task list. Use it to plan multi-step work and mark progress; \
         keep exactly one item in_progress while working."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {"type": "string"},
                            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]}
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Think
    }

    fn title(&self, _: &Value) -> String {
        "Update plan".into()
    }

    async fn call(&self, input: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
        let todos: Vec<TodoItem> = match serde_json::from_value(input["todos"].clone()) {
            Ok(t) => t,
            Err(e) => return Ok(ToolOutput::error(format!("invalid todos: {e}"))),
        };
        let done = todos
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        Ok(ToolOutput {
            content: format!("Plan updated ({done}/{} completed)", todos.len()),
            plan: Some(todos),
            ..Default::default()
        })
    }
}
