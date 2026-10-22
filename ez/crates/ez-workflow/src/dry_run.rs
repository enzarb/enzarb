//! Dry runs against logged events (ADR-0029 §6): no sandbox, no GitHub writes, no model calls.
//!
//! The walk uses [`RunState`] exactly as tracks does; where it needs an outcome it can't know
//! (an agent's result, a command's exit, a wait's event) it stops, unless the caller assumed
//! that node's outputs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::event::{Event, TriggerEval, evaluate_triggers};
use crate::expr::Engine;
use crate::run::{Rendered, RunInfo, RunState, RunStatus, Transition};
use crate::schema::{Edge, EndStatus, Node, NodeType, WorkflowsFile};
use crate::validate::{ModeCatalog, Report, validate};

/// Nodes one trace visits at most (loops end earlier through `max_visits`).
pub const DEFAULT_MAX_STEPS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DryRunOptions {
    /// Trace this workflow even if its trigger doesn't match the event.
    #[serde(default)]
    pub workflow: Option<String>,
    /// Assumed outputs per node id, used on every visit (`{"result": {...}}`,
    /// `{"exit_code": 0}`, `{"event": …, "timed_out": false}`).
    #[serde(default)]
    pub assume: BTreeMap<String, Value>,
    /// 0 means [`DEFAULT_MAX_STEPS`].
    #[serde(default)]
    pub max_steps: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DryRun {
    pub report: Report,
    /// Every workflow's trigger, in file order (empty when the file has errors).
    pub triggers: Vec<TriggerEval>,
    /// One per matching (or forced) workflow.
    pub traces: Vec<Trace>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub workflow: String,
    /// Traced because the caller named it, not because its trigger matched.
    pub forced: bool,
    pub steps: Vec<TraceStep>,
    pub end: TraceEnd,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceStep {
    pub node: String,
    pub node_type: NodeType,
    pub visit: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<Rendered>,
    /// The agent node's mode and its built-in tools (`None`: all built-ins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumed: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<Transition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeView {
    pub id: String,
    pub tools: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEnd {
    /// The run ended (an `end` node, or a failure such as `max_visits` or a failing edge).
    Ended {
        status: EndStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// The walk needs this node's outputs to continue: assume them to see further.
    NeedsOutcome {
        node: String,
        node_type: NodeType,
        outputs: Vec<String>,
        edges: Vec<Edge>,
    },
    StepLimit {
        steps: usize,
    },
}

/// Validates, evaluates triggers and traces every matching workflow for one event.
pub fn dry_run(
    engine: &Engine,
    file: &WorkflowsFile,
    modes: Option<&ModeCatalog>,
    event: &Event,
    opts: &DryRunOptions,
) -> DryRun {
    let report = validate(engine, file, modes);
    if !report.is_ok() {
        return DryRun {
            report,
            triggers: Vec::new(),
            traces: Vec::new(),
        };
    }
    let triggers = evaluate_triggers(engine, file, event);
    let mut traced: Vec<(String, bool)> = triggers
        .iter()
        .filter(|t| t.matched)
        .map(|t| (t.workflow.clone(), false))
        .collect();
    if let Some(w) = &opts.workflow
        && !traced.iter().any(|(id, _)| id == w)
    {
        traced.push((w.clone(), true));
    }
    let max_steps = if opts.max_steps == 0 {
        DEFAULT_MAX_STEPS
    } else {
        opts.max_steps
    };
    let traces = traced
        .into_iter()
        .map(|(w, forced)| {
            trace(
                engine,
                file,
                modes,
                event,
                &w,
                forced,
                &opts.assume,
                max_steps,
            )
        })
        .collect();
    DryRun {
        report,
        triggers,
        traces,
    }
}

#[allow(clippy::too_many_arguments)]
fn trace(
    engine: &Engine,
    file: &WorkflowsFile,
    modes: Option<&ModeCatalog>,
    event: &Event,
    workflow: &str,
    forced: bool,
    assume: &BTreeMap<String, Value>,
    max_steps: usize,
) -> Trace {
    let run = RunInfo {
        id: "dry-run".into(),
        attempt: 1,
        test: true,
    };
    let mut state = match RunState::start(file, workflow, event, run) {
        Ok(s) => s,
        Err(e) => {
            return Trace {
                workflow: workflow.into(),
                forced,
                steps: Vec::new(),
                end: TraceEnd::Ended {
                    status: EndStatus::Failed,
                    node: None,
                    reason: Some(e.to_string()),
                },
            };
        }
    };
    let wf = &file.workflows[workflow];
    let mut steps = Vec::new();
    loop {
        let id = match &state.status {
            RunStatus::Ended {
                status,
                node,
                reason,
            } => {
                if let Some(n) = node {
                    steps.push(TraceStep {
                        node: n.clone(),
                        node_type: NodeType::End,
                        visit: state.visits.get(n).copied().unwrap_or(1),
                        rendered: Some(Rendered::End { status: *status }),
                        mode: None,
                        assumed: None,
                        transition: None,
                    });
                }
                let end = TraceEnd::Ended {
                    status: *status,
                    node: node.clone(),
                    reason: reason.clone(),
                };
                return done(workflow, forced, steps, end);
            }
            RunStatus::At { node } => node.clone(),
        };
        if steps.len() >= max_steps {
            return done(
                workflow,
                forced,
                steps,
                TraceEnd::StepLimit { steps: max_steps },
            );
        }
        let node = &wf.nodes[&id];
        let mode = match node {
            Node::Agent(a) => Some(ModeView {
                id: a.mode.clone(),
                tools: modes.and_then(|m| m.modes.get(&a.mode).cloned().flatten()),
            }),
            _ => None,
        };
        let mut step = TraceStep {
            node: id.clone(),
            node_type: node.node_type(),
            visit: state.visits.get(&id).copied().unwrap_or(1),
            rendered: None,
            mode,
            assumed: None,
            transition: None,
        };
        match state.render(engine, file) {
            Ok(Ok(r)) => step.rendered = Some(r),
            Ok(Err(msg)) => state.fail(msg),
            Err(e) => state.fail(e.to_string()),
        }
        if state.is_ended() {
            steps.push(step);
            continue;
        }
        let Some(outputs) = assume.get(&id) else {
            let end = TraceEnd::NeedsOutcome {
                node: id.clone(),
                node_type: node.node_type(),
                outputs: node
                    .node_type()
                    .outputs()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                edges: node.next().map(|n| n.edges()).unwrap_or_default(),
            };
            steps.push(step);
            return done(workflow, forced, steps, end);
        };
        step.assumed = Some(outputs.clone());
        match state.advance(engine, file, outputs.clone()) {
            Ok(t) => step.transition = Some(t),
            Err(e) => state.fail(e.to_string()),
        }
        steps.push(step);
    }
}

fn done(workflow: &str, forced: bool, steps: Vec<TraceStep>, end: TraceEnd) -> Trace {
    Trace {
        workflow: workflow.into(),
        forced,
        steps,
        end,
    }
}
