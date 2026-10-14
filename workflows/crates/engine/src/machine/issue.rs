//! The `issue` workflow (workflows.md state diagram).
//!
//! Differences from the diagram, all additive:
//! * `Implementing` covers both the agent run and opening the change request; the
//!   diagram's transient `ChangeRequestOpened` state is folded into `Done`, which
//!   carries the handoff (`StartWorkflow` for the change_request workflow).
//! * `Cancelled` is the target of "any state: WorkItemClosed" (and of the UI cancel).
//! * `Failed` remembers the phase it failed in, so `RetryRequested` can resume it.
//! * `AwaitingAnswers` nudges once before it goes `Stale`; a reply from the author
//!   or a retry resumes triage from `Stale`.
//! * Epics (docs/adr/0018-epics.md): triage may answer `epic`, or a writer asks for one with
//!   `/epic` or the `epic` label. Planning then writes a breakdown into sub-issues, and the
//!   usual approval leads to `Decomposing` (the `create_sub_issues` job) and `AwaitingChildren`
//!   instead of `Implementing`. The epic never opens a change request; it is `Done` when every
//!   sub-issue is.

use enzarb_connector::command::has_epic_command;
use enzarb_connector::epic::parse_epic_marker;
use enzarb_connector::{
    ChangeRequestSpec, CommentKind, Event, EventData, EventType, ItemKind, ItemRef, ProgressTarget,
    WorkItemKey,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::change_request::OpenDependency;
use super::fx::{AgentSpec, Fx};
use super::prompt::{AgentOutput, Decision, PromptContext, SubIssue, parse_epic_plan};
use super::stack::{StackParent, stack_marker};
use super::title::pr_title;
use super::{
    Ctx, Effect, EngineEvent, EventPayload, JobKind, Transition, UiActionKind, WorkflowKind,
    WorkflowState,
};

/// The label that asks for an epic. Compared case-insensitively.
const EPIC_LABEL: &str = "epic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssuePhase {
    Triaging,
    AwaitingAnswers,
    Stale,
    Duplicate,
    Planning,
    AwaitingApproval,
    Implementing,
    /// Epic: the `create_sub_issues` job is running.
    Decomposing,
    /// Epic: the sub-issues exist; waiting for them to be done.
    AwaitingChildren,
    Done,
    Failed,
    Cancelled,
}

/// A sub-issue that `create_sub_issues` made (or found by its marker).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedChild {
    /// The key in the approved plan.
    pub key: String,
    pub item: WorkItemKey,
    #[serde(default)]
    pub title: String,
}

/// `state_data` of an issue work item (all fields default for forward compatibility).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IssueData {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub author: String,
    /// Comment that started the current step, for progress markers.
    pub trigger_comment_id: Option<u64>,
    /// Kind of that comment (PR review comments need a different API).
    pub trigger_comment_kind: CommentKind,
    /// Text of that comment (answers, plan feedback).
    pub trigger_text: String,
    pub triage_summary: String,
    pub plan: String,
    /// The author was nudged while awaiting answers.
    pub nudged: bool,
    /// The agent run finished and the change request is being opened.
    pub implemented: bool,
    /// `agent_implement` body (the pull request description).
    pub pr_body: String,
    pub failed_phase: Option<IssuePhase>,
    pub error: String,
    /// Number of the change request once opened.
    pub change_request: Option<u64>,
    /// The change request the implementation branch was cut from, chosen when the
    /// implementation started (stacked change requests). `None` when it starts from the
    /// default branch.
    pub stack: Option<StackParent>,
    /// Plan this issue as an epic (triage said so, or a writer asked).
    pub epic: bool,
    /// Nesting depth: `0` for a top-level issue, the parent's plus one for an epic's sub-issue.
    /// Taken from the stored parent, never from the issue body.
    pub epic_depth: u32,
    /// Work item key of the epic this issue is a sub-issue of.
    pub epic_parent: Option<String>,
    /// The approved breakdown, in creation order (epics only).
    pub sub_issues: Vec<SubIssue>,
    /// The sub-issues created for this epic.
    pub children: Vec<CreatedChild>,
    /// Sub-issues not done yet, as last read. Refreshed on `DependenciesChanged`; the ticker
    /// compares it with the database to decide when to send that event.
    pub open_dependencies: Vec<OpenDependency>,
    /// The maintainers were told that no sub-issue can complete.
    pub children_notified: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueState {
    pub phase: IssuePhase,
    pub data: IssueData,
}

fn branch(n: u64) -> String {
    format!("enzarb/issue-{n}")
}

fn scratch_branch(step: &str, n: u64) -> String {
    format!("enzarb/{step}-{n}")
}

fn target(ctx: &Ctx, d: &IssueData) -> ProgressTarget {
    ctx.progress_target(d.trigger_comment_id, d.trigger_comment_kind)
}

fn prompt_ctx(ctx: &Ctx, d: &IssueData) -> PromptContext {
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
        trigger: d.trigger_text.clone(),
        triage_summary: d.triage_summary.clone(),
        plan: d.plan.clone(),
        base_ref: ctx.default_branch.clone(),
        epic: d.epic,
        epic_allowed: if epic_allowed(ctx, d) { "yes" } else { "no" }.into(),
        max_subissues: ctx.config.max_epic_children.to_string(),
        ..Default::default()
    }
}

/// Epics do not nest: an issue at the depth limit is never one.
fn epic_allowed(ctx: &Ctx, d: &IssueData) -> bool {
    d.epic_depth < ctx.config.max_epic_depth
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...")
    }
}

fn st(phase: IssuePhase, data: IssueData) -> WorkflowState {
    WorkflowState::Issue(IssueState { phase, data })
}

// -- step starters (queue the agent job for a phase) -------------------------

fn start_triage(ctx: &Ctx, fx: &mut Fx, d: &IssueData) {
    fx.agent(
        JobKind::AgentTriage,
        &target(ctx, d),
        AgentSpec {
            base_ref: ctx.default_branch.clone(),
            branch: scratch_branch("triage", d.number),
            push: false,
        },
        prompt_ctx(ctx, d),
    );
}

fn start_plan(ctx: &Ctx, fx: &mut Fx, d: &IssueData) {
    fx.agent(
        JobKind::AgentPlan,
        &target(ctx, d),
        AgentSpec {
            base_ref: ctx.default_branch.clone(),
            branch: scratch_branch("plan", d.number),
            push: false,
        },
        prompt_ctx(ctx, d),
    );
}

fn start_implement(ctx: &Ctx, fx: &mut Fx, d: &IssueData) {
    let mut pc = prompt_ctx(ctx, d);
    pc.branch = branch(d.number);
    let base_ref = match &d.stack {
        Some(p) => {
            pc.base_ref = p.head_ref.clone();
            p.head_ref.clone()
        }
        None => ctx.default_branch.clone(),
    };
    fx.agent(
        JobKind::AgentImplement,
        &target(ctx, d),
        AgentSpec {
            base_ref,
            branch: branch(d.number),
            push: true,
        },
        pc,
    );
}

/// Enter `phase` and queue its job, after a trigger that starts work.
fn begin(
    ctx: &Ctx,
    ev: &EngineEvent,
    phase: IssuePhase,
    mut d: IssueData,
    note: &str,
) -> Transition {
    if phase == IssuePhase::Implementing {
        // Stack on the parent's change request when there is exactly one open one.
        d.stack = ctx.stack_parent.clone().filter(|p| !p.merged);
    }
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx, &d));
    fx.wake_at(None);
    match phase {
        IssuePhase::Triaging => start_triage(ctx, &mut fx, &d),
        IssuePhase::Planning => start_plan(ctx, &mut fx, &d),
        IssuePhase::Implementing => start_implement(ctx, &mut fx, &d),
        _ => unreachable!("begin is only used for agent phases"),
    }
    Transition::to(st(phase, d), fx.finish(), note)
}

/// Move to `Failed`, tell people, and optionally cancel jobs still in flight.
fn fail(ctx: &Ctx, ev: &EngineEvent, s: &IssueState, error: &str, cancel_jobs: bool) -> Transition {
    let mut d = s.data.clone();
    d.failed_phase = Some(s.phase);
    d.error = truncate(error, 500);
    let mut fx = Fx::new(ctx, ev);
    if cancel_jobs {
        fx.cancel_jobs();
    }
    fx.wake_at(None);
    fx.comment(format!(
        "enzarb stopped while working on this: {}\n\nA maintainer can retry it from the enzarb UI.",
        d.error
    ));
    fx.failed(&target(ctx, &d));
    Transition::to(st(IssuePhase::Failed, d), fx.finish(), "failed")
}

fn cancel(ctx: &Ctx, ev: &EngineEvent, s: &IssueState, comment: Option<String>) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    fx.cancel_jobs();
    fx.wake_at(None);
    if let Some(c) = comment {
        fx.comment(c);
    }
    Transition::to(
        st(IssuePhase::Cancelled, s.data.clone()),
        fx.finish(),
        "cancelled",
    )
}

// -- first event --------------------------------------------------------------

pub(super) fn start(ctx: &Ctx, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    let Some(e) = ev.connector() else {
        return Transition::reject(state, "not an opening event");
    };
    if e.event_type != EventType::WorkItemOpened || e.data.item.kind != ItemKind::Issue {
        return Transition::reject(state, "not an opening event");
    }
    let item = &e.data.item;
    let mut d = IssueData {
        number: item.number,
        url: item.url.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        author: item.author.clone(),
        ..Default::default()
    };
    if ctx.is_bot(&e.data.actor) {
        // The one bot-authored issue that starts a workflow: a sub-issue that this enzarb
        // created for an epic. It still goes through triage, plan and approval.
        match epic_child_of(ctx, e) {
            Some(parent) => {
                d.epic_depth = parent.depth + 1;
                d.epic_parent = Some(parent.key.to_string());
            }
            None => return Transition::reject(state, "bot actor ignored"),
        }
    }
    begin(ctx, ev, IssuePhase::Triaging, d, "issue opened")
}

/// The stored epic that the bot-authored issue in `e` is a sub-issue of, if all of this holds:
/// it was opened by this enzarb itself (not just any bot), its body names a parent in the same
/// repository that the reducer found in the store (in `Decomposing` or `AwaitingChildren`),
/// and the marker's key is one of the sub-issues in that parent's approved plan.
fn epic_child_of<'a>(ctx: &'a Ctx, e: &Event) -> Option<&'a super::EpicParent> {
    if !e.data.actor.login.eq_ignore_ascii_case(&ctx.bot_login) {
        return None;
    }
    let (parent, key) = parse_epic_marker(&e.data.item.body)?;
    let found = ctx.epic_parent.as_ref()?;
    // GitHub names are case-insensitive; the stored keys are lowercase.
    let same = |a: &dyn std::fmt::Display, b: &dyn std::fmt::Display| {
        a.to_string().eq_ignore_ascii_case(&b.to_string())
    };
    (same(&found.key, &parent)
        && same(&parent.repo, &ctx.work_item.repo)
        && found.sub_keys.contains(&key))
    .then_some(found)
}

// -- later events -------------------------------------------------------------

pub(super) fn step(ctx: &Ctx, s: &IssueState, ev: &EngineEvent) -> Transition {
    use IssuePhase::*;
    let state = WorkflowState::Issue(s.clone());

    // Closing (or merging) always cancels, whoever did it.
    if ev.closed().is_some() {
        return match s.phase {
            Done | Duplicate | Cancelled => Transition::reject(&state, "already finished"),
            _ => cancel(ctx, ev, s, None),
        };
    }

    match &ev.payload {
        EventPayload::Ui(u) => match u.action {
            UiActionKind::Cancel => {
                if !ctx.can_cancel() {
                    return Transition::reject(&state, "cancel requires write access");
                }
                match s.phase {
                    Done | Duplicate | Cancelled => Transition::reject(&state, "already finished"),
                    _ => cancel(ctx, ev, s, ctx.cancel_comment(&u.actor.login)),
                }
            }
            UiActionKind::RepoRemoved => match s.phase {
                Done | Duplicate | Cancelled => Transition::reject(&state, "already finished"),
                _ => cancel(ctx, ev, s, None).repo_removed(),
            },
            UiActionKind::Retry => retry(ctx, ev, s, &state),
            UiActionKind::Approve => approval(ctx, ev, s, &state),
            UiActionKind::Merge => Transition::reject(&state, "nothing to merge"),
            UiActionKind::Comment => ui_comment(ctx, ev, s, &state, &u.actor.login),
        },
        EventPayload::JobSucceeded { kind, output, .. } => {
            job_succeeded(ctx, ev, s, &state, *kind, output)
        }
        EventPayload::JobFailed { kind, error, .. } => job_failed(ctx, ev, s, &state, *kind, error),
        EventPayload::TimerFired { job_id } => timer(ctx, ev, s, &state, *job_id),
        EventPayload::Unmergeable { .. } | EventPayload::ReadinessPoll => {
            Transition::reject(&state, "not a change request")
        }
        EventPayload::DependenciesChanged => children_changed(ctx, ev, s, &state),
        EventPayload::Connector(_) if ev.is_approval() => approval(ctx, ev, s, &state),
        EventPayload::Connector(_) if ev.labeled().is_some() => labeled(ctx, ev, s, &state),
        EventPayload::Connector(_) => match ev.comment_like() {
            Some(c) => comment(ctx, ev, s, &state, &c),
            // WorkItemOpened again, change request events, reviews: not for this workflow.
            None => Transition::reject(&state, "event not applicable to an issue"),
        },
    }
}

fn approval(ctx: &Ctx, ev: &EngineEvent, s: &IssueState, state: &WorkflowState) -> Transition {
    if ev.actor().is_some_and(|a| ctx.is_bot(a)) {
        return Transition::reject(state, "bot actor ignored");
    }
    if !ctx.can_write() {
        return Transition::reject(state, "approval requires write access");
    }
    let mut d = s.data.clone();
    let tc = ev.connector().and_then(|e| e.data.comment.as_ref());
    d.trigger_comment_id = tc.map(|c| c.id);
    d.trigger_comment_kind = tc.map(|c| c.kind).unwrap_or_default();
    d.trigger_text.clear();
    match s.phase {
        IssuePhase::AwaitingApproval if d.epic => decompose(ctx, ev, d),
        IssuePhase::AwaitingApproval => {
            d.implemented = false;
            begin(ctx, ev, IssuePhase::Implementing, d, "plan approved")
        }
        // The sub-issues cannot all complete (one was closed): a maintainer decides that
        // the epic is finished anyway.
        IssuePhase::AwaitingChildren if stuck(&d) => {
            let note = "closed by a maintainer with sub-issues left unfinished";
            finish_epic(ctx, ev, d, format!("This epic is closed: {note}."), note)
        }
        IssuePhase::AwaitingChildren => {
            Transition::reject(state, "sub-issues are still in progress")
        }
        _ => Transition::reject(state, "no plan is awaiting approval"),
    }
}

use enzarb_connector::command::has_approve_command;

// -- epics ----------------------------------------------------------------------

/// `AwaitingApproval -> Decomposing`: the breakdown was approved, create the sub-issues.
fn decompose(ctx: &Ctx, ev: &EngineEvent, d: IssueData) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx, &d));
    fx.wake_at(None);
    fx.create_sub_issues(d.epic_depth + 1, &d.sub_issues);
    Transition::to(st(IssuePhase::Decomposing, d), fx.finish(), "epic approved")
}

/// Every sub-issue still open is one that will never complete.
fn stuck(d: &IssueData) -> bool {
    !d.open_dependencies.is_empty() && d.open_dependencies.iter().all(|o| !o.problem.is_empty())
}

/// `-> Done` for an epic: tell people and stop.
fn finish_epic(
    ctx: &Ctx,
    ev: &EngineEvent,
    d: IssueData,
    comment: String,
    note: &str,
) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    fx.comment(comment);
    fx.done(&target(ctx, &d));
    fx.wake_at(None);
    Transition::to(st(IssuePhase::Done, d), fx.finish(), note)
}

/// The `create_sub_issues` job finished: remember the children and wait for them.
fn decomposed(ctx: &Ctx, ev: &EngineEvent, s: &IssueState, output: &Value) -> Transition {
    #[derive(Deserialize)]
    struct Out {
        children: Vec<CreatedChild>,
    }
    let children = match serde_json::from_value::<Out>(output.clone()) {
        Ok(o) if !o.children.is_empty() => o.children,
        Ok(_) => return fail(ctx, ev, s, "no sub-issues were created", false),
        Err(e) => {
            return fail(
                ctx,
                ev,
                s,
                &format!("sub-issue output unreadable: {e}"),
                false,
            );
        }
    };
    let mut d = s.data.clone();
    let mut list = String::new();
    for c in &children {
        list.push_str(&format!("\n- #{} {}", c.item.number, c.title));
    }
    d.children = children;
    d.open_dependencies = ctx.open_dependencies.clone();
    d.children_notified = false;
    let mut fx = Fx::new(ctx, ev);
    fx.comment(format!(
        "enzarb created {} sub-issues for this epic. Each one is triaged and planned on its own and needs its own `/approve`:\n{list}\n\nThis epic is done when all of them are.",
        d.children.len()
    ));
    fx.done(&target(ctx, &d));
    fx.wake_at(None);
    Transition::to(
        st(IssuePhase::AwaitingChildren, d),
        fx.finish(),
        "sub-issues created",
    )
}

/// `DependenciesChanged` for an epic waiting on its sub-issues (the reducer put the ones not
/// done yet into `ctx.open_dependencies`).
fn children_changed(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
) -> Transition {
    if s.phase != IssuePhase::AwaitingChildren {
        return Transition::reject(state, "not waiting for sub-issues");
    }
    let mut d = s.data.clone();
    d.open_dependencies = ctx.open_dependencies.clone();
    if d.open_dependencies.is_empty() {
        let n = d.children.len();
        return finish_epic(
            ctx,
            ev,
            d,
            format!("All {n} sub-issues are done. Closing this epic."),
            "all sub-issues done",
        );
    }
    if !stuck(&d) {
        d.children_notified = false;
        return Transition::to(
            st(IssuePhase::AwaitingChildren, d),
            Vec::new(),
            "children changed",
        );
    }
    if d.children_notified {
        return Transition::to(
            st(IssuePhase::AwaitingChildren, d),
            Vec::new(),
            "children changed",
        );
    }
    d.children_notified = true;
    let why: Vec<String> = d
        .open_dependencies
        .iter()
        .map(|o| format!("- {}", o.problem))
        .collect();
    let mut fx = Fx::new(ctx, ev);
    fx.comment(format!(
        "The remaining sub-issues of this epic cannot complete:\n{}\n\nA maintainer can reopen or replace them, or comment `/approve` to close this epic as it is.",
        why.join("\n")
    ));
    Transition::to(
        st(IssuePhase::AwaitingChildren, d),
        fx.finish(),
        "sub-issues cannot complete",
    )
}

/// A writer asked for an epic (`/epic`, or the `epic` label). Before the plan is approved this
/// sets the epic flag; once a plan exists it is written again as a breakdown.
fn request_epic(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
    d: IssueData,
) -> Transition {
    use IssuePhase::*;
    if !ctx.can_write() {
        return Transition::reject(state, "epic requires write access");
    }
    if !epic_allowed(ctx, &d) {
        return Transition::reject(state, "sub-issues of an epic cannot be epics themselves");
    }
    if d.epic {
        return Transition::reject(state, "already planned as an epic");
    }
    let mut d = d;
    d.epic = true;
    match s.phase {
        // Remembered: the plan is written as a breakdown once triage is done.
        Triaging | AwaitingAnswers | Stale => {
            let mut fx = Fx::new(ctx, ev);
            fx.received(&target(ctx, &d));
            // Only the flag changes; the comment is not an answer to triage.
            let mut kept = s.data.clone();
            kept.epic = true;
            Transition::to(st(s.phase, kept), fx.finish(), "epic requested")
        }
        AwaitingApproval => {
            d.sub_issues.clear();
            begin(ctx, ev, Planning, d, "epic requested; re-planning")
        }
        Planning | Implementing | Decomposing => {
            Transition::reject(state, "busy: a job is running")
        }
        AwaitingChildren | Duplicate | Done | Failed | Cancelled => {
            Transition::reject(state, "no longer active")
        }
    }
}

/// `WorkItemLabeled`: only the `epic` label means anything.
fn labeled(ctx: &Ctx, ev: &EngineEvent, s: &IssueState, state: &WorkflowState) -> Transition {
    if !ev
        .labeled()
        .is_some_and(|l| l.eq_ignore_ascii_case(EPIC_LABEL))
    {
        return Transition::reject(state, "label ignored");
    }
    if ev.actor().is_some_and(|a| ctx.is_bot(a)) {
        return Transition::reject(state, "bot actor ignored");
    }
    let mut d = s.data.clone();
    d.trigger_comment_id = None;
    d.trigger_comment_kind = CommentKind::default();
    d.trigger_text =
        "A maintainer labeled this issue `epic`: break it down into sub-issues.".into();
    request_epic(ctx, ev, s, state, d)
}

/// Feedback typed in the UI: handled exactly like the same comment written on GitHub (plan
/// feedback while awaiting approval, an answer while awaiting answers), and also posted to
/// the issue so the thread shows it. The bot's own comment is ignored when it comes back.
fn ui_comment(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
    login: &str,
) -> Transition {
    let Some(c) = ev.comment_like().filter(|c| !c.text.is_empty()) else {
        return Transition::reject(state, "the comment is empty");
    };
    let mut t = comment(ctx, ev, s, state, &c);
    if t.accepted {
        let mut fx = Fx::new(ctx, ev);
        fx.comment(format!("{}\n\n_Sent by @{login} from enzarb._", c.text));
        t.effects.splice(0..0, fx.finish());
    }
    t
}

fn comment(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
    c: &super::CommentLike,
) -> Transition {
    use IssuePhase::*;
    if ctx.is_bot(c.actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    // An explicit @mention by someone with write access resumes a Failed or Stale item,
    // exactly like the UI retry (same permission check).
    let is_author = c.actor.login.eq_ignore_ascii_case(&s.data.author);
    // (The author answering a Stale item keeps the answer-carrying path below.)
    if c.mention && ctx.can_write() && (s.phase == Failed || (s.phase == Stale && !is_author)) {
        return retry(ctx, ev, s, state);
    }
    let mut d = s.data.clone();
    d.trigger_comment_id = c.comment.map(|x| x.id);
    d.trigger_comment_kind = c.comment.map(|x| x.kind).unwrap_or_default();
    d.trigger_text = c.text.to_string();
    // `/epic` from someone with write access. Anyone else's comment is just a comment.
    if has_epic_command(c.text) && ctx.can_write() {
        return request_epic(ctx, ev, s, state, d);
    }
    match s.phase {
        // Only the author answers questions.
        AwaitingAnswers | Stale => {
            if !is_author {
                return Transition::reject(state, "only the author can answer the questions");
            }
            d.nudged = false;
            begin(ctx, ev, Triaging, d, "author answered")
        }
        AwaitingApproval => {
            // The connector pairs an `/approve` comment with an `ApprovalGranted`
            // event; the latter owns it, so it is not plan feedback.
            if has_approve_command(c.text) {
                return Transition::reject(state, "approve command is handled as an approval");
            }
            if !is_author && !ctx.can_write() {
                return Transition::reject(state, "feedback needs the author or write access");
            }
            begin(ctx, ev, Planning, d, "plan feedback")
        }
        Triaging | Planning | Implementing | Decomposing => {
            Transition::reject(state, "busy: a job is running")
        }
        AwaitingChildren => Transition::reject(state, "waiting for the sub-issues"),
        Duplicate | Done | Failed | Cancelled => Transition::reject(state, "no longer active"),
    }
}

fn retry(ctx: &Ctx, ev: &EngineEvent, s: &IssueState, state: &WorkflowState) -> Transition {
    if !ctx.can_write() {
        return Transition::reject(state, "retry requires write access");
    }
    let mut d = s.data.clone();
    d.trigger_comment_id = None;
    d.trigger_comment_kind = CommentKind::default();
    d.error.clear();
    match s.phase {
        IssuePhase::Failed => {
            let phase = d.failed_phase.take().unwrap_or(IssuePhase::Triaging);
            match phase {
                IssuePhase::Planning | IssuePhase::Implementing => {
                    d.implemented = false;
                    begin(ctx, ev, phase, d, "retry")
                }
                // Resumes where it stopped: sub-issues already made are found by their markers.
                IssuePhase::Decomposing => decompose(ctx, ev, d),
                _ => begin(ctx, ev, IssuePhase::Triaging, d, "retry"),
            }
        }
        IssuePhase::Stale => {
            d.nudged = false;
            begin(ctx, ev, IssuePhase::Triaging, d, "retry")
        }
        _ => Transition::reject(state, "nothing to retry"),
    }
}

fn timer(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
    job_id: Option<i64>,
) -> Transition {
    use IssuePhase::*;
    match (job_id, s.phase) {
        (None, AwaitingAnswers) if !s.data.nudged => {
            let mut d = s.data.clone();
            d.nudged = true;
            let mut fx = Fx::new(ctx, ev);
            fx.comment(format!(
                "@{} still waiting on answers to the questions above. Without a reply this issue will be marked stale.",
                d.author
            ));
            fx.wake_at(Some(
                ctx.now + chrono::Duration::seconds(ctx.config.stale_after_secs),
            ));
            Transition::to(st(AwaitingAnswers, d), fx.finish(), "nudged")
        }
        (None, AwaitingAnswers) => {
            let mut fx = Fx::new(ctx, ev);
            fx.wake_at(None);
            fx.comment("Marking this stale: there was no answer. Reply here to resume triage.");
            Transition::to(st(Stale, s.data.clone()), fx.finish(), "stale")
        }
        // A running job passed its deadline.
        (Some(_), Triaging | Planning | Implementing | Decomposing) => {
            fail(ctx, ev, s, "the agent run exceeded its deadline", true)
        }
        _ => Transition::reject(state, "no timer expected"),
    }
}

/// The job kinds whose result the phase is waiting for.
fn awaited(phase: IssuePhase) -> &'static [JobKind] {
    match phase {
        IssuePhase::Triaging => &[JobKind::AgentTriage],
        IssuePhase::Planning => &[JobKind::AgentPlan],
        IssuePhase::Implementing => &[JobKind::AgentImplement, JobKind::OpenChangeRequest],
        IssuePhase::Decomposing => &[JobKind::CreateSubIssues],
        _ => &[],
    }
}

fn job_failed(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
    kind: JobKind,
    error: &str,
) -> Transition {
    if matches!(kind, JobKind::SetProgress) {
        return Transition::ack(state, "progress marker failed; ignored");
    }
    if awaited(s.phase).contains(&kind) {
        return fail(
            ctx,
            ev,
            s,
            &format!("{} failed: {error}", kind.as_str()),
            false,
        );
    }
    if kind == JobKind::PostComment {
        return Transition::ack(state, "comment failed; ignored");
    }
    Transition::reject(state, "stale job result")
}

fn job_succeeded(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &IssueState,
    state: &WorkflowState,
    kind: JobKind,
    output: &Value,
) -> Transition {
    use IssuePhase::*;
    if matches!(kind, JobKind::SetProgress | JobKind::PostComment) {
        return Transition::ack(state, "ack");
    }
    if !awaited(s.phase).contains(&kind) {
        return Transition::reject(state, "stale job result");
    }
    let d = s.data.clone();

    if kind == JobKind::CreateSubIssues {
        return decomposed(ctx, ev, s, output);
    }
    if kind == JobKind::OpenChangeRequest {
        if !d.implemented {
            return Transition::reject(state, "stale job result");
        }
        return match serde_json::from_value::<ItemRef>(output.clone()) {
            Ok(pr) => handoff(ctx, ev, d, pr),
            Err(e) => fail(
                ctx,
                ev,
                s,
                &format!("change request output unreadable: {e}"),
                false,
            ),
        };
    }
    if s.phase == Implementing && d.implemented {
        return Transition::reject(state, "stale job result");
    }

    let (decision, out) = match AgentOutput::from_job_output(kind, output) {
        Ok(x) => x,
        Err(e) => return fail(ctx, ev, s, &format!("unusable agent output: {e}"), false),
    };
    let mut d = d;
    let mut fx = Fx::new(ctx, ev);
    let tgt = target(ctx, &d);
    match (s.phase, decision) {
        (Triaging, Decision::Questions) => {
            fx.comment(out.body);
            fx.done(&tgt);
            fx.wake_at(Some(
                ctx.now + chrono::Duration::seconds(ctx.config.nudge_after_secs),
            ));
            d.nudged = false;
            Transition::to(st(AwaitingAnswers, d), fx.finish(), "questions posted")
        }
        (Triaging, Decision::Duplicate) => {
            fx.comment(out.body);
            fx.done(&tgt);
            fx.wake_at(None);
            Transition::to(st(Duplicate, d), fx.finish(), "duplicate")
        }
        (Triaging, decision @ (Decision::Ready | Decision::Epic)) => {
            d.triage_summary = out.body;
            d.trigger_text.clear();
            let mut note = "triage done";
            if decision == Decision::Epic && epic_allowed(ctx, &d) {
                d.epic = true;
                note = "triage done: epic";
            } else if decision == Decision::Epic {
                // Epics do not nest: plan it as one change request.
                d.triage_summary.push_str(
                    "\n\n(Triage found this large, but it is itself a sub-issue of an epic and \
                     cannot be split further: plan it as a single change request.)",
                );
            }
            start_plan(ctx, &mut fx, &d);
            Transition::to(st(Planning, d), fx.finish(), note)
        }
        (Planning, Decision::Plan) => {
            if d.epic {
                match parse_epic_plan(&out.body, ctx.config.max_epic_children) {
                    Ok(subs) => d.sub_issues = subs,
                    Err(e) => {
                        return fail(ctx, ev, s, &format!("unusable epic breakdown: {e}"), false);
                    }
                }
            }
            fx.comment(out.body.clone());
            fx.done(&tgt);
            d.plan = out.body;
            Transition::to(st(AwaitingApproval, d), fx.finish(), "plan posted")
        }
        (Implementing, Decision::Done) => {
            // The sandbox pushes after the run; refuse to open a PR for nothing.
            if output.get("pushed").and_then(Value::as_bool) != Some(true) {
                return fail(ctx, ev, s, "the agent's commits were not pushed", false);
            }
            d.implemented = true;
            d.pr_body = out.body.clone();
            let head_ref = output
                .get("branch")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| branch(d.number));
            let mut body = format!("{}\n\nCloses #{}", out.body.trim_end(), d.number);
            let base_ref = match &d.stack {
                Some(p) => {
                    body.push_str(&format!(
                        "\n\nStacked on {}: this change request is based on `{}` and is \
                         retargeted to `{}` when that one is merged.\n{}",
                        p.key,
                        p.head_ref,
                        ctx.default_branch,
                        stack_marker(p)
                    ));
                    p.head_ref.clone()
                }
                None => ctx.default_branch.clone(),
            };
            fx.open_change_request(&ChangeRequestSpec {
                repo: ctx.work_item.repo.clone(),
                title: pr_title(
                    out.title.as_deref(),
                    &d.title,
                    &ctx.work_item.repo.full_name(),
                ),
                body,
                head_ref,
                base_ref,
                draft: false,
            });
            Transition::to(
                st(Implementing, d),
                fx.finish(),
                "implemented; opening change request",
            )
        }
        (Implementing, Decision::Blocked) => fail(
            ctx,
            ev,
            s,
            &format!("implementation blocked: {}", out.body),
            false,
        ),
        _ => unreachable!("decision validated against the job kind"),
    }
}

/// `Implementing -> Done`: announce the PR and hand it to the change_request workflow.
fn handoff(ctx: &Ctx, ev: &EngineEvent, mut d: IssueData, pr: ItemRef) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    let tgt = target(ctx, &d);
    d.change_request = Some(pr.number);
    fx.comment(format!("enzarb opened {} for this issue.", pr.url));
    fx.done(&tgt);
    fx.wake_at(None);
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
    fx.effects.push(Effect::StartWorkflow {
        key,
        workflow: WorkflowKind::ChangeRequest,
        event: Box::new(event),
    });
    Transition::to(
        st(IssuePhase::Done, d),
        fx.finish(),
        "change request opened",
    )
}
