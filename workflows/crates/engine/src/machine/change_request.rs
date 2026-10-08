//! The `change_request` workflow (workflows.md state diagram).
//!
//! Differences from the diagram, all additive:
//! * `Merged`/`Closed` are reachable from every non-final state, not only from
//!   `AwaitingChanges`; `Cancelled` is the UI cancel, `Failed` is retryable.
//! * Feedback that arrives while comments are being addressed is remembered and
//!   handled by one more `AddressingComments` round, instead of being dropped.
//! * Commits pushed by the agent itself are not re-reviewed automatically; any
//!   other new commits (`ChangeRequestUpdated`) are.
//! * A `ChangeRequestOpened` for a `Closed` or `Cancelled` item (reopen) restarts the review.

use enzarb_connector::{CommentKind, EventType, ItemKind, ItemRef, ProgressTarget};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fx::{AgentSpec, Fx};
use super::prompt::{AgentOutput, Decision, PromptContext};
use super::{Ctx, EngineEvent, EventPayload, JobKind, Transition, UiActionKind, WorkflowState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrPhase {
    Reviewing,
    AwaitingChanges,
    AddressingComments,
    Merged,
    Closed,
    Failed,
    Cancelled,
}

/// Feedback received while a run was in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub comment_id: Option<u64>,
    #[serde(default)]
    pub comment_kind: CommentKind,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrData {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub author: String,
    pub head_ref: String,
    pub base_ref: String,
    pub head_sha: String,
    /// Commit the last review covered; a `ChangeRequestUpdated` for it is a duplicate.
    pub reviewed_sha: String,
    pub trigger_comment_id: Option<u64>,
    pub trigger_comment_kind: CommentKind,
    pub trigger_text: String,
    pub pending: Option<Pending>,
    /// Verdict of the last review: `approve`, `request_changes` or `comment`.
    pub last_review: String,
    pub failed_phase: Option<CrPhase>,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CrState {
    pub phase: CrPhase,
    pub data: CrData,
}

fn st(phase: CrPhase, data: CrData) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState { phase, data })
}

fn target(ctx: &Ctx, d: &CrData) -> ProgressTarget {
    ctx.progress_target(d.trigger_comment_id, d.trigger_comment_kind)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}...")
    }
}

fn prompt_ctx(ctx: &Ctx, d: &CrData) -> PromptContext {
    PromptContext {
        bot_handle: ctx.bot_handle.clone(),
        repo: ctx.work_item.repo.to_string(),
        repo_name: ctx.work_item.repo.full_name(),
        item_kind: "change request".into(),
        number: d.number.to_string(),
        url: d.url.clone(),
        title: d.title.clone(),
        body: d.body.clone(),
        author: d.author.clone(),
        trigger: d.trigger_text.clone(),
        base_ref: d.base_ref.clone(),
        head_ref: d.head_ref.clone(),
        head_sha: d.head_sha.clone(),
        branch: d.head_ref.clone(),
        ..Default::default()
    }
}

fn start_review(ctx: &Ctx, fx: &mut Fx, d: &CrData) {
    fx.agent(
        JobKind::AgentReview,
        &target(ctx, d),
        AgentSpec {
            // Review the source branch as it is; nothing is pushed.
            base_ref: d.head_ref.clone(),
            branch: d.head_ref.clone(),
            push: false,
        },
        prompt_ctx(ctx, d),
    );
}

fn start_address(ctx: &Ctx, fx: &mut Fx, d: &CrData) {
    fx.agent(
        JobKind::AgentAddressComments,
        &target(ctx, d),
        AgentSpec {
            base_ref: d.head_ref.clone(),
            branch: d.head_ref.clone(),
            push: true,
        },
        prompt_ctx(ctx, d),
    );
}

fn begin(ctx: &Ctx, ev: &EngineEvent, phase: CrPhase, d: CrData, note: &str) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx, &d));
    fx.wake_at(None);
    match phase {
        CrPhase::Reviewing => start_review(ctx, &mut fx, &d),
        CrPhase::AddressingComments => start_address(ctx, &mut fx, &d),
        _ => unreachable!("begin is only used for agent phases"),
    }
    Transition::to(st(phase, d), fx.finish(), note)
}

fn fail(ctx: &Ctx, ev: &EngineEvent, s: &CrState, error: &str, cancel_jobs: bool) -> Transition {
    let mut d = s.data.clone();
    d.failed_phase = Some(s.phase);
    d.error = truncate(error, 500);
    d.pending = None;
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
    Transition::to(st(CrPhase::Failed, d), fx.finish(), "failed")
}

fn finish(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &CrState,
    phase: CrPhase,
    comment: Option<String>,
) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    fx.cancel_jobs();
    fx.wake_at(None);
    if let Some(c) = comment {
        fx.comment(c);
    }
    Transition::to(st(phase, s.data.clone()), fx.finish(), "finished")
}

fn data_from_item(item: &ItemRef) -> CrData {
    CrData {
        number: item.number,
        url: item.url.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        author: item.author.clone(),
        head_ref: item.head_ref.clone().unwrap_or_default(),
        base_ref: item.base_ref.clone().unwrap_or_default(),
        head_sha: item.head_sha.clone().unwrap_or_default(),
        reviewed_sha: item.head_sha.clone().unwrap_or_default(),
        ..Default::default()
    }
}

pub(super) fn start(ctx: &Ctx, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    match ev.connector() {
        Some(e)
            if e.event_type == EventType::ChangeRequestOpened
                && e.data.item.kind == ItemKind::ChangeRequest =>
        {
            let mut d = data_from_item(&e.data.item);
            if d.base_ref.is_empty() {
                d.base_ref = ctx.default_branch.clone();
            }
            begin(ctx, ev, CrPhase::Reviewing, d, "change request opened")
        }
        _ => Transition::reject(state, "not an opening event"),
    }
}

pub(super) fn step(ctx: &Ctx, s: &CrState, ev: &EngineEvent) -> Transition {
    use CrPhase::*;
    let state = WorkflowState::ChangeRequest(s.clone());
    let finished = matches!(s.phase, Merged | Closed | Cancelled);

    if let Some(merged) = ev.closed() {
        if finished {
            return Transition::reject(&state, "already finished");
        }
        return finish(ctx, ev, s, if merged { Merged } else { Closed }, None);
    }

    match &ev.payload {
        EventPayload::Ui(u) => match u.action {
            UiActionKind::Cancel => {
                if !ctx.can_write() {
                    Transition::reject(&state, "cancel requires write access")
                } else if finished {
                    Transition::reject(&state, "already finished")
                } else {
                    finish(
                        ctx,
                        ev,
                        s,
                        Cancelled,
                        Some(format!("enzarb: cancelled by @{}.", u.actor.login)),
                    )
                }
            }
            UiActionKind::Retry => retry(ctx, ev, s, &state),
            UiActionKind::Approve => Transition::reject(&state, "nothing to approve"),
        },
        EventPayload::JobSucceeded { kind, output, .. } => {
            job_succeeded(ctx, ev, s, &state, *kind, output)
        }
        EventPayload::JobFailed { kind, error, .. } => {
            if *kind == JobKind::SetProgress || *kind == JobKind::PostComment {
                Transition::ack(&state, "best-effort job failed; ignored")
            } else if awaited(s.phase) == Some(*kind) {
                fail(
                    ctx,
                    ev,
                    s,
                    &format!("{} failed: {error}", kind.as_str()),
                    false,
                )
            } else {
                Transition::reject(&state, "stale job result")
            }
        }
        EventPayload::TimerFired { job_id: Some(_) }
            if matches!(s.phase, Reviewing | AddressingComments) =>
        {
            fail(ctx, ev, s, "the agent run exceeded its deadline", true)
        }
        EventPayload::TimerFired { .. } => Transition::reject(&state, "no timer expected"),
        EventPayload::Connector(e) => match e.event_type {
            EventType::ChangeRequestOpened => reopened(ctx, ev, s, &state),
            EventType::ChangeRequestUpdated => updated(ctx, ev, s, &state),
            EventType::ReviewSubmitted => review_submitted(ctx, ev, s, &state),
            EventType::CommentAdded | EventType::Mentioned => {
                let c = ev.comment_like().expect("comment-like");
                feedback(
                    ctx,
                    ev,
                    s,
                    &state,
                    c.actor,
                    c.comment.map(|x| (x.id, x.kind)),
                    c.text,
                )
            }
            EventType::ApprovalGranted => Transition::reject(&state, "nothing to approve"),
            _ => Transition::reject(&state, "event not applicable to a change request"),
        },
    }
}

fn reopened(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    let item = &ev.connector().expect("connector").data.item;
    if !matches!(s.phase, CrPhase::Closed | CrPhase::Cancelled) {
        return Transition::reject(state, "duplicate: already tracked");
    }
    let mut d = data_from_item(item);
    if d.base_ref.is_empty() {
        d.base_ref = ctx.default_branch.clone();
    }
    begin(ctx, ev, CrPhase::Reviewing, d, "reopened")
}

fn updated(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    use CrPhase::*;
    let item = &ev.connector().expect("connector").data.item;
    let sha = item.head_sha.clone().unwrap_or_default();
    match s.phase {
        AwaitingChanges | Reviewing => {
            if sha == s.data.reviewed_sha && !sha.is_empty() {
                return Transition::reject(state, "duplicate: this commit was already reviewed");
            }
            let mut d = data_from_item(item);
            d.base_ref = if d.base_ref.is_empty() {
                s.data.base_ref.clone()
            } else {
                d.base_ref
            };
            // A review already running is for an older commit: restart it.
            let mut t = begin(ctx, ev, Reviewing, d, "new commits");
            if s.phase == Reviewing {
                t.effects.insert(0, super::Effect::CancelJobs);
            }
            t
        }
        AddressingComments => {
            // Most likely the agent's own push; it is not re-reviewed.
            let mut d = s.data.clone();
            d.head_sha = sha;
            Transition::to(
                st(AddressingComments, d),
                Vec::new(),
                "new commits during a run",
            )
        }
        Merged | Closed | Failed | Cancelled => Transition::reject(state, "no longer active"),
    }
}

fn review_submitted(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    let e = ev.connector().expect("connector");
    if ctx.is_bot(&e.data.actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    let Some(r) = &e.data.review else {
        return Transition::reject(state, "review missing");
    };
    let rs = r.state.to_ascii_lowercase();
    if rs == "approved" || rs == "dismissed" {
        return Transition::reject(state, "review requests no changes");
    }
    if rs == "commented" && r.body.trim().is_empty() {
        return Transition::reject(state, "empty review comment");
    }
    let text = if r.body.trim().is_empty() {
        format!(
            "A review requested changes ({}); see the inline comments.",
            r.state
        )
    } else {
        r.body.clone()
    };
    feedback(ctx, ev, s, state, &e.data.actor, None, &text)
}

/// A comment or review that may ask for changes.
fn feedback(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &CrState,
    state: &WorkflowState,
    actor: &enzarb_connector::Actor,
    comment: Option<(u64, CommentKind)>,
    text: &str,
) -> Transition {
    use CrPhase::*;
    if ctx.is_bot(actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    if !ctx.can_write() {
        return Transition::reject(state, "addressing feedback requires write access");
    }
    match s.phase {
        AwaitingChanges => {
            let mut d = s.data.clone();
            d.trigger_comment_id = comment.map(|c| c.0);
            d.trigger_comment_kind = comment.map(|c| c.1).unwrap_or_default();
            d.trigger_text = text.to_string();
            begin(ctx, ev, AddressingComments, d, "feedback received")
        }
        AddressingComments => {
            let mut d = s.data.clone();
            d.pending = Some(Pending {
                comment_id: comment.map(|c| c.0),
                comment_kind: comment.map(|c| c.1).unwrap_or_default(),
                text: text.to_string(),
            });
            let mut fx = Fx::new(ctx, ev);
            fx.received(&ctx.progress_target(
                comment.map(|c| c.0),
                comment.map(|c| c.1).unwrap_or_default(),
            ));
            Transition::to(
                st(AddressingComments, d),
                fx.finish(),
                "queued for the next round",
            )
        }
        Reviewing => Transition::reject(state, "busy: a review is running"),
        Merged | Closed | Failed | Cancelled => Transition::reject(state, "no longer active"),
    }
}

fn retry(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    if !ctx.can_write() {
        return Transition::reject(state, "retry requires write access");
    }
    if s.phase != CrPhase::Failed {
        return Transition::reject(state, "nothing to retry");
    }
    let mut d = s.data.clone();
    d.error.clear();
    let phase = d.failed_phase.take().unwrap_or(CrPhase::Reviewing);
    match phase {
        CrPhase::AddressingComments => {
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            begin(ctx, ev, CrPhase::AddressingComments, d, "retry")
        }
        _ => {
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            begin(ctx, ev, CrPhase::Reviewing, d, "retry")
        }
    }
}

fn awaited(phase: CrPhase) -> Option<JobKind> {
    match phase {
        CrPhase::Reviewing => Some(JobKind::AgentReview),
        CrPhase::AddressingComments => Some(JobKind::AgentAddressComments),
        _ => None,
    }
}

fn job_succeeded(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &CrState,
    state: &WorkflowState,
    kind: JobKind,
    output: &Value,
) -> Transition {
    use CrPhase::*;
    if matches!(kind, JobKind::SetProgress | JobKind::PostComment) {
        return Transition::ack(state, "ack");
    }
    if awaited(s.phase) != Some(kind) {
        return Transition::reject(state, "stale job result");
    }
    let (decision, out) = match AgentOutput::from_job_output(kind, output) {
        Ok(x) => x,
        Err(e) => return fail(ctx, ev, s, &format!("unusable agent output: {e}"), false),
    };
    let mut d = s.data.clone();
    let mut fx = Fx::new(ctx, ev);
    let tgt = target(ctx, &d);
    match (s.phase, decision) {
        (Reviewing, _) => {
            fx.comment(format!(
                "### enzarb review: {}\n\n{}",
                decision.as_str(),
                out.body
            ));
            fx.done(&tgt);
            d.last_review = decision.as_str().to_string();
            d.reviewed_sha = d.head_sha.clone();
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            d.trigger_text.clear();
            Transition::to(st(AwaitingChanges, d), fx.finish(), "review posted")
        }
        (AddressingComments, Decision::Done) => {
            fx.comment(out.body);
            fx.done(&tgt);
            // The agent's own push is not re-reviewed.
            if let Some(sha) = output.get("head_sha").and_then(Value::as_str) {
                d.head_sha = sha.to_string();
                d.reviewed_sha = sha.to_string();
            }
            match d.pending.take() {
                Some(p) => {
                    d.trigger_comment_id = p.comment_id;
                    d.trigger_comment_kind = p.comment_kind;
                    d.trigger_text = p.text;
                    start_address(ctx, &mut fx, &d);
                    Transition::to(st(AddressingComments, d), fx.finish(), "next round")
                }
                None => {
                    d.trigger_comment_id = None;
                    d.trigger_comment_kind = CommentKind::default();
                    d.trigger_text.clear();
                    Transition::to(st(AwaitingChanges, d), fx.finish(), "comments addressed")
                }
            }
        }
        (AddressingComments, Decision::Blocked) => fail(
            ctx,
            ev,
            s,
            &format!("could not address the comments: {}", out.body),
            false,
        ),
        _ => unreachable!("decision validated against the job kind"),
    }
}
