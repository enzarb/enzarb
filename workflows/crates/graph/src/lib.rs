//! Graph workflow definitions (ADR-0020): the YAML schema, validation, edge
//! conditions and [`advance`]. Pure: no IO, no async. The workflows kernel and
//! sandbox-init both use this crate, so both sides walk a graph the same way.
//!
//! A definition (`.enzarb/workflows/<name>.yaml` on the default branch):
//!
//! ```yaml
//! version: 1
//! name: issue
//! on: [issue_opened]
//! start: prepare
//! nodes:
//!   prepare:   {type: command, run: "mise run prepare", optional: true, next: implement}
//!   implement: {type: agent, prompt: implement, next: {pushed: open_pr, else: failed}}
//!   open_pr:   {type: action, action: open_change_request, next: done}
//!   failed:    {type: end, status: failed}
//!   done:      {type: end, status: done}
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use serde::Deserialize;
use serde::de::{self, MapAccess, Visitor};
use serde_json::Value;

pub mod expr;

pub use expr::{Expr, ExprError};

/// The only definition version.
pub const VERSION: u32 = 1;
/// `max_visits` when a node does not set it.
pub const DEFAULT_MAX_VISITS: u32 = 5;
/// The fallback key of a conditional `next`.
pub const ELSE: &str = "else";

/// Actions a definition may name.
pub const KNOWN_ACTIONS: &[&str] = &[
    "post_comment",
    "open_change_request",
    "merge_change_request",
];
/// The action that only a `wait` on `merge_requested` may lead to.
pub const MERGE_ACTION: &str = "merge_change_request";
/// Events a `wait` node may list.
pub const KNOWN_EVENTS: &[&str] = &[
    "comment",
    "approval",
    "ci",
    "review",
    "merge_requested",
    "dependencies_done",
    "timer",
];

/// A parsed workflow definition. Run [`validate`] (or use [`load`]) before
/// trusting it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Definition {
    pub version: u32,
    pub name: String,
    /// Triggers that create a task (e.g. `issue_opened`).
    #[serde(default)]
    pub on: Vec<String>,
    pub start: String,
    pub nodes: BTreeMap<String, Node>,
}

/// One node: its type-specific fields plus the common edge fields.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Node {
    #[serde(flatten)]
    pub kind: NodeKind,
    #[serde(default)]
    pub next: Option<Edges>,
    #[serde(default = "default_max_visits")]
    pub max_visits: u32,
    /// Where to go instead once `max_visits` is exceeded; absent ends the
    /// graph `failed`.
    #[serde(default)]
    pub on_limit: Option<String>,
    /// `command`/`agent`: a failure follows the edges instead of failing the graph.
    #[serde(default)]
    pub optional: bool,
}

fn default_max_visits() -> u32 {
    DEFAULT_MAX_VISITS
}

/// Node types. An unknown `type` is a parse error.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    /// A shell command in the run's worktree (sandbox).
    Command { run: String },
    /// An agent run (sandbox). `prompt` is a prompt name (one word) or inline
    /// prompt text.
    Agent { prompt: String },
    /// Suspends until one of `events` arrives (kernel). `from` says whose comments count.
    Wait {
        events: Vec<String>,
        #[serde(default)]
        from: WaitFrom,
    },
    /// A privileged connector call (kernel). `body` is the text of a
    /// `post_comment`; absent means the latest agent output's `body`.
    Action {
        action: String,
        #[serde(default)]
        body: Option<String>,
    },
    /// Terminal (kernel).
    End { status: EndStatus },
}

/// Whose `comment` resumes a `wait` node. Approvals always need write access.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitFrom {
    /// Users with write access (the default).
    #[default]
    Writers,
    /// Anyone but bots (opt-in).
    Anyone,
}

/// Terminal status of an `end` node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndStatus {
    Done,
    Failed,
    Cancelled,
}

/// Where a node runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runner {
    Sandbox,
    Kernel,
}

impl NodeKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Command { .. } => "command",
            Self::Agent { .. } => "agent",
            Self::Wait { .. } => "wait",
            Self::Action { .. } => "action",
            Self::End { .. } => "end",
        }
    }

    pub fn runner(&self) -> Runner {
        match self {
            Self::Command { .. } | Self::Agent { .. } => Runner::Sandbox,
            _ => Runner::Kernel,
        }
    }
}

/// A node's `next`: a node id, or conditions checked in order.
#[derive(Debug, Clone, PartialEq)]
pub enum Edges {
    To(String),
    /// `(condition, target)` in file order; `else` is a condition that always
    /// holds.
    Cond(Vec<(String, String)>),
}

impl Edges {
    /// Every target, in order.
    pub fn targets(&self) -> Vec<&str> {
        match self {
            Self::To(t) => vec![t.as_str()],
            Self::Cond(c) => c.iter().map(|(_, t)| t.as_str()).collect(),
        }
    }
}

impl<'de> Deserialize<'de> for Edges {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Edges;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a node id or a map from condition to node id")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Edges, E> {
                Ok(Edges::To(s.to_string()))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Edges, A::Error> {
                // A sequence keeps file order (a map type would sort it).
                let mut out = Vec::new();
                while let Some((k, v)) = m.next_entry::<CondKey, String>()? {
                    out.push((k.0, v));
                }
                Ok(Edges::Cond(out))
            }
        }
        d.deserialize_any(V)
    }
}

/// A condition key. YAML reads `true`/`false`/numbers as non-strings; keep
/// their source text.
struct CondKey(String);

impl<'de> Deserialize<'de> for CondKey {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = CondKey;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a condition")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<CondKey, E> {
                Ok(CondKey(s.to_string()))
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<CondKey, E> {
                Ok(CondKey(b.to_string()))
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<CondKey, E> {
                Ok(CondKey(n.to_string()))
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> Result<CondKey, E> {
                Ok(CondKey(n.to_string()))
            }
        }
        d.deserialize_any(V)
    }
}

/// A definition that does not load.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("workflow definition does not parse: {0}")]
    Parse(String),
    #[error("workflow definition is invalid: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

impl Definition {
    /// Parses YAML (JSON is YAML too).
    pub fn from_yaml(src: &str) -> Result<Self, Error> {
        serde_yaml_ng::from_str(src).map_err(|e| Error::Parse(e.to_string()))
    }

    /// Parses the wire form (`RunStart.graph.definition`): a string holding
    /// the YAML source (preferred: it keeps condition order), or an object.
    pub fn from_value(v: &Value) -> Result<Self, Error> {
        match v {
            Value::String(s) => Self::from_yaml(s),
            v => Self::deserialize(v).map_err(|e| Error::Parse(e.to_string())),
        }
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }
}

/// Parses and validates.
pub fn load(src: &str) -> Result<Definition, Error> {
    let d = Definition::from_yaml(src)?;
    validate(&d)?;
    Ok(d)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Checks every rule of ADR-0020 §1. Reports all problems at once.
pub fn validate(d: &Definition) -> Result<(), Error> {
    let mut p = Vec::new();
    if d.version != VERSION {
        p.push(format!(
            "unsupported version {} (expected {VERSION})",
            d.version
        ));
    }
    if d.name.trim().is_empty() {
        p.push("`name` is empty".into());
    }
    if d.on.is_empty() {
        p.push("`on` lists no triggers".into());
    }
    if d.nodes.is_empty() {
        p.push("no nodes".into());
    }
    if !d.nodes.contains_key(&d.start) {
        p.push(format!("start node `{}` does not exist", d.start));
    }
    for (id, n) in &d.nodes {
        if !valid_id(id) {
            p.push(format!("node id `{id}` must match [a-z0-9_-]{{1,64}}"));
        }
        match &n.kind {
            NodeKind::Command { run } if run.trim().is_empty() => {
                p.push(format!("node `{id}`: `run` is empty"))
            }
            NodeKind::Agent { prompt } if prompt.trim().is_empty() => {
                p.push(format!("node `{id}`: `prompt` is empty"))
            }
            NodeKind::Wait { events, .. } => {
                if events.is_empty() {
                    p.push(format!("node `{id}`: `events` is empty"));
                }
                for e in events {
                    if !KNOWN_EVENTS.contains(&e.as_str()) {
                        p.push(format!("node `{id}`: unknown event `{e}`"));
                    }
                }
            }
            NodeKind::Action { action, .. } if !KNOWN_ACTIONS.contains(&action.as_str()) => {
                p.push(format!("node `{id}`: unknown action `{action}`"))
            }
            _ => {}
        }
        let is_end = matches!(n.kind, NodeKind::End { .. });
        match (&n.next, is_end) {
            (Some(_), true) => p.push(format!("node `{id}`: an end node has no `next`")),
            (None, false) => p.push(format!("node `{id}`: `next` is missing")),
            _ => {}
        }
        if n.max_visits == 0 {
            p.push(format!("node `{id}`: `max_visits` must be at least 1"));
        }
        if n.optional && !matches!(n.kind, NodeKind::Command { .. } | NodeKind::Agent { .. }) {
            p.push(format!(
                "node `{id}`: only command and agent nodes can be optional"
            ));
        }
        if let Some(Edges::Cond(c)) = &n.next {
            if c.is_empty() {
                p.push(format!("node `{id}`: `next` has no conditions"));
            }
            for (i, (cond, _)) in c.iter().enumerate() {
                if cond == ELSE {
                    if i + 1 != c.len() {
                        p.push(format!("node `{id}`: `else` must be the last condition"));
                    }
                } else if let Err(e) = expr::parse(cond) {
                    p.push(format!("node `{id}`: {e}"));
                }
            }
            if c.last().is_some_and(|(k, _)| k != ELSE) {
                p.push(format!("node `{id}`: conditional `next` needs an `else`"));
            }
        }
        for t in edges_of(n) {
            if !d.nodes.contains_key(t) {
                p.push(format!("node `{id}`: edge to unknown node `{t}`"));
            }
        }
        if n.on_limit.as_deref() == Some(id.as_str()) {
            p.push(format!("node `{id}`: `on_limit` points at itself"));
        }
    }
    // Every node can reach an end: walk the edges backwards from the ends.
    let mut can_end: BTreeSet<&str> = d
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n.kind, NodeKind::End { .. }))
        .map(|(id, _)| id.as_str())
        .collect();
    let mut queue: VecDeque<&str> = can_end.iter().copied().collect();
    while let Some(t) = queue.pop_front() {
        for (id, n) in &d.nodes {
            if !can_end.contains(id.as_str()) && edges_of(n).any(|x| x == t) {
                can_end.insert(id);
                queue.push_back(id);
            }
        }
    }
    for id in d.nodes.keys() {
        if !can_end.contains(id.as_str()) {
            p.push(format!("node `{id}` cannot reach an end node"));
        }
    }
    // The merge gate: a merge is entered only from a wait on exactly
    // `merge_requested`, so a human request precedes every merge.
    for (mid, m) in &d.nodes {
        if !matches!(&m.kind, NodeKind::Action { action, .. } if action == MERGE_ACTION) {
            continue;
        }
        if *mid == d.start {
            p.push(format!("node `{mid}`: a merge cannot be the start node"));
        }
        for (id, n) in &d.nodes {
            if !edges_of(n).any(|t| t == mid) {
                continue;
            }
            let gated = matches!(&n.kind, NodeKind::Wait { events, .. }
                if events.iter().all(|e| e == "merge_requested") && !events.is_empty())
                && n.on_limit.as_deref() != Some(mid.as_str());
            if !gated {
                p.push(format!(
                    "node `{id}`: `{MERGE_ACTION}` (`{mid}`) is reachable only through a wait on `merge_requested`"
                ));
            }
        }
    }
    if p.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid(p))
    }
}

fn edges_of(n: &Node) -> impl Iterator<Item = &str> {
    n.next
        .iter()
        .flat_map(|e| e.targets())
        .chain(n.on_limit.as_deref())
}

/// The kernel nodes a segment starting at `start` can stop at: those reachable from it
/// through sandbox nodes only (edges and `on_limit`). The kernel rejects a segment result
/// that stops anywhere else, so a sandbox cannot skip a `wait`, gate or action.
pub fn segment_stops(def: &Definition, start: &str) -> BTreeSet<String> {
    let mut stops = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([start.to_string()]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(n) = def.node(&id) else { continue };
        if n.kind.runner() == Runner::Kernel {
            stops.insert(id);
            continue;
        }
        queue.extend(edges_of(n).map(str::to_string));
    }
    stops
}

/// The position of a running graph: the current node, every node's latest
/// output and visit counts. Persisted by the kernel, carried by segments.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, Deserialize)]
pub struct GraphState {
    pub node: String,
    #[serde(default)]
    pub outputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub visits: BTreeMap<String, u32>,
}

/// Result of entering a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// The graph is now at this node (`GraphState::node`); its visit is counted.
    Node(String),
    /// The graph ends `failed` without an end node (a failed non-optional
    /// sandbox step, no matching condition, or a visit limit with no `on_limit`).
    Failed(String),
}

impl GraphState {
    /// A fresh state entering `def.start`.
    pub fn begin(def: &Definition) -> (Self, Next) {
        let mut s = Self::default();
        let start = def.start.clone();
        let n = enter(def, &mut s, &start);
        (s, n)
    }
}

/// Whether a node output counts as success: its `ok` when present, else
/// `exit_code == 0` when present, else true.
pub fn output_ok(out: &Value) -> bool {
    if let Some(b) = out.get("ok").and_then(Value::as_bool) {
        return b;
    }
    match out.get("exit_code") {
        Some(c) => c.as_i64() == Some(0),
        None => true,
    }
}

/// Moves to `target`, counting the visit. Past `max_visits` it takes the
/// node's `on_limit` edge (counted the same way), or fails.
pub fn enter(def: &Definition, state: &mut GraphState, target: &str) -> Next {
    let mut target = target.to_string();
    // Each hop moves along an `on_limit`; bound it by the node count.
    for _ in 0..=def.nodes.len() {
        let Some(node) = def.node(&target) else {
            return Next::Failed(format!("node `{target}` does not exist"));
        };
        let v = state.visits.get(&target).copied().unwrap_or(0) + 1;
        if v <= node.max_visits {
            state.visits.insert(target.clone(), v);
            state.node = target.clone();
            return Next::Node(target);
        }
        match &node.on_limit {
            Some(l) => target = l.clone(),
            None => {
                return Next::Failed(format!(
                    "node `{target}` exceeded max_visits ({})",
                    node.max_visits
                ));
            }
        }
    }
    Next::Failed("on_limit edges loop".into())
}

/// Records `output` for the current node and follows its edges.
pub fn advance(def: &Definition, state: &mut GraphState, output: Value) -> Next {
    let cur = state.node.clone();
    let Some(node) = def.node(&cur) else {
        return Next::Failed(format!("node `{cur}` does not exist"));
    };
    let ok = output_ok(&output);
    state.outputs.insert(cur.clone(), output);
    let out = &state.outputs[&cur];
    // A failed sandbox step fails the graph, whatever its edges, unless it is optional;
    // an optional step's edges (conditions on `ok`, `exit_code`, ...) decide.
    if node.kind.runner() == Runner::Sandbox && !ok && !node.optional {
        return Next::Failed(format!("node `{cur}` failed"));
    }
    let target = match &node.next {
        None => return Next::Failed(format!("node `{cur}` has no `next`")),
        Some(Edges::To(t)) => t.clone(),
        Some(Edges::Cond(c)) => {
            let hit = c.iter().find(|(cond, _)| {
                cond == ELSE || expr::parse(cond).is_ok_and(|e| expr::eval(&e, out))
            });
            match hit {
                Some((_, t)) => t.clone(),
                None => return Next::Failed(format!("node `{cur}`: no condition matched")),
            }
        }
    };
    enter(def, state, &target)
}

#[cfg(test)]
mod tests;
