//! The task document (ADR-0022 §3): the only data templates and conditions
//! see. Its top-level keys are the documented roots; anything else a prompt
//! uses must be a declared step input.

use crate::events::Event;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Everything known about a task.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct TaskDocument {
    pub task: TaskInfo,
    pub subjects: Subjects,
    /// The event that triggered the task, or the last event a wait accepted.
    #[serde(default)]
    pub event: Option<Event>,
    /// Validated step outputs by name (output files, and `result` under its
    /// `as` name). Persist across phase re-entry; later writes replace.
    #[serde(default)]
    pub outputs: Map<String, Value>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    #[serde(default)]
    pub facts: Facts,
    #[serde(default)]
    pub relations: Vec<Relation>,
    /// Bounded summary of recent tasks of the project (filled by the engine).
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    /// Task variables assigned by `set` (nodes, handlers). Scoped to the
    /// task; persist across phase re-entry.
    #[serde(default)]
    pub vars: Map<String, Value>,
    /// The enzarb instance (filled by the engine).
    #[serde(default)]
    pub instance: Instance,
}

/// What a binding's `if` and `with` read when the engine routes an event
/// about an unlinked object: the event, plus a summary of the live tasks the
/// event's object is related to (e.g. the task of an issue a new PR
/// references), supplied by the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingContext {
    pub event: Event,
    #[serde(default)]
    pub relations: Vec<RoutingRelation>,
}

/// A related task, as seen at routing time.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct RoutingRelation {
    /// How the event's object relates to the task (`references`, `parent`, …).
    pub kind: String,
    /// The related task's id.
    pub task: String,
    #[serde(default)]
    pub workflow: String,
    /// Engine phase or end status of the related task.
    #[serde(default)]
    pub status: String,
    /// `phase/…` and node of the related task.
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub node: String,
    /// The related task's `vars`.
    #[serde(default)]
    pub vars: Map<String, Value>,
    /// The related task's primary subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<crate::events::SubjectRef>,
}

/// The enzarb instance the task runs on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Instance {
    #[serde(default)]
    pub bot: Bot,
    /// Base URL of the instance UI.
    #[serde(default)]
    pub url: String,
}

/// The GitHub App identity of the instance.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Bot {
    /// GitHub login (`enzarb[bot]`).
    #[serde(default)]
    pub login: String,
    /// Mention handle without `@` (`enzarb`).
    #[serde(default)]
    pub handle: String,
}

/// Root names a template may use without declaring an input.
pub const ROOTS: &[&str] = &[
    "task",
    "subjects",
    "event",
    "outputs",
    "artifacts",
    "facts",
    "relations",
    "history",
    "vars",
    "instance",
];

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct TaskInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub project: String,
    /// The triggering repository, `owner/repo` (the primary workspace).
    #[serde(default)]
    pub repo: String,
    /// Workflow file reference the task runs.
    #[serde(default)]
    pub workflow: String,
    /// The default branch of `repo`.
    #[serde(default)]
    pub default_branch: String,
}

/// The GitHub objects linked to the task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Subjects {
    #[serde(default)]
    pub issue: Option<Subject>,
    #[serde(default)]
    pub pull_requests: Vec<Subject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Subject {
    pub repo: String,
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub url: String,
    /// `open`, `closed` or `merged`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub author: String,
    /// Pull requests only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Merged pull requests: the merge commit (verified, from GitHub).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_sha: Option<String>,
    #[serde(default)]
    pub comments: Vec<SubjectComment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct SubjectComment {
    pub author: String,
    pub body: String,
    #[serde(default)]
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Artifact {
    pub title: String,
    /// `markdown`, `json` or `text`.
    pub format: String,
    pub content: String,
    /// `phase/node` that produced it.
    #[serde(default)]
    pub node: String,
}

/// Facts the engine verified from git and GitHub (never from agent output).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Facts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitFacts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci: Option<CiFacts>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviews: Vec<ReviewFact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
    /// Tag that contains the merged change, once released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contained_in_tag: Option<String>,
    /// `None` until the engine knows; `[]` means known to have none (so `facts.dependencies is
    /// not none` tells empty from unknown).
    #[serde(default)]
    pub dependencies: Option<Vec<DependencyFact>>,
}

/// Fact kinds that `refresh` and `RefreshFact` name.
pub const FACT_KINDS: &[&str] = &[
    "git",
    "ci",
    "reviews",
    "mergeable",
    "merged",
    "contained_in_tag",
    "dependencies",
];

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct GitFacts {
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub head: String,
    /// The branch is on the remote at `head`.
    #[serde(default)]
    pub pushed: bool,
    /// Commits ahead of the base branch.
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub changed_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct CiFacts {
    pub sha: String,
    /// `pending`, `success` or `failure`.
    pub state: String,
    #[serde(default)]
    pub failing: Vec<FailingCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct FailingCheck {
    pub name: String,
    #[serde(default)]
    pub log_excerpt: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ReviewFact {
    pub author: String,
    pub state: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct DependencyFact {
    pub task: String,
    pub status: String,
    /// The dependency has ended.
    #[serde(default)]
    pub ended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Relation {
    pub kind: String,
    /// Target task id, or a subject reference until the engine links it.
    pub target: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEntry {
    pub task: String,
    pub workflow: String,
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub ended_at: String,
}

impl TaskDocument {
    /// Looks up a dotted path (`outputs.plan`, `subjects.issue.title`, `a.0`).
    pub fn lookup(&self, path: &str) -> Option<Value> {
        let root = serde_json::to_value(self).ok()?;
        let mut cur = &root;
        for seg in path.split('.') {
            cur = match cur {
                Value::Object(m) => m.get(seg)?,
                Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        (!cur.is_null()).then(|| cur.clone())
    }

    /// Replaces one fact by kind (value is the fact's JSON). Unknown kinds are an error.
    pub fn set_fact(&mut self, kind: &str, value: Value) -> Result<(), String> {
        if !FACT_KINDS.contains(&kind) {
            return Err(format!("unknown fact kind `{kind}`"));
        }
        let mut facts = serde_json::to_value(&self.facts).map_err(|e| e.to_string())?;
        facts
            .as_object_mut()
            .expect("facts is an object")
            .insert(kind.into(), value);
        self.facts = serde_json::from_value(facts).map_err(|e| format!("fact `{kind}`: {e}"))?;
        Ok(())
    }
}
