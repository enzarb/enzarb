//! Tool abstraction and the built-in tool set.

pub mod ask;
mod bash;
mod fs;
mod search;
mod todo;

pub use ask::{Answer, AskUserQuestion, Question, QuestionOption};
pub use bash::Bash;
pub use fs::{Edit, Read, Write};
pub use search::{Glob, Grep};
pub use todo::{TodoItem, TodoStatus, TodoWrite};

use async_trait::async_trait;
use ez_llm::ToolSpec;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Matches the ACP `ToolKind` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl ToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolKind::Read => "read",
            ToolKind::Edit => "edit",
            ToolKind::Delete => "delete",
            ToolKind::Move => "move",
            ToolKind::Search => "search",
            ToolKind::Execute => "execute",
            ToolKind::Think => "think",
            ToolKind::Fetch => "fetch",
            ToolKind::SwitchMode => "switch_mode",
            ToolKind::Other => "other",
        }
    }

    /// Whether the tool never mutates anything (safe to auto-allow by default).
    pub fn is_read_only(self) -> bool {
        matches!(
            self,
            ToolKind::Read | ToolKind::Search | ToolKind::Think | ToolKind::Fetch
        )
    }
}

/// A file modification performed by a tool, used for diffs and post-edit hooks.
#[derive(Debug, Clone, PartialEq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
    pub edits: Vec<FileEdit>,
    pub plan: Option<Vec<TodoItem>>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..Default::default()
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            ..Default::default()
        }
    }
}

/// Interaction with the human on the other side of the protocol.
#[async_trait]
pub trait UserBridge: Send + Sync {
    async fn ask_user(
        &self,
        session_id: &str,
        questions: Vec<Question>,
    ) -> anyhow::Result<Vec<Answer>>;
}

#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub session_id: String,
    pub user: Arc<dyn UserBridge>,
    pub cancel: CancellationToken,
}

impl ToolContext {
    pub fn resolve(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> String;
    fn input_schema(&self) -> Value;
    fn kind(&self) -> ToolKind;

    /// Short human-readable title for a call, e.g. "Read src/main.rs".
    fn title(&self, input: &Value) -> String {
        let _ = input;
        self.name().to_string()
    }

    /// Files this call touches, for client "follow along" features.
    fn locations(&self, input: &Value) -> Vec<String> {
        ["file_path", "path"]
            .iter()
            .filter_map(|k| input.get(*k).and_then(Value::as_str).map(str::to_string))
            .take(1)
            .collect()
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput>;

    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name().to_string(),
            description: self.description(),
            input_schema: self.input_schema(),
        }
    }
}

/// All built-in tools (excluding agent-level tools such as SwitchMode).
pub fn builtin_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(Read),
        Arc::new(Write),
        Arc::new(Edit),
        Arc::new(Glob),
        Arc::new(Grep),
        Arc::new(Bash),
        Arc::new(TodoWrite),
        Arc::new(AskUserQuestion),
    ]
}

pub(crate) fn str_arg<'a>(input: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing required string parameter `{key}`"))
}

/// Truncate long output, keeping the head and tail.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let mut head_end = half;
    while !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len() - half;
    while !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    format!(
        "{}\n\n... [{} bytes truncated] ...\n\n{}",
        &s[..head_end],
        tail_start - head_end,
        &s[tail_start..]
    )
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub struct NoUser;

    #[async_trait]
    impl UserBridge for NoUser {
        async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
            anyhow::bail!("no user")
        }
    }

    pub fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            session_id: "test".into(),
            user: Arc::new(NoUser),
            cancel: CancellationToken::new(),
        }
    }
}
