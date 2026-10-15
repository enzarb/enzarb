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
//!   event (`comment` from a writer, or anyone with `from: anyone`; `approval` with write
//!   access); `end` finishes the task.
//! * A change request opened by an `open_change_request` node is handed to the
//!   `change_request` workflow, as the compiled `issue` machine does.
//!
//! `work_items.state` is the [`GraphPhase`]; `state_data` is [`GraphData`].

use std::collections::BTreeMap;

use enzarb_connector::{
    ChangeRequestSpec, CommentKind, Event, EventData, EventType, ItemKind, ItemRef, WorkItemKey,
};
use enzarb_graph::{Definition, EndStatus, GraphState, Next, NodeKind, Runner, WaitFrom};
use enzarb_proto::{GraphSegment, RunFinished};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::change_request::OpenDependency;
use super::fx::{AgentSpec, Fx};
use super::issue::CreatedChild;
use super::prompt::{AgentOutput, PromptContext, SubIssue, parse_epic_plan, render_graph_prompt};
use super::{
    Ctx, Effect, EngineEvent, EventPayload, JobKind, PRIORITY_COMMENT, Transition, UiActionKind,
    WorkflowKind, WorkflowState,
};

/// `work_items.workflow` prefix of graph workflows.
pub const PREFIX: &str = "graph:";
/// The trigger that starts an issue graph.
pub const ISSUE_TRIGGER: &str = "issue_opened";
/// Actions this kernel runs. `merge_change_request` validates but is not run yet.
pub const KERNEL_ACTIONS: &[&str] = &["post_comment", "open_change_request", "create_sub_issues"];
/// Events a `wait` node can resume on in this kernel.
pub const KERNEL_EVENTS: &[&str] = &["comment", "approval", "children_done", "dependencies_done"];
/// Where a repository keeps its graph definitions.
pub fn definition_path(name: &str) -> String {
    format!(".enzarb/workflows/{name}.yaml")
}

/// A dependency of a graph task that is not completed (read by the reducer).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BlockingDependency {
    pub key: String,
    /// `#12`, or `owner/repo#12` in another repository.
    pub label: String,
    pub title: String,
    /// The dependency's work item state; empty when enzarb does not track it.
    pub state: String,
    pub url: String,
    /// Its change request, when it has one.
    pub pr_url: String,
    /// It can no longer complete by itself (closed, cancelled, failed): a human must act.
    pub failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyProgress {
    Completed,
    Open,
    Failed,
}

/// Whether a dependency is completed: its change request is merged, or (without one) it
/// finished `Done` (an epic, or work that needed no change request). An open change request
/// is not completed. A closed or cancelled change request, or a dependency that failed, was
/// cancelled, closed or is a duplicate, cannot complete by itself.
pub fn dependency_progress(
    workflow: Option<&str>,
    state: Option<&str>,
    cr_state: Option<&str>,
) -> DependencyProgress {
    use DependencyProgress::*;
    match (workflow, state, cr_state) {
        (None, _, _) => Open,
        (_, _, Some("Merged")) => Completed,
        (_, _, Some("Closed" | "Cancelled")) => Failed,
        (_, _, Some(_)) => Open,
        (_, Some("Done" | "Merged"), None) => Completed,
        (_, Some("Cancelled" | "Closed" | "Duplicate" | "Failed"), None) => Failed,
        _ => Open,
    }
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
    /// Waiting for dependencies to complete before `start`, or before a segment that can run
    /// an agent that pushes (`blocked_resume`). Not a definition node: the kernel enforces it.
    Blocked,
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
    /// The latest approved-or-posted plan (`{plan}` in prompts): the body of the latest
    /// `plan` decision, or the compiled machine's plan of an adopted issue.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plan: String,
    /// The latest agent message (`RunFinished.summary` of the latest segment).
    pub summary: String,
    /// The branch the latest segment pushed.
    pub branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_request: Option<u64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Epics (ADR-0018 as graph nodes). `epic`: the item is to be broken into sub-issues
    /// (triage `epic`, or `/epic` / the `epic` label from a writer); exposed to conditions as
    /// the item facts `epic` and `epic_allowed`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub epic: bool,
    /// Nesting depth (`0` for a top-level issue); from the stored parent for a sub-issue.
    #[serde(skip_serializing_if = "is_zero")]
    pub epic_depth: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub epic_parent: Option<String>,
    /// The approved breakdown, in creation order (from the latest plan).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sub_issues: Vec<SubIssue>,
    /// The sub-issues created.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<CreatedChild>,
    /// Dependencies not done yet, as last read (`DependenciesChanged`; the ticker compares).
    pub open_dependencies: Vec<OpenDependency>,
    /// At `create_sub_issues` or a wait on `children_done`/`dependencies_done`: a sub-issue
    /// that names this item as its epic is accepted.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub accepts_children: bool,
    /// `Blocked`: the dependencies not completed, as last read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<BlockingDependency>,
    /// `Blocked`: the node to resume at once they complete; `None` is `start`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_resume: Option<String>,
    /// Why the task waits (`{waiting_on, reason, text}`, contracts §6), set while `Blocked`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<Value>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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
            NodeKind::Wait { events, .. } => {
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
        plan: d.plan.clone(),
        epic: d.epic,
        epic_allowed: if epic_allowed(ctx, d) { "yes" } else { "no" }.into(),
        max_subissues: ctx.config.max_epic_children.to_string(),
        ..Default::default()
    }
}

/// Below the configured depth an issue may be an epic.
fn epic_allowed(ctx: &Ctx, d: &GraphData) -> bool {
    d.epic_depth < ctx.config.max_epic_depth
}

/// Writes the item facts conditions can read ([`enzarb_graph::ITEM`]).
fn set_item(ctx: &Ctx, d: &mut GraphData) {
    let allowed = epic_allowed(ctx, d);
    d.graph.outputs.insert(
        enzarb_graph::ITEM.into(),
        json!({"epic": d.epic && allowed, "epic_allowed": allowed, "epic_depth": d.epic_depth}),
    );
}

fn waits_on_children(n: &enzarb_graph::Node) -> bool {
    match &n.kind {
        NodeKind::Wait { events, .. } => events
            .iter()
            .any(|e| e == "children_done" || e == "dependencies_done"),
        NodeKind::Action { action, .. } => action == "create_sub_issues",
        _ => false,
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
    // The one bot-authored issue that starts a workflow: a sub-issue this enzarb created
    // for a stored epic (compiled or graph), checked like the compiled machine does.
    let child_of = if ctx.is_bot(&e.data.actor) {
        match super::issue::epic_child_of(ctx, e) {
            Some(p) => Some(p),
            None => return Transition::reject(state, "bot actor ignored"),
        }
    } else {
        None
    };
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
        epic_depth: child_of.map_or(0, |p| p.depth + 1),
        epic_parent: child_of.map(|p| p.key.to_string()),
        ..Default::default()
    };
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx));
    let phase = enter_start(ctx, &mut fx, def, &mut d);
    let note = if phase == GraphPhase::Blocked {
        format!("graph `{}` blocked by dependencies", def.name)
    } else {
        format!("graph `{}` started", def.name)
    };
    Transition::to(st(phase, d), fx.finish(), note)
}

/// Enters `start`, unless a dependency is not completed: then `Blocked` until it is.
fn enter_start(ctx: &Ctx, fx: &mut Fx, def: &Definition, d: &mut GraphData) -> GraphPhase {
    if blocked(ctx, d) {
        d.blocked_resume = None;
        d.waiting_on = None;
        return GraphPhase::Blocked;
    }
    let (mut pos, next) = GraphState::begin(def);
    // The item facts are there before the first edge is taken.
    let mut probe = d.clone();
    set_item(ctx, &mut probe);
    pos.outputs.extend(probe.graph.outputs);
    d.graph.set_position(pos);
    match next {
        Next::Node(_) => drive(ctx, fx, def, d),
        Next::Failed(r) => failed(ctx, fx, d, &r),
    }
}

/// The one dependency guard (before `start`, and before a segment that can run an agent that
/// pushes): true, with `blocked_by` and `wait` set, while any dependency is not completed.
fn blocked(ctx: &Ctx, d: &mut GraphData) -> bool {
    if ctx.blocking_dependencies.is_empty() {
        d.blocked_by.clear();
        d.wait = None;
        return false;
    }
    d.blocked_by = ctx.blocking_dependencies.clone();
    let list = d
        .blocked_by
        .iter()
        .map(|b| {
            let mut facts = Vec::new();
            if !b.title.is_empty() {
                facts.push(b.title.clone());
            }
            facts.push(if b.state.is_empty() {
                "not tracked by enzarb".to_string()
            } else {
                b.state.clone()
            });
            if !b.pr_url.is_empty() {
                facts.push(format!("PR {} not merged", b.pr_url));
            }
            format!("{} ({})", b.label, facts.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ");
    d.wait = Some(if d.blocked_by.iter().any(|b| b.failed) {
        json!({
            "waiting_on": "human",
            "reason": "dependency_failed",
            "text": format!(
                "Blocked by {list}. A dependency cannot complete by itself: reopen or replace it, \
                 or remove the dependency"
            ),
        })
    } else {
        json!({
            "waiting_on": "dependency",
            "reason": "dependencies_open",
            "text": format!("Blocked by {list}"),
        })
    });
    true
}

/// An `agent` node that pushes code: anything but the read-only prompts.
fn pushes(n: &enzarb_graph::Node) -> bool {
    matches!(&n.kind, NodeKind::Agent { prompt }
        if !["triage", "plan", "plan_epic", "review"].contains(&prompt.trim()))
}

/// Whether a segment starting at `start` can reach an agent that pushes.
fn pushes_ahead(def: &Definition, start: &str) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = std::collections::VecDeque::from([start.to_string()]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(n) = def.node(&id) else { continue };
        if n.kind.runner() != Runner::Sandbox {
            continue;
        }
        if pushes(n) {
            return true;
        }
        queue.extend(n.next.iter().flat_map(|e| e.targets()).map(str::to_string));
        queue.extend(n.on_limit.clone());
    }
    false
}

fn target(ctx: &Ctx) -> enzarb_connector::ProgressTarget {
    ctx.progress_target(None, CommentKind::default())
}

fn failed(ctx: &Ctx, fx: &mut Fx, d: &mut GraphData, error: &str) -> GraphPhase {
    d.error = truncate(error, 500);
    d.waiting_on = None;
    d.accepts_children = false;
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
    set_item(ctx, d);
    d.accepts_children = waits_on_children(node);
    match &node.kind {
        NodeKind::Command { .. } | NodeKind::Agent { .. } => {
            if pushes_ahead(def, &id) && blocked(ctx, d) {
                d.blocked_resume = Some(id);
                d.waiting_on = None;
                d.accepts_children = false;
                return GraphPhase::Blocked;
            }
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
            "create_sub_issues" => match parse_epic_plan(&d.plan, ctx.config.max_epic_children) {
                Ok(subs) => {
                    fx.create_sub_issues(d.epic_depth + 1, &subs);
                    d.sub_issues = subs;
                    d.waiting_on = Some(JobKind::CreateSubIssues);
                    GraphPhase::Running
                }
                Err(e) => {
                    d.accepts_children = false;
                    failed(
                        ctx,
                        fx,
                        d,
                        &format!("node `{id}`: unusable sub-issue breakdown: {e}"),
                    )
                }
            },
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
    if s.phase == GraphPhase::Failed && is_retry(ctx, ev, false) {
        return retry(ctx, ev, s, &state);
    }
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
    if s.phase == GraphPhase::Blocked
        && let Some(t) = blocked_step(ctx, ev, s, &state, &def)
    {
        return t;
    }
    match &ev.payload {
        EventPayload::Ui(u) => match u.action {
            UiActionKind::Cancel if ctx.can_cancel() => {
                cancel(ctx, ev, s, ctx.cancel_comment(&u.actor.login))
            }
            UiActionKind::Cancel => Transition::reject(&state, "cancel requires write access"),
            UiActionKind::RepoRemoved => cancel(ctx, ev, s, None).repo_removed(),
            UiActionKind::Approve => wait_event(ctx, ev, s, &state, &def),
            UiActionKind::Comment => wait_event(ctx, ev, s, &state, &def),
            UiActionKind::Retry => Transition::reject(&state, "nothing to retry"),
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
        EventPayload::DependenciesChanged => dependencies_changed(ctx, ev, s, &def),
        EventPayload::Connector(_) => match epic_request(ctx, ev, s, &state) {
            Some(Ok(t)) => t,
            Some(Err(flagged)) => wait_event(ctx, ev, &flagged, &state, &def),
            None => wait_event(ctx, ev, s, &state, &def),
        },
        _ => Transition::reject(&state, "event not applicable to a graph workflow"),
    }
}

/// An event for a `Blocked` task. Dependency changes and a write-access retry re-evaluate
/// (no override); cancel, close and repository removal go on as usual (`None`); anything
/// else waits.
fn blocked_step(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Option<Transition> {
    let reevaluate =
        matches!(ev.payload, EventPayload::DependenciesChanged) || is_retry(ctx, ev, false);
    if !reevaluate {
        return match &ev.payload {
            EventPayload::Ui(u)
                if matches!(u.action, UiActionKind::Cancel | UiActionKind::RepoRemoved) =>
            {
                None
            }
            _ => Some(Transition::reject(state, "blocked by dependencies")),
        };
    }
    let mut d = s.data.clone();
    d.open_dependencies = ctx.open_dependencies.clone();
    let mut fx = Fx::new(ctx, ev);
    if blocked(ctx, &mut d) {
        return Some(Transition::to(
            st(GraphPhase::Blocked, d),
            Vec::new(),
            "still blocked",
        ));
    }
    let phase = match d.blocked_resume.take() {
        None => enter_start(ctx, &mut fx, def, &mut d),
        Some(node) => {
            d.graph.node = node;
            drive(ctx, &mut fx, def, &mut d)
        }
    };
    let note = format!("dependencies completed; resumed at {}", d.graph.node);
    Some(Transition::to(st(phase, d), fx.finish(), note))
}

/// `/epic` on its own line or the `epic` label, from a writer: the item is an epic from now
/// on (an item fact). A label is answered here; a `/epic` comment also goes on as a comment
/// (`Err` carries the flagged task), so a wait on comments can re-plan it as a breakdown.
fn epic_request(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
) -> Option<Result<Transition, Box<GraphTask>>> {
    let label = ev.labeled() == Some("epic");
    let comment = ev
        .comment_like()
        .is_some_and(|c| enzarb_connector::command::has_epic_command(c.text));
    if !(label || comment) || ev.actor().is_some_and(|a| ctx.is_bot(a)) || !ctx.can_write() {
        return label.then(|| Ok(Transition::reject(state, "epic requires write access")));
    }
    if !epic_allowed(ctx, &s.data) {
        return label.then(|| {
            Ok(Transition::reject(
                state,
                "sub-issues of an epic cannot be epics themselves",
            ))
        });
    }
    let mut flagged = s.clone();
    flagged.data.epic = true;
    set_item(ctx, &mut flagged.data);
    if label {
        Some(Ok(Transition::to(
            WorkflowState::Graph(flagged),
            Vec::new(),
            "epic requested",
        )))
    } else {
        Some(Err(Box::new(flagged)))
    }
}

/// `DependenciesChanged` (the reducer put the dependencies not done yet in
/// `ctx.open_dependencies`). Stored always; at a wait on `children_done` (only the
/// sub-issues count) or `dependencies_done`, the graph resumes once none is still pending,
/// with `{event, open, completed, failed}` (work item keys; `failed`: closed without
/// completing) so the definition can branch.
fn dependencies_changed(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    def: &Definition,
) -> Transition {
    let mut task = s.clone();
    task.data.open_dependencies = ctx.open_dependencies.clone();
    let event = match def.node(&s.data.graph.node).map(|n| &n.kind) {
        Some(NodeKind::Wait { events, .. }) if s.phase == GraphPhase::Waiting => events
            .iter()
            .find(|e| *e == "children_done" || *e == "dependencies_done")
            .cloned(),
        _ => None,
    };
    let unchanged =
        |t: GraphTask| Transition::to(WorkflowState::Graph(t), Vec::new(), "dependencies changed");
    let Some(event) = event else {
        return unchanged(task);
    };
    let children: Vec<String> = s
        .data
        .children
        .iter()
        .map(|c| super::super::reducer::normalize_key(&c.item).to_string())
        .collect();
    let counts = |k: &str| event == "dependencies_done" || children.iter().any(|c| c == k);
    let deps: Vec<&OpenDependency> = ctx
        .open_dependencies
        .iter()
        .filter(|o| counts(&o.key))
        .collect();
    if deps.iter().any(|o| o.problem.is_empty()) {
        return unchanged(task);
    }
    let failed: Vec<&str> = deps.iter().map(|o| o.key.as_str()).collect();
    let completed: Vec<&str> = children
        .iter()
        .map(String::as_str)
        .filter(|c| !failed.contains(c))
        .collect();
    let out = json!({"event": event, "open": [], "completed": completed, "failed": failed});
    advance(ctx, ev, &task, def, out, "dependencies settled")
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
            let mut fx = Fx::new(ctx, ev);
            // The sandbox is agent-writable: trust only what it could legitimately report.
            // It may stop only at a kernel node reachable from the segment's start through
            // sandbox nodes, so it cannot skip a wait, gate or action.
            let start = s.data.graph.node.clone();
            let stops = enzarb_graph::segment_stops(def, &start);
            if !stops.contains(&g.stopped_at) {
                let why = format!(
                    "the segment from `{start}` reported stopping at `{}`, which it cannot reach",
                    g.stopped_at
                );
                let phase = failed(ctx, &mut fx, &mut d, &why);
                return Transition::to(st(phase, d), fx.finish(), "segment result rejected");
            }
            if let Ok(o) = AgentOutput::parse(&fin.summary)
                && o.decision == "plan"
            {
                d.plan = o.body;
            }
            d.summary = fin.summary;
            if fin.pushed {
                d.branch = fin.branch;
            }
            // Outputs: only sandbox nodes' come from the sandbox. Visits: sandbox nodes' never
            // go down; kernel nodes' are the kernel's own, and entering the stop counts here.
            let mut pos = d.graph.position();
            for (id, out) in g.outputs {
                if def
                    .node(&id)
                    .is_some_and(|n| n.kind.runner() == Runner::Sandbox)
                {
                    // A triage `epic` decision makes the item an epic (when allowed).
                    if out.get("decision").and_then(Value::as_str) == Some("epic")
                        && epic_allowed(ctx, &d)
                    {
                        d.epic = true;
                    }
                    pos.outputs.insert(id, out);
                }
            }
            for (id, n) in g.visits {
                if def
                    .node(&id)
                    .is_some_and(|x| x.kind.runner() == Runner::Sandbox)
                {
                    let v = pos.visits.entry(id).or_insert(0);
                    *v = (*v).max(n);
                }
            }
            let phase = match enzarb_graph::enter(def, &mut pos, &g.stopped_at) {
                Next::Node(_) => {
                    d.graph.set_position(pos);
                    drive(ctx, &mut fx, def, &mut d)
                }
                Next::Failed(r) => {
                    d.graph.set_position(pos);
                    failed(ctx, &mut fx, &mut d, &r)
                }
            };
            Transition::to(st(phase, d), fx.finish(), "segment finished")
        }
        JobKind::CreateSubIssues => {
            let children: Vec<CreatedChild> = output
                .get("children")
                .cloned()
                .and_then(|c| serde_json::from_value(c).ok())
                .unwrap_or_default();
            let created: Vec<Value> = children
                .iter()
                .map(|c| json!({"key": c.key, "number": c.item.number}))
                .collect();
            let mut task = s.clone();
            task.data.children = children;
            advance(
                ctx,
                ev,
                &task,
                def,
                json!({ "created": created }),
                "sub-issues created",
            )
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
    let (events, from) = match def.node(&s.data.graph.node).map(|n| &n.kind) {
        Some(NodeKind::Wait { events, from }) if s.phase == GraphPhase::Waiting => (events, *from),
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
        if from == WaitFrom::Writers && !ctx.can_write() {
            return Transition::reject(state, "this wait takes comments from writers only");
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

// ---------------------------------------------------------------------------
// Retry and adoption of compiled `issue` items
// ---------------------------------------------------------------------------

/// A retry request with write access: the UI retry, or an explicit @mention (not by the
/// issue author when `author_answers`, whose mention on a `Stale` issue is an answer).
fn is_retry(ctx: &Ctx, ev: &EngineEvent, author_answers: bool) -> bool {
    if ev.actor().is_some_and(|a| ctx.is_bot(a)) || !ctx.can_write() {
        return false;
    }
    match &ev.payload {
        EventPayload::Ui(u) => u.action == UiActionKind::Retry,
        EventPayload::Connector(_) => ev
            .comment_like()
            .is_some_and(|c| c.mention && !author_answers),
        _ => false,
    }
}

/// Starts `d` (task fields kept, position reset) at the definition's `start`.
fn begin_fresh(
    ctx: &Ctx,
    ev: &EngineEvent,
    src: &GraphSource,
    def: &Definition,
    mut d: GraphData,
    note: &str,
) -> Transition {
    d.graph = GraphPin {
        name: def.name.clone(),
        path: src.path.clone(),
        sha: src.sha.clone(),
        source: src.source.clone(),
        ..Default::default()
    };
    d.error.clear();
    d.waiting_on = None;
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx));
    let phase = enter_start(ctx, &mut fx, def, &mut d);
    let note = if phase == GraphPhase::Blocked {
        format!("{note}; moved to graph workflow, blocked by dependencies")
    } else {
        format!("{note}; moved to graph workflow at {}", d.graph.node)
    };
    Transition::to(st(phase, d), fx.finish(), note)
}

/// `Failed -> start`: the definition is re-read from the default branch ([`Ctx::graph`]);
/// without a usable one the pinned definition runs again.
fn retry(ctx: &Ctx, ev: &EngineEvent, s: &GraphTask, state: &WorkflowState) -> Transition {
    let pinned = GraphSource {
        name: s.data.graph.name.clone(),
        path: s.data.graph.path.clone(),
        sha: s.data.graph.sha.clone(),
        source: s.data.graph.source.clone(),
    };
    let fresh = ctx
        .graph
        .as_ref()
        .and_then(|src| load(src).ok().map(|d| (src.clone(), d)));
    let Some((src, def)) = fresh.or_else(|| load(&pinned).ok().map(|d| (pinned, d))) else {
        return Transition::reject(state, "the pinned definition no longer loads");
    };
    let mut t = begin_fresh(ctx, ev, &src, &def, s.data.clone(), "retry");
    t.note = t.note.replace("moved to graph workflow at", "restarted at");
    t
}

/// The first `wait` node listening for `approval`, breadth-first from `start`.
fn approval_wait(def: &Definition) -> Option<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut queue = std::collections::VecDeque::from([def.start.clone()]);
    while let Some(id) = queue.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(n) = def.node(&id) else { continue };
        if let NodeKind::Wait { events, .. } = &n.kind
            && events.iter().any(|e| e == "approval")
        {
            return Some(id);
        }
        queue.extend(n.next.iter().flat_map(|e| e.targets()).map(str::to_string));
        queue.extend(n.on_limit.clone());
    }
    None
}

/// A compiled `issue` item moves to the repository's issue graph when a human acts on it:
/// an approval of its plan (`AwaitingApproval`) resumes the graph at its first approval
/// wait, a retry (`Failed`, `Stale`) starts the graph fresh. `legacy` runs first and decides
/// whether the event is accepted at all; everything else stays with the compiled machine.
pub(super) fn adopt_or(
    ctx: &Ctx,
    s: &super::IssueState,
    ev: &EngineEvent,
    legacy: fn(&Ctx, &super::IssueState, &EngineEvent) -> Transition,
) -> Transition {
    let t = legacy(ctx, s, ev);
    if !t.accepted {
        return t;
    }
    adopt(ctx, s, ev).unwrap_or(t)
}

fn adopt(ctx: &Ctx, s: &super::IssueState, ev: &EngineEvent) -> Option<Transition> {
    use super::IssuePhase;
    let src = ctx.graph.as_ref()?;
    if s.data.epic || ctx.live_agent_jobs {
        return None;
    }
    let def = load(src)
        .ok()
        .filter(|d| d.on.iter().any(|t| t == ISSUE_TRIGGER))?;
    let d = GraphData {
        number: s.data.number,
        url: s.data.url.clone(),
        title: s.data.title.clone(),
        body: s.data.body.clone(),
        author: s.data.author.clone(),
        plan: s.data.plan.clone(),
        ..Default::default()
    };
    match s.phase {
        IssuePhase::AwaitingApproval
            if ev.is_approval()
                && ctx.can_write()
                && !ev.actor().is_some_and(|a| ctx.is_bot(a)) =>
        {
            let wait = approval_wait(&def)?;
            let mut d = d;
            d.graph = GraphPin {
                name: def.name.clone(),
                path: src.path.clone(),
                sha: src.sha.clone(),
                source: src.source.clone(),
                node: wait.clone(),
                visits: [(wait, 1)].into(),
                ..Default::default()
            };
            let task = GraphTask {
                phase: GraphPhase::Waiting,
                data: d,
            };
            let login = ev.actor().map(|a| a.login.clone()).unwrap_or_default();
            let out = json!({"event": "approval", "author": login});
            let mut t = advance(ctx, ev, &task, &def, out, "plan approved");
            if let WorkflowState::Graph(g) = &t.next {
                t.note = format!(
                    "plan approved; moved to graph workflow at {}",
                    g.data.graph.node
                );
            }
            let mut fx = Fx::new(ctx, ev);
            fx.received(&target(ctx));
            let mut effects = fx.finish();
            effects.append(&mut t.effects);
            t.effects = effects;
            Some(t)
        }
        IssuePhase::Failed | IssuePhase::Stale => {
            let author = ev
                .actor()
                .is_some_and(|a| a.login.eq_ignore_ascii_case(&s.data.author));
            let answers = s.phase == IssuePhase::Stale && author;
            is_retry(ctx, ev, answers).then(|| begin_fresh(ctx, ev, src, &def, d, "retry"))
        }
        _ => None,
    }
}
