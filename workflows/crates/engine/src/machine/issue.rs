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

use enzarb_connector::{
    ChangeRequestSpec, CommentKind, Event, EventData, EventType, ItemKind, ItemRef, ProgressTarget,
    WorkItemKey,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fx::{AgentSpec, Fx};
use super::prompt::{AgentOutput, Decision, PromptContext};
use super::title::pr_title;
use super::{
    Ctx, Effect, EngineEvent, EventPayload, JobKind, Transition, UiActionKind, WorkflowKind,
    WorkflowState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssuePhase {
    Triaging,
    AwaitingAnswers,
    Stale,
    Duplicate,
    Planning,
    AwaitingApproval,
    Implementing,
    Done,
    Failed,
    Cancelled,
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
        ..Default::default()
    }
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
    fx.agent(
        JobKind::AgentImplement,
        &target(ctx, d),
        AgentSpec {
            base_ref: ctx.default_branch.clone(),
            branch: branch(d.number),
            push: true,
        },
        pc,
    );
}

/// Enter `phase` and queue its job, after a trigger that starts work.
fn begin(ctx: &Ctx, ev: &EngineEvent, phase: IssuePhase, d: IssueData, note: &str) -> Transition {
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
    if ctx.is_bot(&e.data.actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    let item = &e.data.item;
    let d = IssueData {
        number: item.number,
        url: item.url.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        author: item.author.clone(),
        ..Default::default()
    };
    begin(ctx, ev, IssuePhase::Triaging, d, "issue opened")
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
        },
        EventPayload::JobSucceeded { kind, output, .. } => {
            job_succeeded(ctx, ev, s, &state, *kind, output)
        }
        EventPayload::JobFailed { kind, error, .. } => job_failed(ctx, ev, s, &state, *kind, error),
        EventPayload::TimerFired { job_id } => timer(ctx, ev, s, &state, *job_id),
        EventPayload::Unmergeable { .. } => Transition::reject(&state, "not a change request"),
        EventPayload::Connector(_) if ev.is_approval() => approval(ctx, ev, s, &state),
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
    if s.phase != IssuePhase::AwaitingApproval {
        return Transition::reject(state, "no plan is awaiting approval");
    }
    let mut d = s.data.clone();
    let tc = ev.connector().and_then(|e| e.data.comment.as_ref());
    d.trigger_comment_id = tc.map(|c| c.id);
    d.trigger_comment_kind = tc.map(|c| c.kind).unwrap_or_default();
    d.trigger_text.clear();
    d.implemented = false;
    begin(ctx, ev, IssuePhase::Implementing, d, "plan approved")
}

use enzarb_connector::command::has_approve_command;

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
        Triaging | Planning | Implementing => Transition::reject(state, "busy: a job is running"),
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
        (Some(_), Triaging | Planning | Implementing) => {
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
        (Triaging, Decision::Ready) => {
            d.triage_summary = out.body;
            d.trigger_text.clear();
            start_plan(ctx, &mut fx, &d);
            Transition::to(st(Planning, d), fx.finish(), "triage done")
        }
        (Planning, Decision::Plan) => {
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
            fx.open_change_request(&ChangeRequestSpec {
                repo: ctx.work_item.repo.clone(),
                title: pr_title(
                    out.title.as_deref(),
                    &d.title,
                    &ctx.work_item.repo.full_name(),
                ),
                body: format!("{}\n\nCloses #{}", out.body.trim_end(), d.number),
                head_ref,
                base_ref: ctx.default_branch.clone(),
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
