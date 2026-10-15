//! Graph workflows (ADR-0020): the kernel side. A pure transition like the compiled
//! machines; the graph logic itself is `enzarb-graph`, shared with sandbox-init.
//!
//! * **Start.** An issue opened in a repository whose default branch has
//!   `.enzarb/workflows/issue.yaml` (read by the reducer through
//!   `Connector::get_file` at the branch head, [`Ctx::graph`]) starts a `graph:issue` task
//!   instead of the compiled `issue` machine. The definition and its sha are pinned in
//!   `state_data.graph` for the task's life. A definition that does not load, or uses a node
//!   this kernel cannot run yet, falls back to the compiled machine with a comment naming the
//!   problem. A definition whose `on` does not list `issue_opened` falls back silently.
//! * **Sandbox nodes** (`command`, `agent`): one `agent_segment` job; sandbox-init walks
//!   consecutive sandbox nodes and reports where it stopped (`RunFinished.graph`).
//! * **Kernel nodes:** `action` queues the connector job (`post_comment`,
//!   `open_change_request`) and continues on its result; `wait` suspends until a matching
//!   event (`comment`, `approval` with write access); `end` finishes the task.
//! * A change request opened by an `open_change_request` node is handed to the
//!   `change_request` workflow, as the compiled `issue` machine does.
//!
//! `work_items.state` is the [`GraphPhase`]; `state_data` is [`GraphData`].

use std::collections::BTreeMap;

use enzarb_connector::{
    ChangeRequestSpec, CommentKind, Event, EventData, EventType, ItemKind, ItemRef, WorkItemKey,
};
use enzarb_graph::{Definition, EndStatus, GraphState, Next, NodeKind};
use enzarb_proto::{GraphSegment, RunFinished};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::fx::{AgentSpec, Fx};
use super::prompt::{AgentOutput, PromptContext, render_graph_prompt};
use super::{
    Ctx, Effect, EngineEvent, EventPayload, JobKind, PRIORITY_COMMENT, Transition, UiActionKind,
    WorkflowKind, WorkflowState,
};

/// `work_items.workflow` prefix of graph workflows.
pub const PREFIX: &str = "graph:";
/// The trigger that starts an issue graph.
pub const ISSUE_TRIGGER: &str = "issue_opened";
/// Actions this kernel runs. `merge_change_request` validates but is not run yet.
pub const KERNEL_ACTIONS: &[&str] = &["post_comment", "open_change_request"];
/// Events a `wait` node can resume on in this kernel.
pub const KERNEL_EVENTS: &[&str] = &["comment", "approval"];
/// Where a repository keeps its graph definitions.
pub fn definition_path(name: &str) -> String {
    format!(".enzarb/workflows/{name}.yaml")
}

/// A definition read from a repository's default branch, before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphSource {
    pub name: String,
    pub path: String,
    /// The commit it was read at (the default branch head).
    pub sha: String,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphPhase {
    /// A job (segment run or action) is in flight.
    Running,
    /// At a `wait` node.
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl GraphPhase {
    /// There is no retry for graph tasks yet, so `Failed` is final too.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

/// `state_data.graph`: the pinned definition and the position in it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphPin {
    pub name: String,
    pub path: String,
    pub sha: String,
    /// The definition's YAML source, as read at `sha`.
    pub source: String,
    pub node: String,
    pub outputs: BTreeMap<String, Value>,
    pub visits: BTreeMap<String, u32>,
}

impl GraphPin {
    fn position(&self) -> GraphState {
        GraphState {
            node: self.node.clone(),
            outputs: self.outputs.clone(),
            visits: self.visits.clone(),
        }
    }

    fn set_position(&mut self, s: GraphState) {
        self.node = s.node;
        self.outputs = s.outputs;
        self.visits = s.visits;
    }
}

/// `work_items.state_data` of a graph task.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphData {
    pub graph: GraphPin,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub author: String,
    /// The job kind the current node waits on (`agent_segment` or an action's job).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_on: Option<JobKind>,
    /// Segment runs queued so far.
    pub segments: u32,
    /// The latest agent message (`RunFinished.summary` of the latest segment).
    pub summary: String,
    /// The branch the latest segment pushed.
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_request: Option<u64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphTask {
    pub phase: GraphPhase,
    pub data: GraphData,
}

/// Loads `src` and checks that this kernel can run every node.
pub fn load(src: &GraphSource) -> Result<Definition, String> {
    let def = enzarb_graph::load(&src.source).map_err(|e| e.to_string())?;
    let mut p = Vec::new();
    for (id, n) in &def.nodes {
        match &n.kind {
            NodeKind::Action { action, .. } if !KERNEL_ACTIONS.contains(&action.as_str()) => p
                .push(format!(
                    "node `{id}`: action `{action}` is not supported yet"
                )),
            NodeKind::Wait { events } => {
                for e in events {
                    if !KERNEL_EVENTS.contains(&e.as_str()) {
                        p.push(format!(
                            "node `{id}`: waiting on `{e}` is not supported yet"
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if p.is_empty() {
        Ok(def)
    } else {
        Err(format!("workflow definition is invalid: {}", p.join("; ")))
    }
}

fn branch(n: u64) -> String {
    format!("enzarb/issue-{n}")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max).collect::<String>())
    }
}

fn st(phase: GraphPhase, data: GraphData) -> WorkflowState {
    WorkflowState::Graph(GraphTask { phase, data })
}

fn prompt_ctx(ctx: &Ctx, d: &GraphData) -> PromptContext {
    PromptContext {
        bot_handle: ctx.bot_handle.clone(),
        repo: ctx.work_item.repo.to_string(),
        repo_name: ctx.work_item.repo.full_name(),
        item_kind: "issue".into(),
        number: d.number.to_string(),
        url: d.url.clone(),
        title: d.title.clone(),
        body: d.body.clone(),
        author: d.author.clone(),
        base_ref: ctx.default_branch.clone(),
        branch: branch(d.number),
        ..Default::default()
    }
}

/// The rendered prompt of every `agent` node, by node id.
pub fn segment_prompts(def: &Definition, pc: &PromptContext) -> BTreeMap<String, String> {
    def.nodes
        .iter()
        .filter_map(|(id, n)| match &n.kind {
            NodeKind::Agent { prompt } => Some((id.clone(), render_graph_prompt(prompt, pc))),
            _ => None,
        })
        .collect()
}

/// Starts the issue graph when [`Ctx::graph`] holds one for this trigger, else `legacy`.
pub(super) fn start_or(
    ctx: &Ctx,
    state: &WorkflowState,
    ev: &EngineEvent,
    legacy: fn(&Ctx, &WorkflowState, &EngineEvent) -> Transition,
) -> Transition {
    let Some(src) = &ctx.graph else {
        return legacy(ctx, state, ev);
    };
    match load(src) {
        Ok(def) if def.on.iter().any(|t| t == ISSUE_TRIGGER) => start(ctx, state, ev, src, &def),
        Ok(_) => legacy(ctx, state, ev),
        Err(e) => {
            let mut t = legacy(ctx, state, ev);
            if t.accepted {
                // Its own key: the legacy start may post a comment from this event too.
                t.effects.push(Effect::Job {
                    kind: JobKind::PostComment,
                    priority: PRIORITY_COMMENT,
                    idempotency_key: format!("{}:{}:graph_invalid", ctx.work_item, ev.id),
                    input: json!({
                        "item": ctx.work_item,
                        "body": format!(
                            "enzarb could not use `{}` (at {}), so it runs the built-in \
                             workflow instead.\n\n{}",
                            src.path,
                            src.sha,
                            truncate(&e, 2000)
                        ),
                    }),
                });
                t.note = format!("{}; graph definition rejected", t.note);
            }
            t
        }
    }
}

fn start(
    ctx: &Ctx,
    state: &WorkflowState,
    ev: &EngineEvent,
    src: &GraphSource,
    def: &Definition,
) -> Transition {
    let Some(e) = ev.connector() else {
        return Transition::reject(state, "not an opening event");
    };
    if e.event_type != EventType::WorkItemOpened || e.data.item.kind != ItemKind::Issue {
        return Transition::reject(state, "not an opening event");
    }
    if ctx.is_bot(&e.data.actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    let item = &e.data.item;
    let mut d = GraphData {
        graph: GraphPin {
            name: def.name.clone(),
            path: src.path.clone(),
            sha: src.sha.clone(),
            source: src.source.clone(),
            ..Default::default()
        },
        number: item.number,
        url: item.url.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        author: item.author.clone(),
        ..Default::default()
    };
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx));
    let (pos, next) = GraphState::begin(def);
    d.graph.set_position(pos);
    let phase = match next {
        Next::Node(_) => drive(ctx, &mut fx, def, &mut d),
        Next::Failed(r) => failed(ctx, &mut fx, &mut d, &r),
    };
    Transition::to(
        st(phase, d),
        fx.finish(),
        format!("graph `{}` started", def.name),
    )
}

fn target(ctx: &Ctx) -> enzarb_connector::ProgressTarget {
    ctx.progress_target(None, CommentKind::default())
}

fn failed(ctx: &Ctx, fx: &mut Fx, d: &mut GraphData, error: &str) -> GraphPhase {
    d.error = truncate(error, 500);
    d.waiting_on = None;
    fx.comment(format!("enzarb stopped while working on this: {}", d.error));
    fx.failed(&target(ctx));
    GraphPhase::Failed
}

/// Acts on the current node (already entered): queue its job, wait, or end.
fn drive(ctx: &Ctx, fx: &mut Fx, def: &Definition, d: &mut GraphData) -> GraphPhase {
    let id = d.graph.node.clone();
    let Some(node) = def.node(&id) else {
        return failed(ctx, fx, d, &format!("node `{id}` does not exist"));
    };
    match &node.kind {
        NodeKind::Command { .. } | NodeKind::Agent { .. } => {
            let pc = prompt_ctx(ctx, d);
            let segment = GraphSegment {
                // The YAML source keeps the order of conditions (ADR-0020 §1).
                definition: Value::String(d.graph.source.clone()),
                start: id,
                outputs: d.graph.outputs.clone(),
                visits: d.graph.visits.clone(),
                prompts: segment_prompts(def, &pc),
            };
            fx.segment(
                &target(ctx),
                AgentSpec {
                    base_ref: ctx.default_branch.clone(),
                    branch: branch(d.number),
                    push: true,
                },
                pc,
                segment,
            );
            d.segments += 1;
            d.waiting_on = Some(JobKind::AgentSegment);
            GraphPhase::Running
        }
        NodeKind::Wait { .. } => {
            d.waiting_on = None;
            GraphPhase::Waiting
        }
        NodeKind::Action { action, body } => match action.as_str() {
            "post_comment" => {
                let text = body.clone().unwrap_or_else(|| agent_body(&d.summary));
                fx.comment(text);
                d.waiting_on = Some(JobKind::PostComment);
                GraphPhase::Running
            }
            "open_change_request" => {
                let out = AgentOutput::parse(&d.summary).ok();
                let title = out
                    .as_ref()
                    .and_then(|o| o.title.clone())
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| d.title.clone());
                let body = format!(
                    "{}\n\nCloses #{}",
                    agent_body(&d.summary).trim_end(),
                    d.number
                );
                let head_ref = if d.branch.is_empty() {
                    branch(d.number)
                } else {
                    d.branch.clone()
                };
                fx.open_change_request(&ChangeRequestSpec {
                    repo: ctx.work_item.repo.clone(),
                    title,
                    body,
                    head_ref,
                    base_ref: ctx.default_branch.clone(),
                    draft: false,
                });
                d.waiting_on = Some(JobKind::OpenChangeRequest);
                GraphPhase::Running
            }
            other => failed(
                ctx,
                fx,
                d,
                &format!("action `{other}` is not supported yet"),
            ),
        },
        NodeKind::End { status } => {
            d.waiting_on = None;
            match status {
                EndStatus::Done => {
                    fx.done(&target(ctx));
                    GraphPhase::Done
                }
                EndStatus::Failed => {
                    let why = format!("the workflow ended at `{id}`");
                    failed(ctx, fx, d, &why)
                }
                EndStatus::Cancelled => GraphPhase::Cancelled,
            }
        }
    }
}

/// The `body` of the agent's final `json` block, else the whole message.
fn agent_body(summary: &str) -> String {
    match AgentOutput::parse(summary) {
        Ok(o) if !o.body.trim().is_empty() => o.body,
        _ => summary.to_string(),
    }
}

/// Records `output` for the current node, follows its edges and acts on the next node.
fn advance(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    def: &Definition,
    output: Value,
    note: &str,
) -> Transition {
    let mut d = s.data.clone();
    let mut fx = Fx::new(ctx, ev);
    let mut pos = d.graph.position();
    let phase = match enzarb_graph::advance(def, &mut pos, output) {
        Next::Node(_) => {
            d.graph.set_position(pos);
            drive(ctx, &mut fx, def, &mut d)
        }
        Next::Failed(r) => {
            d.graph.set_position(pos);
            failed(ctx, &mut fx, &mut d, &r)
        }
    };
    Transition::to(st(phase, d), fx.finish(), note)
}

fn cancel(ctx: &Ctx, ev: &EngineEvent, s: &GraphTask, comment: Option<String>) -> Transition {
    let mut d = s.data.clone();
    d.waiting_on = None;
    let mut fx = Fx::new(ctx, ev);
    fx.cancel_jobs();
    if let Some(c) = comment {
        fx.comment(c);
    }
    Transition::to(st(GraphPhase::Cancelled, d), fx.finish(), "cancelled")
}

pub(super) fn step(ctx: &Ctx, s: &GraphTask, ev: &EngineEvent) -> Transition {
    let state = WorkflowState::Graph(s.clone());
    if s.phase.is_terminal() {
        return Transition::reject(&state, "already finished");
    }
    if ev.closed().is_some() {
        return cancel(ctx, ev, s, None);
    }
    let def = match enzarb_graph::load(&s.data.graph.source) {
        Ok(d) => d,
        Err(e) => {
            let mut d = s.data.clone();
            let mut fx = Fx::new(ctx, ev);
            let phase = failed(ctx, &mut fx, &mut d, &format!("pinned definition: {e}"));
            return Transition::to(st(phase, d), fx.finish(), "failed");
        }
    };
    match &ev.payload {
        EventPayload::Ui(u) => match u.action {
            UiActionKind::Cancel if ctx.can_cancel() => {
                cancel(ctx, ev, s, ctx.cancel_comment(&u.actor.login))
            }
            UiActionKind::Cancel => Transition::reject(&state, "cancel requires write access"),
            UiActionKind::RepoRemoved => cancel(ctx, ev, s, None).repo_removed(),
            UiActionKind::Approve => wait_event(ctx, ev, s, &state, &def),
            UiActionKind::Comment => wait_event(ctx, ev, s, &state, &def),
            UiActionKind::Retry => {
                Transition::reject(&state, "retry is not supported for graph workflows yet")
            }
            UiActionKind::Merge => Transition::reject(&state, "nothing to merge"),
        },
        EventPayload::JobSucceeded { kind, output, .. } if s.data.waiting_on == Some(*kind) => {
            job_succeeded(ctx, ev, s, &def, *kind, output)
        }
        EventPayload::JobFailed { kind, error, .. } if s.data.waiting_on == Some(*kind) => {
            let mut d = s.data.clone();
            let mut fx = Fx::new(ctx, ev);
            let phase = failed(ctx, &mut fx, &mut d, error);
            Transition::to(st(phase, d), fx.finish(), "failed")
        }
        EventPayload::JobSucceeded { .. } | EventPayload::JobFailed { .. } => {
            Transition::ack(&state, "not the job this graph waits on")
        }
        EventPayload::Connector(_) => wait_event(ctx, ev, s, &state, &def),
        _ => Transition::reject(&state, "event not applicable to a graph workflow"),
    }
}

fn job_succeeded(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    def: &Definition,
    kind: JobKind,
    output: &Value,
) -> Transition {
    match kind {
        JobKind::AgentSegment => {
            let fin = serde_json::from_value::<RunFinished>(output.clone()).ok();
            let Some((fin, g)) = fin.and_then(|f| f.graph.clone().map(|g| (f, g))) else {
                let mut d = s.data.clone();
                let mut fx = Fx::new(ctx, ev);
                let phase = failed(
                    ctx,
                    &mut fx,
                    &mut d,
                    "the sandbox returned no graph result (is sandbox-container up to date?)",
                );
                return Transition::to(st(phase, d), fx.finish(), "failed");
            };
            let mut d = s.data.clone();
            d.summary = fin.summary;
            if fin.pushed {
                d.branch = fin.branch;
            }
            let mut fx = Fx::new(ctx, ev);
            let known = def.node(&g.stopped_at).is_some();
            d.graph.set_position(GraphState {
                node: g.stopped_at.clone(),
                outputs: g.outputs,
                visits: g.visits,
            });
            // The sandbox stops only at kernel nodes; anything else is a broken result.
            let phase = match def.node(&g.stopped_at).map(|n| n.kind.runner()) {
                Some(enzarb_graph::Runner::Kernel) => drive(ctx, &mut fx, def, &mut d),
                _ if !known => failed(
                    ctx,
                    &mut fx,
                    &mut d,
                    &format!("the segment stopped at unknown node `{}`", g.stopped_at),
                ),
                _ => failed(
                    ctx,
                    &mut fx,
                    &mut d,
                    &format!("the segment stopped at sandbox node `{}`", g.stopped_at),
                ),
            };
            Transition::to(st(phase, d), fx.finish(), "segment finished")
        }
        JobKind::OpenChangeRequest => {
            let Ok(pr) = serde_json::from_value::<ItemRef>(output.clone()) else {
                return advance(ctx, ev, s, def, output.clone(), "action finished");
            };
            let out = json!({"number": pr.number, "url": pr.url});
            let mut t = advance(ctx, ev, s, def, out, "change request opened");
            if let WorkflowState::Graph(g) = &mut t.next {
                g.data.change_request = Some(pr.number);
            }
            t.effects.push(handoff(ctx, ev, pr));
            t
        }
        _ => advance(ctx, ev, s, def, output.clone(), "action finished"),
    }
}

/// Hands an opened change request to the `change_request` workflow (as the compiled
/// `issue` machine does).
fn handoff(ctx: &Ctx, ev: &EngineEvent, pr: ItemRef) -> Effect {
    let key = WorkItemKey::new(ctx.work_item.repo.clone(), pr.number);
    let event = Event::new(
        "enzarb",
        format!("handoff-{}-{}", ctx.work_item, ev.id),
        EventType::ChangeRequestOpened,
        ctx.now,
        EventData {
            repo: ctx.work_item.repo.clone(),
            actor: enzarb_connector::Actor {
                login: ctx.bot_login.clone(),
                id: 0,
                is_bot: true,
            },
            item: ItemRef {
                kind: ItemKind::ChangeRequest,
                ..pr
            },
            comment: None,
            merged: None,
            review: None,
            ci: None,
            label: None,
        },
    );
    Effect::StartWorkflow {
        key,
        workflow: WorkflowKind::ChangeRequest,
        event: Box::new(event),
    }
}

/// An event while the task may be at a `wait` node.
fn wait_event(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Transition {
    if ev.actor().is_some_and(|a| ctx.is_bot(a)) {
        return Transition::reject(state, "bot actor ignored");
    }
    let events = match def.node(&s.data.graph.node).map(|n| &n.kind) {
        Some(NodeKind::Wait { events }) if s.phase == GraphPhase::Waiting => events,
        _ => return Transition::reject(state, "not waiting for an event"),
    };
    let wants = |e: &str| events.iter().any(|x| x == e);
    let login = ev.actor().map(|a| a.login.clone()).unwrap_or_default();
    if ev.is_approval() {
        if !wants("approval") {
            return Transition::reject(state, "not waiting for an approval");
        }
        if !ctx.can_write() {
            return Transition::reject(state, "approval requires write access");
        }
        let out = json!({"event": "approval", "author": login});
        return advance(ctx, ev, s, def, out, "approved");
    }
    if let Some(c) = ev.comment_like() {
        if !wants("comment") {
            return Transition::reject(state, "not waiting for a comment");
        }
        let out = json!({
            "event": "comment",
            "author": login,
            "body": c.text,
            "can_write": ctx.can_write(),
        });
        return advance(ctx, ev, s, def, out, "comment received");
    }
    Transition::reject(state, "event not applicable to a graph workflow")
}
