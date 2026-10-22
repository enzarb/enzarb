//! Validation of a parsed workflows file (ADR-0029 §3).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet, VecDeque};

use crate::event::{KNOWN_EVENTS, valid_pattern};
use crate::expr::{Engine, NODE_ROOTS, TRIGGER_ROOTS, TemplateKind, root_of};
use crate::schema::{
    self, ActionKind, MAX_ID_LEN, MAX_MAX_RUNS, MAX_NODES, MAX_TIMEOUT, MAX_WORKFLOWS, Node,
    VERSION, Workflow, WorkflowsFile, parse_duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// One finding. `path` is the dotted TOML path (`workflows.fix.nodes.triage.mode`); empty
/// for the whole file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    /// No errors (warnings allowed).
    pub fn is_ok(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
    }

    fn error(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Error, path, message);
    }

    fn warn(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.push(Severity::Warning, path, message);
    }

    fn push(&mut self, severity: Severity, path: impl Into<String>, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            severity,
            path: path.into(),
            message: message.into(),
        });
    }
}

/// What validation needs to know about the modes of the same commit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModeCatalog {
    /// Mode id → its built-in `tools` allowlist (`None`: all built-ins), in file order.
    pub modes: IndexMap<String, Option<Vec<String>>>,
}

impl ModeCatalog {
    /// Reads the mode ids and tool lists from a `.ez/modes.toml`. ez-core validates the rest of
    /// the file; this only needs what workflows refer to.
    pub fn from_modes_toml(text: &str) -> Result<Self, String> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| format!("modes.toml: {}", e.to_string().trim_end()))?;
        let mut modes = IndexMap::new();
        if let Some(t) = table.get("modes") {
            let t = t.as_table().ok_or("modes.toml: `modes` must be a table")?;
            for (id, v) in t {
                let def = v
                    .as_table()
                    .ok_or_else(|| format!("modes.toml: mode `{id}` must be a table"))?;
                let tools = match def.get("tools") {
                    None => None,
                    Some(v) => Some(
                        v.as_array()
                            .ok_or_else(|| {
                                format!("modes.toml: `modes.{id}.tools` must be a list")
                            })?
                            .iter()
                            .filter_map(|t| t.as_str().map(str::to_string))
                            .collect(),
                    ),
                };
                modes.insert(id.clone(), tools);
            }
        }
        if modes.is_empty() {
            return Err("modes.toml defines no modes".into());
        }
        Ok(Self { modes })
    }

    pub fn contains(&self, id: &str) -> bool {
        self.modes.contains_key(id)
    }
}

/// Parses and validates in one go: the file (if it parsed) and every finding.
pub fn check(
    engine: &Engine,
    text: &str,
    modes: Option<&ModeCatalog>,
) -> (Option<WorkflowsFile>, Report) {
    match schema::parse(text) {
        Ok(file) => {
            let report = validate(engine, &file, modes);
            (Some(file), report)
        }
        Err(e) => {
            let mut r = Report::default();
            r.error("", e);
            (None, r)
        }
    }
}

/// Validates a parsed file against the modes of the same commit (`None`: the commit has no
/// `.ez/modes.toml`, which is an error for every agent node, ADR-0028 "No defaults").
pub fn validate(engine: &Engine, file: &WorkflowsFile, modes: Option<&ModeCatalog>) -> Report {
    let mut r = Report::default();
    if file.version != VERSION {
        r.error(
            "version",
            format!("unsupported version {}; expected {VERSION}", file.version),
        );
    }
    if file.workflows.len() > MAX_WORKFLOWS {
        r.error(
            "workflows",
            format!(
                "{} workflows; the limit is {MAX_WORKFLOWS}",
                file.workflows.len()
            ),
        );
    }
    for (id, wf) in &file.workflows {
        let path = format!("workflows.{id}");
        check_id(&mut r, &path, "workflow", id);
        validate_workflow(engine, &mut r, &path, wf, modes);
    }
    r
}

fn check_id(r: &mut Report, path: &str, what: &str, id: &str) {
    let ok = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        r.error(
            path,
            format!(
                "{what} id `{id}` must be 1-{MAX_ID_LEN} letters, digits, `_` or `-`, starting with a letter or digit"
            ),
        );
    }
}

fn check_patterns(r: &mut Report, path: &str, on: &[String]) {
    if on.is_empty() {
        r.error(path, "needs at least one event");
    }
    for (i, p) in on.iter().enumerate() {
        let path = format!("{path}[{i}]");
        if !valid_pattern(p) {
            r.error(path, format!("`{p}` is not `event` or `event.action`"));
        } else if !KNOWN_EVENTS.contains(&p.split('.').next().unwrap_or(p)) {
            r.warn(path, format!("`{p}` is not a known GitHub event"));
        }
    }
}

fn check_duration(r: &mut Report, path: &str, value: Option<&str>) {
    let Some(v) = value else { return };
    match parse_duration(v) {
        Ok(d) if d.is_zero() => r.error(path, "must be longer than zero"),
        Ok(d) if d > MAX_TIMEOUT => r.error(
            path,
            format!(
                "`{v}` is longer than the limit of {}",
                humantime::format_duration(MAX_TIMEOUT)
            ),
        ),
        Ok(_) => {}
        Err(e) => r.error(path, e),
    }
}

/// Checks a template or expression parses and reads only `roots` (and known node ids under
/// `nodes`).
struct Refs<'a> {
    engine: &'a Engine,
    nodes: &'a IndexMap<String, Node>,
}

impl Refs<'_> {
    fn template(&self, r: &mut Report, path: &str, kind: TemplateKind, src: &str, roots: &[&str]) {
        match self.engine.template_vars(kind, src) {
            Ok(vars) => self.vars(r, path, &vars, roots),
            Err(e) => r.error(path, e),
        }
    }

    fn expr(&self, r: &mut Report, path: &str, src: &str, roots: &[&str]) {
        match self.engine.expr_vars(src) {
            Ok(vars) => self.vars(r, path, &vars, roots),
            Err(e) => r.error(path, e),
        }
    }

    fn vars(&self, r: &mut Report, path: &str, vars: &BTreeSet<String>, roots: &[&str]) {
        let mut seen = HashSet::new();
        for v in vars {
            let root = root_of(v);
            if !roots.contains(&root) {
                if seen.insert(root.to_string()) {
                    r.error(
                        path,
                        format!(
                            "unknown variable `{root}` (available: {})",
                            roots.join(", ")
                        ),
                    );
                }
                continue;
            }
            if root == "nodes"
                && let Some(node) = v.split('.').nth(1)
                && !self.nodes.contains_key(node)
                && seen.insert(format!("nodes.{node}"))
            {
                r.error(path, format!("`nodes.{node}`: there is no node `{node}`"));
            }
        }
    }
}

fn validate_workflow(
    engine: &Engine,
    r: &mut Report,
    path: &str,
    wf: &Workflow,
    modes: Option<&ModeCatalog>,
) {
    let refs = Refs {
        engine,
        nodes: &wf.nodes,
    };
    check_patterns(r, &format!("{path}.on"), &wf.on);
    if let Some(c) = &wf.condition {
        refs.expr(r, &format!("{path}.if"), c, TRIGGER_ROOTS);
    }
    check_duration(r, &format!("{path}.timeout"), wf.timeout.as_deref());
    if let Some(m) = wf.max_runs
        && !(1..=MAX_MAX_RUNS).contains(&m)
    {
        r.error(
            format!("{path}.max_runs"),
            format!("must be 1-{MAX_MAX_RUNS}"),
        );
    }
    if wf.nodes.is_empty() {
        r.error(format!("{path}.nodes"), "needs at least one node");
        return;
    }
    if wf.nodes.len() > MAX_NODES {
        r.error(
            format!("{path}.nodes"),
            format!("{} nodes; the limit is {MAX_NODES}", wf.nodes.len()),
        );
    }
    if !wf.nodes.contains_key(&wf.start) {
        r.error(
            format!("{path}.start"),
            format!("there is no node `{}`", wf.start),
        );
    }

    let mut pushes = false;
    for (id, node) in &wf.nodes {
        let np = format!("{path}.nodes.{id}");
        check_id(r, &np, "node", id);
        validate_node(&refs, r, &np, node, modes);
        if let Node::Agent(a) = node {
            pushes |= a.push;
        }
        if let Some(next) = node.next() {
            let edges = next.edges();
            let ep = format!("{np}.next");
            if edges.is_empty() {
                r.error(&ep, "needs at least one edge");
            }
            let mut bare: Vec<&str> = NODE_ROOTS.to_vec();
            bare.extend_from_slice(node.node_type().outputs());
            for (i, e) in edges.iter().enumerate() {
                let p = format!("{ep}[{i}]");
                if !wf.nodes.contains_key(&e.to) {
                    r.error(format!("{p}.to"), format!("there is no node `{}`", e.to));
                }
                let last = i + 1 == edges.len();
                match (&e.condition, last) {
                    (Some(c), _) => refs.expr(r, &format!("{p}.if"), c, &bare),
                    (None, true) => {}
                    (None, false) => r.error(
                        &p,
                        "only the last edge may omit `if`; the edges after it never run",
                    ),
                }
                if last && e.condition.is_some() {
                    r.error(
                        &p,
                        "the last edge must not have `if`, so every outcome has a next node",
                    );
                }
            }
        }
        if let Some(m) = node_max_visits(node)
            && m == 0
        {
            r.error(format!("{np}.max_visits"), "must be at least 1");
        }
    }
    if !pushes
        && wf
            .nodes
            .values()
            .any(|n| matches!(n, Node::Action(a) if a.action == ActionKind::OpenPullRequest))
    {
        r.warn(
            format!("{path}.nodes"),
            "opens a pull request but no agent node has `push = true`",
        );
    }
    check_reachability(r, path, wf);
}

fn node_max_visits(node: &Node) -> Option<u32> {
    match node {
        Node::Agent(n) => n.max_visits,
        Node::Command(n) => n.max_visits,
        Node::Action(n) => n.max_visits,
        Node::Wait(n) => n.max_visits,
        Node::End(_) => None,
    }
}

fn validate_node(
    refs: &Refs<'_>,
    r: &mut Report,
    np: &str,
    node: &Node,
    modes: Option<&ModeCatalog>,
) {
    match node {
        Node::Agent(a) => {
            match modes {
                None => r.error(
                    format!("{np}.mode"),
                    "the commit has no `.ez/modes.toml`; agent nodes need it",
                ),
                Some(m) => match m.modes.get(&a.mode) {
                    None => r.error(
                        format!("{np}.mode"),
                        format!(
                            "there is no mode `{}` in .ez/modes.toml (modes: {})",
                            a.mode,
                            m.modes.keys().cloned().collect::<Vec<_>>().join(", ")
                        ),
                    ),
                    Some(Some(tools)) if tools.iter().any(|t| t == "AskUserQuestion") => r.warn(
                        format!("{np}.mode"),
                        format!(
                            "mode `{}` lists AskUserQuestion, which unattended runs never offer",
                            a.mode
                        ),
                    ),
                    Some(_) => {}
                },
            }
            if a.instruction.trim().is_empty() {
                r.error(format!("{np}.instruction"), "must not be empty");
            } else {
                refs.template(
                    r,
                    &format!("{np}.instruction"),
                    TemplateKind::Text,
                    &a.instruction,
                    NODE_ROOTS,
                );
            }
            if let Some(props) = &a.result {
                if props.is_empty() {
                    r.error(
                        format!("{np}.result"),
                        "needs at least one property, or omit it",
                    );
                }
                for (k, v) in props {
                    let ok_name = !k.is_empty()
                        && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        && !k.as_bytes()[0].is_ascii_digit();
                    if !ok_name {
                        r.error(
                            format!("{np}.result.{k}"),
                            "property names are letters, digits and `_`",
                        );
                    }
                    if !v.is_object() {
                        r.error(
                            format!("{np}.result.{k}"),
                            "must be a JSON Schema table (`{ type = \"string\" }`)",
                        );
                    }
                }
            }
        }
        Node::Command(c) => {
            match (&c.task, &c.run) {
                (Some(_), Some(_)) => r.error(np, "set `task` or `run`, not both"),
                (None, None) => r.error(np, "needs `task` (a mise task) or `run` (a shell string)"),
                (Some(t), None) if t.trim().is_empty() => {
                    r.error(format!("{np}.task"), "must not be empty")
                }
                (None, Some(s)) if s.trim().is_empty() => {
                    r.error(format!("{np}.run"), "must not be empty")
                }
                _ => {}
            }
            if c.run.is_some() && !c.args.is_empty() {
                r.error(
                    format!("{np}.args"),
                    "`args` go with `task`; put them in `run` instead",
                );
            }
            for (i, a) in c.args.iter().enumerate() {
                refs.template(
                    r,
                    &format!("{np}.args[{i}]"),
                    TemplateKind::Text,
                    a,
                    NODE_ROOTS,
                );
            }
            if let Some(s) = &c.run {
                refs.template(r, &format!("{np}.run"), TemplateKind::Shell, s, NODE_ROOTS);
            }
        }
        Node::Action(a) => {
            let need = |r: &mut Report, field: &str, v: &Option<String>| match v {
                Some(s) if !s.trim().is_empty() => refs.template(
                    r,
                    &format!("{np}.{field}"),
                    TemplateKind::Text,
                    s,
                    NODE_ROOTS,
                ),
                _ => r.error(
                    format!("{np}.{field}"),
                    format!("`{}` needs `{field}`", a.action.as_str()),
                ),
            };
            let forbid = |r: &mut Report, field: &str, set: bool| {
                if set {
                    r.error(
                        format!("{np}.{field}"),
                        format!("`{}` doesn't take `{field}`", a.action.as_str()),
                    );
                }
            };
            match a.action {
                ActionKind::Comment => {
                    need(r, "body", &a.body);
                    forbid(r, "title", a.title.is_some());
                    forbid(r, "labels", !a.labels.is_empty());
                    forbid(r, "draft", a.draft.is_some());
                }
                ActionKind::OpenPullRequest => {
                    need(r, "title", &a.title);
                    if let Some(b) = &a.body {
                        refs.template(r, &format!("{np}.body"), TemplateKind::Text, b, NODE_ROOTS);
                    }
                    forbid(r, "labels", !a.labels.is_empty());
                }
                ActionKind::AddLabels | ActionKind::RemoveLabels => {
                    if a.labels.is_empty() {
                        r.error(
                            format!("{np}.labels"),
                            format!("`{}` needs `labels`", a.action.as_str()),
                        );
                    }
                    for (i, l) in a.labels.iter().enumerate() {
                        refs.template(
                            r,
                            &format!("{np}.labels[{i}]"),
                            TemplateKind::Text,
                            l,
                            NODE_ROOTS,
                        );
                    }
                    forbid(r, "title", a.title.is_some());
                    forbid(r, "body", a.body.is_some());
                    forbid(r, "draft", a.draft.is_some());
                }
            }
        }
        Node::Wait(w) => {
            check_patterns(r, &format!("{np}.on"), &w.on);
            if let Some(c) = &w.condition {
                refs.expr(r, &format!("{np}.if"), c, NODE_ROOTS);
            }
            check_duration(r, &format!("{np}.timeout"), w.timeout.as_deref());
        }
        Node::End(_) => {}
    }
}

/// Unreachable nodes are warnings; a node from which no `end` is reachable is an error.
fn check_reachability(r: &mut Report, path: &str, wf: &Workflow) {
    let targets = |n: &Node| -> Vec<String> {
        n.next()
            .map(|nx| nx.edges().into_iter().map(|e| e.to).collect())
            .unwrap_or_default()
    };
    if wf.nodes.contains_key(&wf.start) {
        let mut seen = HashSet::from([wf.start.clone()]);
        let mut q = VecDeque::from([wf.start.clone()]);
        while let Some(id) = q.pop_front() {
            if let Some(n) = wf.nodes.get(&id) {
                for t in targets(n) {
                    if wf.nodes.contains_key(&t) && seen.insert(t.clone()) {
                        q.push_back(t);
                    }
                }
            }
        }
        for id in wf.nodes.keys().filter(|id| !seen.contains(*id)) {
            r.warn(
                format!("{path}.nodes.{id}"),
                format!("node `{id}` is not reachable from `{}`", wf.start),
            );
        }
    }
    // Reverse search from every end node.
    let mut can_end: HashSet<&str> = wf
        .nodes
        .iter()
        .filter(|(_, n)| matches!(n, Node::End(_)))
        .map(|(id, _)| id.as_str())
        .collect();
    if can_end.is_empty() {
        r.error(format!("{path}.nodes"), "needs at least one `end` node");
        return;
    }
    loop {
        let before = can_end.len();
        for (id, n) in &wf.nodes {
            if !can_end.contains(id.as_str())
                && targets(n).iter().any(|t| can_end.contains(t.as_str()))
            {
                can_end.insert(id.as_str());
            }
        }
        if can_end.len() == before {
            break;
        }
    }
    for id in wf.nodes.keys().filter(|id| !can_end.contains(id.as_str())) {
        r.error(
            format!("{path}.nodes.{id}"),
            format!("no `end` node can be reached from `{id}`"),
        );
    }
}

#[cfg(test)]
mod tests;
