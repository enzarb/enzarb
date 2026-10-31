//! A run's durable state and how it moves through the graph (`advance`).
//!
//! tracks stores [`RunState`] as JSON, executes what [`RunState::render`] describes, and feeds
//! the outcome back to [`RunState::advance`]. The dry run drives the same functions with
//! assumed outcomes, so both walk a graph identically.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use crate::event::{Event, Subject};
use crate::expr::{Engine, TemplateKind};
use crate::schema::{ActionKind, EndNode, EndStatus, Node, NodeType, Workflow, WorkflowsFile};

/// Identity of a run, visible to templates as `run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInfo {
    pub id: String,
    #[serde(default = "one")]
    pub attempt: u32,
    /// A test run: nothing is pushed or posted (ADR-0029 §6).
    #[serde(default)]
    pub test: bool,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunStatus {
    /// At `node`: executing it, or waiting in it.
    At { node: String },
    /// Over. `node` is the `end` node, if one was reached; `reason` explains failures.
    Ended {
        status: EndStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

/// Everything needed to continue a run; tracks persists it per run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub workflow: String,
    pub run: RunInfo,
    /// The `event` variable: the trigger, or the event that resumed the last wait.
    pub event: Value,
    pub subject: Option<Subject>,
    /// Latest outputs per node id (the `nodes` variable).
    pub nodes: Map<String, Value>,
    /// The workflow's `vars` as of the run's start (the `vars` variable).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub vars: Map<String, Value>,
    pub visits: BTreeMap<String, u32>,
    /// The node the last edge was taken from (for the generic end reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub status: RunStatus,
}

/// What [`RunState::advance`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transition {
    /// Took edge number `edge` of `from`'s `next` to `to` (which may be an `end` node; then the
    /// run's status is `Ended`).
    Moved {
        from: String,
        to: String,
        edge: usize,
    },
    /// The run ended without taking an edge (an edge failed, or `max_visits`).
    Ended {
        status: EndStatus,
        reason: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    UnknownWorkflow(String),
    UnknownNode(String),
    NotRunning,
    /// `advance` on an `end` node, or `wait_accepts` on a node that isn't a `wait`.
    WrongNodeType {
        node: String,
        node_type: NodeType,
    },
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownWorkflow(w) => write!(f, "there is no workflow `{w}`"),
            Self::UnknownNode(n) => write!(f, "there is no node `{n}`"),
            Self::NotRunning => f.write_str("the run has ended"),
            Self::WrongNodeType { node, node_type } => {
                write!(f, "node `{node}` is a {} node", node_type.as_str())
            }
        }
    }
}

impl std::error::Error for RunError {}

/// The current node with every template rendered: what tracks executes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Rendered {
    Agent {
        mode: String,
        instruction: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_schema: Option<Value>,
        /// Already false in test runs.
        push: bool,
    },
    Command {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        /// The rendered shell script (`run`), values quoted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        script: Option<String>,
    },
    Action {
        action: ActionKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        labels: Vec<String>,
        draft: bool,
    },
    Wait {
        on: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_secs: Option<u64>,
    },
    End {
        status: EndStatus,
        /// The rendered `reason`, or for `failed`/`cancelled` ends without one a generic
        /// reason; empty for `done`.
        #[serde(default)]
        reason: String,
    },
}

/// The outputs a `wait` node records: the event that resumed it, or a timeout.
pub fn wait_outputs(event: Option<&Event>) -> Value {
    match event {
        Some(e) => json!({ "event": e.context(), "timed_out": false }),
        None => json!({ "event": null, "timed_out": true }),
    }
}

impl RunState {
    /// A new run of `workflow` triggered by `event`, at its `start` node.
    pub fn start(
        file: &WorkflowsFile,
        workflow: &str,
        event: &Event,
        run: RunInfo,
    ) -> Result<Self, RunError> {
        let wf = file
            .workflows
            .get(workflow)
            .ok_or_else(|| RunError::UnknownWorkflow(workflow.into()))?;
        if !wf.nodes.contains_key(&wf.start) {
            return Err(RunError::UnknownNode(wf.start.clone()));
        }
        let mut state = Self {
            workflow: workflow.into(),
            run,
            event: event.context(),
            subject: event.subject(),
            nodes: Map::new(),
            vars: wf
                .vars
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            visits: BTreeMap::new(),
            previous: None,
            status: RunStatus::At {
                node: wf.start.clone(),
            },
        };
        state.enter(wf, &wf.start.clone());
        Ok(state)
    }

    /// The current node id, while running.
    pub fn current(&self) -> Option<&str> {
        match &self.status {
            RunStatus::At { node } => Some(node),
            RunStatus::Ended { .. } => None,
        }
    }

    pub fn is_ended(&self) -> bool {
        matches!(self.status, RunStatus::Ended { .. })
    }

    /// The template context: `event`, `subject`, `nodes`, `run`, `vars`.
    pub fn context(&self) -> Value {
        json!({
            "event": self.event,
            "subject": self.subject,
            "nodes": self.nodes,
            "run": self.run,
            "vars": self.vars,
        })
    }

    fn workflow<'a>(&self, file: &'a WorkflowsFile) -> Result<&'a Workflow, RunError> {
        file.workflows
            .get(&self.workflow)
            .ok_or_else(|| RunError::UnknownWorkflow(self.workflow.clone()))
    }

    fn current_node<'a>(
        &self,
        file: &'a WorkflowsFile,
    ) -> Result<(&'a Workflow, String, &'a Node), RunError> {
        let wf = self.workflow(file)?;
        let id = self.current().ok_or(RunError::NotRunning)?.to_string();
        let node = wf
            .nodes
            .get(&id)
            .ok_or_else(|| RunError::UnknownNode(id.clone()))?;
        Ok((wf, id, node))
    }

    /// Renders the current node's templates. A template that fails at run time is
    /// `Ok(Err(msg))`; the caller ends the run with [`RunState::fail`].
    pub fn render(
        &self,
        engine: &Engine,
        file: &WorkflowsFile,
    ) -> Result<Result<Rendered, String>, RunError> {
        let (_, id, node) = self.current_node(file)?;
        let ctx = self.context();
        let text = |field: &str, src: &str| {
            engine
                .render(TemplateKind::Text, src, &ctx)
                .map_err(|e| format!("node `{id}` {field}: {e}"))
        };
        let opt =
            |field: &str, src: &Option<String>| src.as_deref().map(|s| text(field, s)).transpose();
        let list = |field: &str, srcs: &[String]| {
            srcs.iter()
                .enumerate()
                .map(|(i, s)| text(&format!("{field}[{i}]"), s))
                .collect::<Result<Vec<_>, _>>()
        };
        let rendered = (|| -> Result<Rendered, String> {
            Ok(match node {
                Node::Agent(a) => Rendered::Agent {
                    mode: a.mode.clone(),
                    instruction: text("instruction", &a.instruction)?,
                    result_schema: a.result_schema(),
                    push: a.push && !self.run.test,
                },
                Node::Command(c) => Rendered::Command {
                    task: c.task.clone(),
                    args: list("args", &c.args)?,
                    script: c
                        .run
                        .as_deref()
                        .map(|s| {
                            engine
                                .render(TemplateKind::Shell, s, &ctx)
                                .map_err(|e| format!("node `{id}` run: {e}"))
                        })
                        .transpose()?,
                },
                Node::Action(a) => Rendered::Action {
                    action: a.action,
                    title: opt("title", &a.title)?,
                    body: opt("body", &a.body)?,
                    labels: list("labels", &a.labels)?,
                    draft: a.draft.unwrap_or(false),
                },
                Node::Wait(w) => Rendered::Wait {
                    on: w.on.clone(),
                    condition: w.condition.clone(),
                    timeout_secs: w.wait_timeout().map(|d| d.as_secs()),
                },
                Node::End(e) => Rendered::End {
                    status: e.status,
                    reason: self.end_reason(engine, &id, e)?,
                },
            })
        })();
        Ok(rendered)
    }

    /// The reason an `end` node records: its rendered `reason`, else for `failed`/`cancelled` a
    /// generic one naming the node and the one before it; empty for `done`.
    fn end_reason(&self, engine: &Engine, id: &str, e: &EndNode) -> Result<String, String> {
        let custom = match &e.reason {
            Some(src) => engine
                .render(TemplateKind::Text, src, &self.context())
                .map_err(|m| format!("node `{id}` reason: {m}"))?,
            None => String::new(),
        };
        let custom = custom.trim();
        Ok(if !custom.is_empty() {
            custom.to_string()
        } else if e.status == EndStatus::Done {
            String::new()
        } else {
            match &self.previous {
                Some(p) => format!("ended at `{id}` ({}) after `{p}`", e.status.as_str()),
                None => format!("ended at `{id}` ({})", e.status.as_str()),
            }
        })
    }

    /// Whether `event` resumes the current `wait` node (`on` and `if`). The caller has already
    /// matched the event's subject to the run's. An `if` that fails to evaluate is `Err`.
    pub fn wait_accepts(
        &self,
        engine: &Engine,
        file: &WorkflowsFile,
        event: &Event,
    ) -> Result<Result<bool, String>, RunError> {
        let (_, id, node) = self.current_node(file)?;
        let Node::Wait(w) = node else {
            return Err(RunError::WrongNodeType {
                node: id,
                node_type: node.node_type(),
            });
        };
        if !w.on.iter().any(|p| event.matches(p)) {
            return Ok(Ok(false));
        }
        let Some(cond) = &w.condition else {
            return Ok(Ok(true));
        };
        let mut ctx = self.context();
        ctx["event"] = event.context();
        Ok(engine
            .eval_bool(cond, &ctx)
            .map_err(|e| format!("node `{id}` if: {e}")))
    }

    /// Records the current node's outputs and follows its first matching edge.
    pub fn advance(
        &mut self,
        engine: &Engine,
        file: &WorkflowsFile,
        outputs: Value,
    ) -> Result<Transition, RunError> {
        let (wf, id, node) = self.current_node(file)?;
        let Some(next) = node.next() else {
            return Err(RunError::WrongNodeType {
                node: id,
                node_type: node.node_type(),
            });
        };
        if matches!(node, Node::Wait(_))
            && let Some(ev) = outputs.get("event").filter(|e| e.is_object())
        {
            self.event = ev.clone();
        }
        self.nodes.insert(id.clone(), outputs.clone());

        let mut ctx = self.context();
        if let (Value::Object(c), Value::Object(o)) = (&mut ctx, &outputs) {
            for (k, v) in o {
                c.insert(k.clone(), v.clone());
            }
        }
        for (i, edge) in next.edges().iter().enumerate() {
            let take = match &edge.condition {
                None => true,
                Some(cond) => match engine.eval_bool(cond, &ctx) {
                    Ok(b) => b,
                    Err(e) => {
                        let reason = format!("node `{id}` next[{i}] if `{cond}`: {e}");
                        return Ok(self.end(EndStatus::Failed, None, Some(reason)));
                    }
                },
            };
            if take {
                if !wf.nodes.contains_key(&edge.to) {
                    let reason = format!("node `{id}` next[{i}]: there is no node `{}`", edge.to);
                    return Ok(self.end(EndStatus::Failed, None, Some(reason)));
                }
                let to = edge.to.clone();
                self.previous = Some(id.clone());
                self.enter(wf, &to);
                if let (RunStatus::Ended { node: Some(n), .. }, Some(Node::End(e))) =
                    (&self.status, wf.nodes.get(&to))
                {
                    let n = n.clone();
                    let (reason, ok) = match self.end_reason(engine, &n, e) {
                        Ok(r) => (r, true),
                        Err(m) => (m, false),
                    };
                    let status = if ok { e.status } else { EndStatus::Failed };
                    self.status = RunStatus::Ended {
                        status,
                        node: Some(n),
                        reason: (!reason.is_empty()).then_some(reason),
                    };
                }
                if let RunStatus::Ended {
                    status,
                    node: None,
                    reason,
                } = &self.status
                {
                    return Ok(Transition::Ended {
                        status: *status,
                        reason: reason.clone(),
                    });
                }
                return Ok(Transition::Moved {
                    from: id,
                    to,
                    edge: i,
                });
            }
        }
        let reason = format!("node `{id}`: no edge matched");
        Ok(self.end(EndStatus::Failed, None, Some(reason)))
    }

    /// Ends the run as failed (a step error, a render error, a timeout).
    pub fn fail(&mut self, reason: impl Into<String>) {
        if !self.is_ended() {
            self.end(EndStatus::Failed, None, Some(reason.into()));
        }
    }

    /// Ends the run as cancelled (Stop, or the workflow was removed).
    pub fn cancel(&mut self, reason: impl Into<String>) {
        if !self.is_ended() {
            self.end(EndStatus::Cancelled, None, Some(reason.into()));
        }
    }

    fn end(
        &mut self,
        status: EndStatus,
        node: Option<String>,
        reason: Option<String>,
    ) -> Transition {
        self.status = RunStatus::Ended {
            status,
            node,
            reason: reason.clone(),
        };
        Transition::Ended { status, reason }
    }

    /// Counts a visit to `id`; ends the run past `max_visits` or on an `end` node.
    fn enter(&mut self, wf: &Workflow, id: &str) {
        let Some(node) = wf.nodes.get(id) else {
            self.end(
                EndStatus::Failed,
                None,
                Some(format!("there is no node `{id}`")),
            );
            return;
        };
        let visits = self.visits.entry(id.to_string()).or_insert(0);
        *visits += 1;
        if *visits > node.max_visits() {
            let reason = format!("node `{id}` reached max_visits ({})", node.max_visits());
            self.end(EndStatus::Failed, None, Some(reason));
            return;
        }
        self.status = match node {
            Node::End(e) => RunStatus::Ended {
                status: e.status,
                node: Some(id.to_string()),
                reason: None,
            },
            _ => RunStatus::At {
                node: id.to_string(),
            },
        };
    }
}

#[cfg(test)]
mod tests;
