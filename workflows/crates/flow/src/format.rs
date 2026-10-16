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
    /// How often the phase may be entered (default 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<u32>,
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
    /// The next node. Not allowed on `end`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<Next>,
    /// How often this node may be entered per phase entry (default 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_visits: Option<u32>,
    /// Node entered instead once `max_visits` is exceeded. Without it the
    /// phase exits with `limit_reached`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_limit: Option<String>,
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
    /// Machine-readable wait reason shown in the projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Human text (a MiniJinja template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Board column id while waiting here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Who the task waits on. Default: `human` if the wait has actions or
    /// requires a writer, else `external`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<WaitingOn>,
    /// UI buttons; each posts its `/command` as the clicking user.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<UiAction>,
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
    pub seconds: u64,
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

/// A UI button at a wait.
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
    /// Arguments; strings are MiniJinja templates.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub args: Map<String, Value>,
    /// Task relations created together with the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relate: Option<Relate>,
}

/// Relations to create: `targets` is an expression yielding a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Relate {
    pub kind: String,
    pub targets: String,
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
    /// Names of secrets injected into this step's process.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    /// `owner/repo`; a template.
    pub repo: String,
    pub mode: Access,
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
    /// Secrets the runtime needs (e.g. an API key), by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
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
    /// MiniJinja expression over `event`.
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
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
