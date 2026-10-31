//! The `.ez/workflows.toml` file format (ADR-0029 §2).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The only supported `version`.
pub const VERSION: u32 = 1;
/// Largest accepted file, in bytes.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// Most workflows in one file.
pub const MAX_WORKFLOWS: usize = 100;
/// Most nodes in one workflow.
pub const MAX_NODES: usize = 200;
/// Longest workflow or node id.
pub const MAX_ID_LEN: usize = 64;
/// `max_visits` when a node doesn't set it.
pub const DEFAULT_MAX_VISITS: u32 = 5;
/// `max_runs` when a workflow doesn't set it.
pub const DEFAULT_MAX_RUNS: u32 = 3;
/// Largest `max_runs`.
pub const MAX_MAX_RUNS: u32 = 50;
/// A run's `timeout` when the workflow doesn't set it.
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(14 * 24 * 3600);
/// Longest run or wait timeout.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(90 * 24 * 3600);

/// A whole `.ez/workflows.toml`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowsFile {
    pub version: u32,
    /// In file order.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub workflows: IndexMap<String, Workflow>,
}

/// One workflow: a trigger plus a graph.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// `event` or `event.action` names (`issues`, `issues.labeled`).
    pub on: Vec<String>,
    /// Filter expression over `event` and `subject`.
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    pub start: String,
    /// Run timeout (`14d`, humantime syntax).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    /// Live runs of this workflow at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_runs: Option<u32>,
    /// Per-workflow settings (scalars), readable as `vars.<name>` in templates and conditions.
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub vars: IndexMap<String, serde_json::Value>,
    /// In file order.
    pub nodes: IndexMap<String, Node>,
}

impl Workflow {
    /// The run timeout, or the default when unset or invalid (validation reports invalid values).
    pub fn run_timeout(&self) -> Duration {
        self.timeout
            .as_deref()
            .and_then(|t| parse_duration(t).ok())
            .unwrap_or(DEFAULT_RUN_TIMEOUT)
    }

    pub fn max_runs(&self) -> u32 {
        self.max_runs.unwrap_or(DEFAULT_MAX_RUNS)
    }
}

/// A graph node. The `type` key selects the variant.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Agent(AgentNode),
    Command(CommandNode),
    Action(ActionNode),
    Wait(WaitNode),
    End(EndNode),
}

/// The node types, for messages and the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Agent,
    Command,
    Action,
    Wait,
    End,
}

impl NodeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Command => "command",
            Self::Action => "action",
            Self::Wait => "wait",
            Self::End => "end",
        }
    }

    /// Output names a node of this type produces; inside its own `next` they are also bare
    /// variables.
    pub fn outputs(self) -> &'static [&'static str] {
        match self {
            Self::Agent => &["result", "git", "exit"],
            Self::Command => &["exit_code", "output", "json"],
            Self::Action => &["number", "url"],
            Self::Wait => &["event", "timed_out"],
            Self::End => &[],
        }
    }

    /// Whether the node runs in the sandbox (as a step) rather than in tracks.
    pub fn in_sandbox(self) -> bool {
        matches!(self, Self::Agent | Self::Command)
    }
}

impl Node {
    pub fn node_type(&self) -> NodeType {
        match self {
            Self::Agent(_) => NodeType::Agent,
            Self::Command(_) => NodeType::Command,
            Self::Action(_) => NodeType::Action,
            Self::Wait(_) => NodeType::Wait,
            Self::End(_) => NodeType::End,
        }
    }

    /// The outgoing edges; `None` for `end` nodes.
    pub fn next(&self) -> Option<&Next> {
        match self {
            Self::Agent(n) => Some(&n.next),
            Self::Command(n) => Some(&n.next),
            Self::Action(n) => Some(&n.next),
            Self::Wait(n) => Some(&n.next),
            Self::End(_) => None,
        }
    }

    pub fn max_visits(&self) -> u32 {
        let v = match self {
            Self::Agent(n) => n.max_visits,
            Self::Command(n) => n.max_visits,
            Self::Action(n) => n.max_visits,
            Self::Wait(n) => n.max_visits,
            Self::End(_) => None,
        };
        v.unwrap_or(DEFAULT_MAX_VISITS)
    }
}

/// `ez acp` in a mode, with a rendered instruction as the first user turn.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentNode {
    pub mode: String,
    pub instruction: String,
    /// Properties of the `submit_result` schema: name → JSON Schema fragment. Every property
    /// is required and no others are allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<IndexMap<String, serde_json::Value>>,
    /// Commit and push the run branch when the node succeeds.
    #[serde(default, skip_serializing_if = "is_false")]
    pub push: bool,
    pub next: Next,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visits: Option<u32>,
}

impl AgentNode {
    /// The JSON Schema for `submit_result`, when the node declares a `result`.
    pub fn result_schema(&self) -> Option<serde_json::Value> {
        let props = self.result.as_ref()?;
        let mut properties = serde_json::Map::new();
        for (k, v) in props {
            properties.insert(k.clone(), v.clone());
        }
        let required: Vec<serde_json::Value> = props.keys().map(|k| k.clone().into()).collect();
        Some(serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        }))
    }
}

/// A mise task (`task` + `args`) or a shell string (`run`) in the run's worktree.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommandNode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Templates, one argument each (no shell).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// A shell template; every `{{ … }}` value is shell-quoted (`| safe` inserts it raw).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    pub next: Next,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visits: Option<u32>,
}

/// One GitHub call made by tracks with the App's installation token.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionNode {
    pub action: ActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Templates, one label each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<bool>,
    pub next: Next,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visits: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    /// Comment on the run's subject. Needs `body`.
    Comment,
    /// Open a pull request from the run branch to the default branch. Needs `title`.
    OpenPullRequest,
    /// Add `labels` to the run's subject.
    AddLabels,
    /// Remove `labels` from the run's subject.
    RemoveLabels,
}

impl ActionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::OpenPullRequest => "open_pull_request",
            Self::AddLabels => "add_labels",
            Self::RemoveLabels => "remove_labels",
        }
    }
}

/// Suspends the run until an event about its subject matches, or the timeout passes.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WaitNode {
    pub on: Vec<String>,
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    /// humantime syntax (`7d`); none means the run's timeout bounds the wait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    pub next: Next,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visits: Option<u32>,
}

impl WaitNode {
    pub fn wait_timeout(&self) -> Option<Duration> {
        self.timeout.as_deref().and_then(|t| parse_duration(t).ok())
    }
}

/// Ends the run.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EndNode {
    pub status: EndStatus,
    /// Why the run ends here (a template, rendered with the run's context). A `failed` or
    /// `cancelled` end without one gets a generic reason naming this node and the previous one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndStatus {
    Done,
    Failed,
    Cancelled,
}

impl EndStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// `next = "node"` or `next = [{ if = "…", to = "node" }, …, { to = "node" }]`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Next {
    Node(String),
    Edges(Vec<Edge>),
}

impl Next {
    /// The edges in order; a plain node id is one unconditional edge.
    pub fn edges(&self) -> Vec<Edge> {
        match self {
            Self::Node(to) => vec![Edge {
                condition: None,
                to: to.clone(),
            }],
            Self::Edges(e) => e.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    #[serde(default, rename = "if", skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    pub to: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// humantime durations (`90s`, `30m`, `7d`, `1d 12h`).
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    humantime::parse_duration(s.trim()).map_err(|e| format!("invalid duration `{s}`: {e}"))
}

/// Parses the file. Size and TOML/shape errors only; [`crate::validate`] checks the rest.
pub fn parse(text: &str) -> Result<WorkflowsFile, String> {
    if text.len() > MAX_FILE_BYTES {
        return Err(format!(
            "workflows.toml is {} bytes; the limit is {MAX_FILE_BYTES}",
            text.len()
        ));
    }
    toml::from_str(text).map_err(|e| e.to_string().trim_end().to_string())
}

/// Serializes the file as TOML (the designers write files with this).
pub fn to_toml(file: &WorkflowsFile) -> Result<String, String> {
    toml::to_string(file).map_err(|e| e.to_string())
}
