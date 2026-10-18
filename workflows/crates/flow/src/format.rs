//! The definition file formats (ADR-0022 §2–§4, §11): workflow, phase,
//! step, runtime, board and bindings files. Plain serde types; the JSON
//! Schemas in `docs/workflows/schemas/` are generated from them.
//!
//! Nothing here knows any particular workflow: names of phases, steps,
//! exits, statuses, columns and commands are all data.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// `alias: owner/repo@ref` pairs. A reference whose first path segment is an
/// alias (`std/phases/plan`) is loaded from that repository at that ref.
pub type Imports = BTreeMap<String, String>;

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------- workflow

/// A workflow file: an ordered list of phases.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowFile {
    /// Format version. Always `1`.
    pub version: u32,
    /// Free text shown in the viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub imports: Imports,
    /// The phases, run in order. `done` of one phase enters the next.
    pub phases: Vec<PhaseEntry>,
    /// Custom events this workflow can wait on, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub events: BTreeMap<String, CustomEventDecl>,
    /// Reference to a board file (`std/board`). Layered over the library board.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<String>,
    /// Typed params of the workflow, set by the binding's `with`. Phase
    /// entries pass them down as `{param: <name>}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Param>,
    /// Task-wide handlers, tried in order on every event in any phase, wait
    /// or running step; the first match wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<Handler>,
}

/// A task-wide event handler (`on:` of a workflow).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Handler {
    /// Events that trigger the handler (same matchers as a wait).
    pub events: Vec<Matcher>,
    /// Default sender requirement for matchers without `from` (default `anyone`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Sender>,
    /// Extra condition (an expression; `event` is the candidate).
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
    /// Task variables assigned when the handler fires (see node `set`).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub set: Map<String, Value>,
    /// What to do: `cancel` (end with status `cancelled`), `end:<status>`,
    /// `goto:<phase id>` or `stop_step`. Without `do` the handler only
    /// assigns `set` and the event is still offered to the step or wait.
    #[serde(rename = "do", default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub do_: Option<HandlerDo>,
}

/// The action of a task-wide handler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum HandlerDo {
    /// End the task with status `cancelled`.
    Cancel,
    /// End the task with this status.
    End(String),
    /// Enter this top-level phase (counts as an entry).
    Goto(String),
    /// Stop the running step (it finishes `interrupted`; its edges decide).
    StopStep,
}

/// End status used by `do: cancel`.
pub const CANCELLED: &str = "cancelled";

impl TryFrom<String> for HandlerDo {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        let s = s.trim();
        match s.split_once(':') {
            None if s == "cancel" => Ok(HandlerDo::Cancel),
            None if s == "stop_step" => Ok(HandlerDo::StopStep),
            Some(("end", v)) if !v.trim().is_empty() => Ok(HandlerDo::End(v.trim().into())),
            Some(("goto", v)) if !v.trim().is_empty() => Ok(HandlerDo::Goto(v.trim().into())),
            _ => Err(format!(
                "`do: {s}` must be `cancel`, `end:<status>`, `goto:<phase>` or `stop_step`"
            )),
        }
    }
}

impl From<HandlerDo> for String {
    fn from(d: HandlerDo) -> String {
        match d {
            HandlerDo::Cancel => "cancel".into(),
            HandlerDo::StopStep => "stop_step".into(),
            HandlerDo::End(s) => format!("end:{s}"),
            HandlerDo::Goto(p) => format!("goto:{p}"),
        }
    }
}

// ---------------------------------------------------------------- templated literals

/// An integer field: a number, `{param: <name>}`, or a string: a duration
/// (`90s`, `15m`, `2h`, `14d`, `1w`, where durations are allowed) or a
/// template (`"{{ params.rounds * 2 }}"`) rendering to one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum NumField {
    Int(u64),
    Param { param: String },
    Value(String),
}

impl From<u64> for NumField {
    fn from(n: u64) -> Self {
        NumField::Int(n)
    }
}

/// A text field: a string (a MiniJinja template when it contains `{{`/`{%`)
/// or `{param: <name>}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TextField {
    Param { param: String },
    Value(String),
}

impl TextField {
    /// The text, or the param name for an unresolved `{param}`.
    pub fn as_str(&self) -> &str {
        match self {
            TextField::Param { param } => param,
            TextField::Value(s) => s,
        }
    }
}

impl From<&str> for TextField {
    fn from(s: &str) -> Self {
        TextField::Value(s.into())
    }
}

impl From<String> for TextField {
    fn from(s: String) -> Self {
        TextField::Value(s)
    }
}

/// Parses an integer or a duration (`90`, `90s`, `15m`, `2h`, `14d`, `1w`,
/// also combined like `1h30m`) into seconds.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    let mut total: u64 = 0;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let unit = match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            'w' => 604_800,
            _ => return None,
        };
        let n: u64 = num.parse().ok()?;
        num.clear();
        total = total.checked_add(n.checked_mul(unit)?)?;
    }
    num.is_empty().then_some(total)
}

/// One phase of a workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseEntry {
    /// Unique id in this workflow. Defaults to the last segment of `use`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Reference to a phase file (`std/phases/plan`, `.enzarb/phases/lint`).
    #[serde(rename = "use")]
    pub use_: String,
    /// Values for the phase's params.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub with: Map<String, Value>,
    /// Where each exit goes. Unmapped `done` enters the next phase; any other
    /// unmapped exit ends the task with the exit name as status.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub exits: BTreeMap<String, ExitTo>,
    /// How often the phase may be entered (default 5). A number,
    /// `{param: <name>}` of the workflow, or a template over `params`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<NumField>,
    /// The exit taken instead of entering the phase once more than
    /// `max_entries` times (default `limit_reached`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_limit: Option<String>,
}

/// Target of a phase exit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ExitTo {
    /// `next` (the following phase) or the id of any phase, earlier or later.
    Phase(String),
    /// End the task with this status.
    End { end: String },
}

/// Declaration of a custom event (ADR-0022 §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomEventDecl {
    pub description: String,
    /// JSON Schema of the payload.
    #[serde(default = "any_schema")]
    pub schema: Value,
}

fn any_schema() -> Value {
    Value::Object(Map::new())
}

// ---------------------------------------------------------------- params

/// A typed parameter of a phase or step file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Param {
    #[serde(rename = "type")]
    pub ty: ParamType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Value used when the caller gives none. A param without a default is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Allowed values of an `enum` param.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Item type of a `list` param (default `string`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<ParamType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ParamType {
    String,
    Integer,
    Boolean,
    /// A MiniJinja prompt template: inline text or `{file: path}`.
    Prompt,
    /// A shell command line.
    Command,
    /// One of `values`.
    Enum,
    /// A list of `items`.
    List,
}

/// A value that is either given inline, read from a file in the referencing
/// repository, or taken from a param of the enclosing file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Source {
    Inline(String),
    File { file: String },
    Param { param: String },
}

// ---------------------------------------------------------------- phase

/// A phase file: a subgraph with typed params, named exits and private nodes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseFile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub imports: Imports,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Param>,
    /// Exit names besides the implicit `done` and `limit_reached`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<String>,
    /// The first node.
    pub start: String,
    pub nodes: BTreeMap<String, Node>,
}

/// A node: exactly one of `step`, `wait`, `action`, `end`, `phase`, plus edges.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Node {
    /// Runs a step in the sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<StepUse>,
    /// Waits for events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<Wait>,
    /// Calls a generic connector action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    /// Leaves the phase through the exit named `status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<End>,
    /// Runs a nested phase; its exit is `exit` in the edge conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<PhaseUse>,
    /// Pure branching: no effect, `next` is followed as soon as the node is entered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decide: Option<Decide>,
    /// Task variables assigned on entry, in order: `name: <expression>`
    /// (strings are expressions; other YAML values and `{param: x}` are
    /// literals). Stored in `vars`. A node with only `set` and `next` is a
    /// decide node.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub set: Map<String, Value>,
    /// Task variables assigned when the node is left, after its edge was
    /// chosen (`last`, `exit` and, at a wait, `event` are in scope).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub set_on_exit: Map<String, Value>,
    /// The next node. Not allowed on `end`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Next>,
    /// How often this node may be entered per phase entry (default 10). A
    /// number, `{param: <name>}`, or a template over `params`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visits: Option<NumField>,
    /// Node entered instead once `max_visits` is exceeded. Without it the
    /// phase exits with `limit_reached`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_limit: Option<String>,
}

/// A decide node: no effect, only edges.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decide {
    /// Free text shown in the viewer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Edges: a node id, or ordered branches where the first true `if` wins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Next {
    To(String),
    Branches(Vec<Branch>),
}

/// `{if: <expression>, to: <node>}` or a final `{else: <node>}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Branch {
    /// A MiniJinja expression over the task document.
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(rename = "else", default, skip_serializing_if = "Option::is_none")]
    pub else_: Option<String>,
}

/// Use of a step file, or an inline step definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepUse {
    #[serde(rename = "use", default, skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    /// An inline step (instead of `use`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<Box<StepFile>>,
    /// Param values. `{param: name}` passes a param of the enclosing phase.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub with: Map<String, Value>,
}

/// Use of a nested phase file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PhaseUse {
    #[serde(rename = "use")]
    pub use_: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub with: Map<String, Value>,
}

/// A wait node (engine side).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    /// Event types that can satisfy the wait (catalog types, declared custom
    /// events, `timer`, or `fact`).
    pub events: Vec<Matcher>,
    /// Who may send them (default `anyone`); applies to every matcher that sets no `from`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Sender>,
    /// Machine-readable wait reason shown in the projection (text field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<TextField>,
    /// Human text (a MiniJinja template, or `{param: x}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<TextField>,
    /// Board column id while waiting here (text field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<TextField>,
    /// Who the task waits on. Default: `human` if the wait has actions or
    /// requires a writer, else `external`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<WaitingOn>,
    /// UI buttons; each posts its `/command` as the clicking user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<UiActionSpec>,
    /// Events forwarded into the task's last agent session while waiting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steer: Vec<Matcher>,
    /// Facts refreshed when the wait is entered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refresh: Vec<String>,
    /// A timer set when the wait is entered; it fires a `timer` event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timer: Option<Timer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Timer {
    /// Seconds, a duration (`14d`), `{param: x}`, or a template. A template
    /// that reads more than `params` is rendered on entry.
    pub seconds: NumField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOn {
    Human,
    Agent,
    External,
}

/// Required sender of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sender {
    #[default]
    Anyone,
    /// The actor must have write access (`actor.can_write`, supplied by the engine).
    Writer,
}

/// An event matcher: a type name, or a type with a sender requirement and condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Matcher {
    Type(String),
    Full(MatcherSpec),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MatcherSpec {
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<Sender>,
    /// MiniJinja expression over the task document; `event` is the candidate.
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
}

impl Matcher {
    pub fn event(&self) -> &str {
        match self {
            Matcher::Type(t) => t,
            Matcher::Full(m) => &m.event,
        }
    }
}

/// A UI button as declared at a wait: `label` and `command` are text fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiActionSpec {
    pub label: TextField,
    /// Command word posted as `/command` (without the slash).
    pub command: TextField,
}

/// A rendered UI button (the projection's `actions`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiAction {
    pub label: String,
    /// Command word posted as `/command` (without the slash).
    pub command: String,
}

/// A generic connector call made by the engine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// Connector action name.
    pub name: String,
    /// Arguments: any YAML. String leaves are MiniJinja templates; a leaf
    /// that is exactly one `{{ expr }}` keeps the expression's JSON type;
    /// `{param: x}` is the param's value.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub args: Map<String, Value>,
    /// Task relations: `targets` (evaluated before the call) or `from`
    /// (evaluated over the call's `result`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relate: Option<Relate>,
    /// Stores a successful call's result as `outputs.<as>`.
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub as_: Option<String>,
    /// Links the GitHub object the call returned as a subject of the task:
    /// `true` reads `kind`, `number` and `repo` from the result, or give
    /// `{kind, number, repo}` (`number`/`repo` are expressions over `result`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectLink>,
}

/// Relations to create. Exactly one of `targets` and `from`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Relate {
    pub kind: String,
    /// Expression yielding a list, evaluated before the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<String>,
    /// Expression over the call's `result` (and the document) yielding a
    /// target or a list of them, evaluated after a successful call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// How an action's result becomes a linked subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SubjectLink {
    /// `true`: the result has `kind` (`issue`/`pull_request`), `number` and optionally `repo`.
    Flag(bool),
    Spec(SubjectSpec),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubjectSpec {
    pub kind: crate::events::SubjectKind,
    /// Expression over `result` (default `result.number`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<String>,
    /// Expression over `result` (default `result.repo`, else `task.repo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct End {
    /// The exit name (inside a phase); for the task, its end status.
    pub status: String,
}

// ---------------------------------------------------------------- step

/// A step file: one unit of work in the sandbox, an agent or a command.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub imports: Imports,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, Param>,
    /// An agent step (exactly one of `agent` and `command`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Agent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    /// Named template variables taken from the task document.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, Input>,
    /// Output files collected after the step.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub outputs: BTreeMap<String, Output>,
    /// Structured result submitted through the `submit_result` tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultSpec>,
    /// Extra workspaces besides the task's primary one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<Workspace>,
    /// GitHub token permissions (`contents: write`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub github: BTreeMap<String, Access>,
    /// Secrets injected into this step's process: a bare name (project
    /// scope, env var of that name) or `{name, scope, env | file}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretDecl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<Concurrency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// Events delivered into the running agent session as user turns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steer: Vec<Matcher>,
    /// Events that stop the running step (it then finishes with exit
    /// `interrupted`). Default: a `stop` command from a writer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<Matcher>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// Reference to a runtime file.
    pub runtime: String,
    /// Model name (text field: `{param: x}` or a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<TextField>,
    /// MiniJinja prompt template.
    pub prompt: Source,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Command {
    /// Shell command line (a MiniJinja template).
    pub run: Source,
    #[serde(default, skip_serializing_if = "Tools::is_empty")]
    pub tools: Tools,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tools {
    /// mise tool specs installed before the step (`node@22`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mise: Vec<String>,
}

impl Tools {
    fn is_empty(&self) -> bool {
        self.mise.is_empty()
    }
}

/// A typed reference into the task document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Dotted path into the task document (`outputs.plan`, `subjects.issue.body`).
    pub from: String,
    #[serde(rename = "type", default)]
    pub ty: InputType,
    /// When true (default) a missing value pauses the task instead of rendering.
    #[serde(default = "yes")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    #[default]
    Any,
    String,
    Integer,
    Number,
    Boolean,
    Object,
    List,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// Path in the workspace, read after the step finishes.
    pub file: String,
    /// When set, the file is also kept as an artifact with this title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    /// `markdown`, `json` (parsed) or `text`. Default from the extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// A missing optional file is not an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResultSpec {
    /// JSON Schema of the result (the `submit_result` tool's input).
    pub schema: Value,
    /// Output name the result is stored under (default: the node id).
    #[serde(rename = "as", default, skip_serializing_if = "Option::is_none")]
    pub as_: Option<String>,
}

/// A named secret as written: `NAME` or `{name, scope, env | file}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SecretDecl {
    /// Project scope, injected as the env var `NAME`.
    Name(String),
    Full(Secret),
}

impl SecretDecl {
    /// The full form (bare name: project scope, env var of the same name;
    /// neither `env` nor `file`: env var named like the secret).
    pub fn normalized(&self) -> Secret {
        match self {
            SecretDecl::Name(n) => Secret {
                name: n.clone(),
                scope: SecretScope::Project,
                env: Some(n.clone()),
                file: None,
            },
            SecretDecl::Full(s) => {
                let mut s = s.clone();
                if s.env.is_none() && s.file.is_none() {
                    s.env = Some(s.name.clone());
                }
                s
            }
        }
    }
}

impl From<&str> for SecretDecl {
    fn from(s: &str) -> Self {
        SecretDecl::Name(s.into())
    }
}

/// A secret: where it is stored and how the process receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Secret {
    pub name: String,
    #[serde(default)]
    pub scope: SecretScope,
    /// Environment variable to set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// File (relative to the workspace root) to write the value to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretScope {
    /// The user the step acts for.
    User,
    #[default]
    Project,
    Instance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// `owner/repo`; a template.
    pub repo: String,
    pub mode: Access,
    /// Branch to work on and push (write workspaces only; a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Base ref to branch from or check out (a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Concurrency {
    /// Group key (a template, e.g. `{{ task.repo }}`).
    pub group: String,
    pub max: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Retry {
    /// Extra attempts.
    pub max: u32,
    /// Exits that are retried (default `failure`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
}

// ---------------------------------------------------------------- runtime

/// A named agent runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub install: Tools,
    pub launch: Launch,
    #[serde(default)]
    pub capabilities: Capabilities,
    /// Secrets the runtime needs (e.g. an API key), as in a step.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Launch {
    pub protocol: Protocol,
    /// argv of the agent process.
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Protocol {
    #[serde(rename = "acp")]
    Acp,
    #[serde(rename = "stdio-json")]
    StdioJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Capabilities {
    pub sessions: bool,
    pub resume: bool,
    pub steering: bool,
    pub interactive: bool,
    pub structured_output: bool,
}

// ---------------------------------------------------------------- board

/// A board: ordered columns and the default column per engine phase.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    #[serde(default)]
    pub defaults: BoardDefaults,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub id: String,
    pub title: String,
    /// WIP limit (display only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// Column used when the current node declares none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoardDefaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_human: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_external: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<String>,
}

impl BoardFile {
    /// Layers `over` on `self`: columns with the same id are replaced in
    /// place, new ones appended; each set default overrides.
    pub fn layer(mut self, over: &BoardFile) -> BoardFile {
        for c in &over.columns {
            match self.columns.iter_mut().find(|x| x.id == c.id) {
                Some(x) => *x = c.clone(),
                None => self.columns.push(c.clone()),
            }
        }
        let d = &over.defaults;
        let s = &mut self.defaults;
        for (dst, src) in [
            (&mut s.running, &d.running),
            (&mut s.waiting_human, &d.waiting_human),
            (&mut s.waiting_agent, &d.waiting_agent),
            (&mut s.waiting_external, &d.waiting_external),
            (&mut s.paused, &d.paused),
            (&mut s.ended, &d.ended),
        ] {
            if src.is_some() {
                dst.clone_from(src);
            }
        }
        self
    }
}

// ---------------------------------------------------------------- bindings

/// A bindings file: triggers mapped to workflows (library `bindings.yaml`,
/// Project `spec.workflows.bindings`, repo `.enzarb/workflows.yaml`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingsFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub imports: Imports,
    #[serde(default)]
    pub bindings: Vec<Binding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Name; a later layer's binding with the same id replaces this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The trigger. Required unless `disabled`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<Trigger>,
    /// MiniJinja expression over the routing context (`event`, `relations`).
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
    /// Params of the bound workflow. String leaves are templates over the
    /// routing context.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub with: Map<String, Value>,
    /// Workflow file reference. Required unless `disabled`.
    #[serde(rename = "use", default, skip_serializing_if = "Option::is_none")]
    pub use_: Option<String>,
    /// Removes an inherited binding with the same id.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Trigger {
    /// An event type (catalog or custom).
    Event(String),
    /// A cron schedule (fires `schedule` events).
    Schedule { schedule: String },
}

// ---------------------------------------------------------------- checks

/// A liveness check (`checks/*.yaml` in a library): a data-driven condition the engine's
/// periodic `platform` pass evaluates. While it holds, the engine keeps one **finding** open
/// per task (or per Project for `platform` scope); when it stops holding, the finding clears.
/// A finding is observation only: it never cancels, stops, restarts or times anything out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckFile {
    /// Format version. Always `1`.
    pub version: u32,
    /// Unique id; the finding key is `<id>:<task id>` (or `<id>:<project>`).
    pub id: String,
    /// `dead_wait` (a task waits for something that will not come) or `platform_failure`.
    pub kind: FindingKind,
    pub severity: Severity,
    /// `task` (evaluated per open task) or `platform` (per Project).
    #[serde(default)]
    pub scope: CheckScope,
    /// The finding text. `{task}`, `{node}`, `{phase}` and `{project}` are replaced.
    pub text: String,
    /// When this holds the finding is open.
    pub when: When,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    DeadWait,
    PlatformFailure,
}

impl FindingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::DeadWait => "dead_wait",
            FindingKind::PlatformFailure => "platform_failure",
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckScope {
    #[default]
    Task,
    Platform,
}

/// A condition: combinators over leaf predicates. No scripting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum When {
    /// Every condition holds.
    All(Vec<When>),
    /// At least one condition holds.
    Any(Vec<When>),
    /// The condition does not hold.
    Not(Box<When>),
    /// The task's `waiting_on` equals this (`external`, `human`, ...).
    WaitingOn(String),
    /// The task's current node (`phase/node`) equals this.
    Node(String),
    /// The task's phase equals this.
    Phase(String),
    /// A stored fact's value: `is: unknown` (never fetched or null), `is: false`, or any value.
    Fact(FactIs),
    /// The fact was refreshed to an unknown value at least this many times in a row.
    FactUnknownFor(FactUnknownFor),
    /// A count in a fact: `name` is the fact, optionally followed by a dotted path
    /// (`ci.check_runs`). A list counts its items, a number is itself, an absent value is not
    /// counted (the condition does not hold).
    FactCount(FactCount),
    /// No event reached the task, and/or no step ran for it, for this long.
    IdleFor(IdleFor),
    /// A platform fact compared with a value (reads through the engine's provider).
    Platform(PlatformCmp),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactIs {
    pub name: String,
    /// `unknown`, `false`, `true` or a literal value.
    pub is: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactUnknownFor {
    pub name: String,
    /// Consecutive unknown refreshes (at least 1).
    pub refreshes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FactCount {
    pub name: String,
    pub eq: u64,
}

/// At least one field. A duration is `90s`, `15m`, `2h`, `14d` or `1w`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdleFor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlatformCmp {
    pub fact: PlatformFactName,
    #[serde(default)]
    pub op: CmpOp,
    pub value: Value,
}

/// Platform facts the engine can read. A fact whose source is missing is unknown and never
/// matches. `chart.deploy_state` has no source yet and is always unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum PlatformFactName {
    #[serde(rename = "workflow_run.status")]
    WorkflowRunStatus,
    #[serde(rename = "chart.deploy_state")]
    ChartDeployState,
    #[serde(rename = "sandbox.disk_pct")]
    SandboxDiskPct,
    #[serde(rename = "sandbox.mem_pct")]
    SandboxMemPct,
    #[serde(rename = "project.pending_change")]
    ProjectPendingChange,
}

impl PlatformFactName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkflowRunStatus => "workflow_run.status",
            Self::ChartDeployState => "chart.deploy_state",
            Self::SandboxDiskPct => "sandbox.disk_pct",
            Self::SandboxMemPct => "sandbox.mem_pct",
            Self::ProjectPendingChange => "project.pending_change",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    #[default]
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

impl CheckFile {
    /// Static problems: unsupported version, empty ids or combinators, bad durations, scope
    /// mismatches, `refreshes: 0`.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.version != 1 {
            out.push(format!("unsupported version {} (expected 1)", self.version));
        }
        if self.id.trim().is_empty() || self.id.contains(':') {
            out.push("`id` must be non-empty and contain no `:`".into());
        }
        if self.text.trim().is_empty() {
            out.push("`text` must not be empty".into());
        }
        self.when.problems(self.scope, &mut out);
        out
    }
}

impl When {
    fn problems(&self, scope: CheckScope, out: &mut Vec<String>) {
        match self {
            When::All(v) | When::Any(v) => {
                if v.is_empty() {
                    out.push("`all`/`any` need at least one condition".into());
                }
                v.iter().for_each(|w| w.problems(scope, out));
            }
            When::Not(w) => w.problems(scope, out),
            When::WaitingOn(_) | When::Node(_) | When::Phase(_) | When::Fact(_) => {
                if scope == CheckScope::Platform {
                    out.push("task conditions need `scope: task`".into());
                }
            }
            When::FactUnknownFor(f) => {
                if f.refreshes == 0 {
                    out.push("`refreshes` must be at least 1".into());
                }
                if scope == CheckScope::Platform {
                    out.push("task conditions need `scope: task`".into());
                }
            }
            When::FactCount(_) => {
                if scope == CheckScope::Platform {
                    out.push("task conditions need `scope: task`".into());
                }
            }
            When::IdleFor(i) => {
                if i.no_event.is_none() && i.no_step.is_none() {
                    out.push("`idle_for` needs `no_event` and/or `no_step`".into());
                }
                for d in [&i.no_event, &i.no_step].into_iter().flatten() {
                    if parse_duration(d).is_none_or(|s| s == 0) {
                        out.push(format!("`{d}` is not a duration (90s, 15m, 2h, 14d, 1w)"));
                    }
                }
                if scope == CheckScope::Platform {
                    out.push("task conditions need `scope: task`".into());
                }
            }
            When::Platform(_) => {}
        }
    }
}
