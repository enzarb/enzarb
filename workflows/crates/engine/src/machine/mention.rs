//! The `mention` workflow: `Mentioned` on an item without another workflow runs one
//! `agent_task` job, posts the reply and is `Done`. A later mention restarts it.
//! A mention on an item that already has an issue or change_request workflow never
//! gets here: that workflow treats it as a comment.
//!
//! Running an agent on request needs write access, so mentions from users without
//! it are not accepted.

use enzarb_connector::{CommentKind, EventType, ItemKind, ProgressTarget};
use serde::{Deserialize, Serialize};

use super::fx::{AgentSpec, Fx};
use super::prompt::{AgentOutput, Decision, PromptContext};
use super::{Ctx, EngineEvent, EventPayload, JobKind, Transition, UiActionKind, WorkflowState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MentionPhase {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MentionData {
    pub number: u64,
    pub change_request: bool,
    pub url: String,
    pub title: String,
    pub body: String,
    pub author: String,
    pub head_ref: String,
    pub trigger_comment_id: Option<u64>,
    pub trigger_comment_kind: CommentKind,
    pub trigger_text: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MentionState {
    pub phase: MentionPhase,
    pub data: MentionData,
}

fn st(phase: MentionPhase, data: MentionData) -> WorkflowState {
    WorkflowState::Mention(MentionState { phase, data })
}

fn target(ctx: &Ctx, d: &MentionData) -> ProgressTarget {
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

fn begin(ctx: &Ctx, ev: &EngineEvent, d: MentionData, note: &str) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    let tgt = target(ctx, &d);
    fx.received(&tgt);
    fx.wake_at(None);
    let base_ref = if d.change_request && !d.head_ref.is_empty() {
        d.head_ref.clone()
    } else {
        ctx.default_branch.clone()
    };
    fx.agent(
        JobKind::AgentTask,
        &tgt,
        AgentSpec {
            base_ref,
            branch: format!("enzarb/task-{}", d.number),
            push: false,
        },
        PromptContext {
            bot_handle: ctx.bot_handle.clone(),
            repo: ctx.work_item.repo.to_string(),
            repo_name: ctx.work_item.repo.full_name(),
            item_kind: if d.change_request {
                "change request"
            } else {
                "issue"
            }
            .into(),
            number: d.number.to_string(),
            url: d.url.clone(),
            title: d.title.clone(),
            body: d.body.clone(),
            author: d.author.clone(),
            trigger: d.trigger_text.clone(),
            base_ref: ctx.default_branch.clone(),
            head_ref: d.head_ref.clone(),
            ..Default::default()
        },
    );
    Transition::to(st(MentionPhase::Running, d), fx.finish(), note)
}

/// Data for a mention event.
fn mention_data(ev: &EngineEvent) -> Option<MentionData> {
    let e = ev.connector()?;
    let c = ev.comment_like()?;
    if e.event_type != EventType::Mentioned {
        return None;
    }
    let item = &e.data.item;
    Some(MentionData {
        number: item.number,
        change_request: item.kind == ItemKind::ChangeRequest,
        url: item.url.clone(),
        title: item.title.clone(),
        body: item.body.clone(),
        author: item.author.clone(),
        head_ref: item.head_ref.clone().unwrap_or_default(),
        trigger_comment_id: c.comment.map(|x| x.id),
        trigger_comment_kind: c.comment.map(|x| x.kind).unwrap_or_default(),
        trigger_text: c.text.to_string(),
        error: String::new(),
    })
}

fn gate(ctx: &Ctx, ev: &EngineEvent) -> Result<MentionData, &'static str> {
    let d = mention_data(ev).ok_or("not a mention")?;
    let c = ev.comment_like().expect("mention");
    if ctx.is_bot(c.actor) {
        return Err("bot actor ignored");
    }
    if !ctx.can_write() {
        return Err("mentions require write access");
    }
    Ok(d)
}

pub(super) fn start(ctx: &Ctx, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    match gate(ctx, ev) {
        Ok(d) => begin(ctx, ev, d, "mentioned"),
        Err(why) => Transition::reject(state, why),
    }
}

fn cancel(ctx: &Ctx, ev: &EngineEvent, s: &MentionState, comment: Option<String>) -> Transition {
    let mut fx = Fx::new(ctx, ev);
    fx.cancel_jobs();
    fx.wake_at(None);
    if let Some(c) = comment {
        fx.comment(c);
    }
    Transition::to(
        st(MentionPhase::Cancelled, s.data.clone()),
        fx.finish(),
        "cancelled",
    )
}

fn fail(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &MentionState,
    error: &str,
    cancel_jobs: bool,
) -> Transition {
    let mut d = s.data.clone();
    d.error = truncate(error, 500);
    let mut fx = Fx::new(ctx, ev);
    if cancel_jobs {
        fx.cancel_jobs();
    }
    fx.wake_at(None);
    fx.comment(format!(
        "enzarb could not complete this request: {}\n\nA maintainer can retry it from the enzarb UI.",
        d.error
    ));
    fx.failed(&target(ctx, &d));
    Transition::to(st(MentionPhase::Failed, d), fx.finish(), "failed")
}

pub(super) fn step(ctx: &Ctx, s: &MentionState, ev: &EngineEvent) -> Transition {
    use MentionPhase::*;
    let state = WorkflowState::Mention(s.clone());

    if ev.closed().is_some() {
        return if s.phase == Cancelled {
            Transition::reject(&state, "already finished")
        } else {
            cancel(ctx, ev, s, None)
        };
    }

    match &ev.payload {
        EventPayload::Ui(u) => match u.action {
            UiActionKind::Cancel if !ctx.can_cancel() => {
                Transition::reject(&state, "cancel requires write access")
            }
            UiActionKind::Cancel if s.phase == Cancelled => {
                Transition::reject(&state, "already finished")
            }
            UiActionKind::Cancel => cancel(ctx, ev, s, ctx.cancel_comment(&u.actor.login)),
            UiActionKind::RepoRemoved if s.phase == Cancelled => {
                Transition::reject(&state, "already finished")
            }
            UiActionKind::RepoRemoved => cancel(ctx, ev, s, None).repo_removed(),
            UiActionKind::Retry if !ctx.can_write() => {
                Transition::reject(&state, "retry requires write access")
            }
            UiActionKind::Retry if s.phase == Failed => {
                let mut d = s.data.clone();
                d.error.clear();
                begin(ctx, ev, d, "retry")
            }
            UiActionKind::Retry => Transition::reject(&state, "nothing to retry"),
            UiActionKind::Approve => Transition::reject(&state, "nothing to approve"),
            UiActionKind::Merge => Transition::reject(&state, "nothing to merge"),
            UiActionKind::Comment => {
                Transition::reject(&state, "comments from the UI are for issues")
            }
        },
        EventPayload::JobSucceeded { kind, output, .. } => {
            if matches!(kind, JobKind::SetProgress | JobKind::PostComment) {
                return Transition::ack(&state, "ack");
            }
            if *kind != JobKind::AgentTask || s.phase != Running {
                return Transition::reject(&state, "stale job result");
            }
            match AgentOutput::from_job_output(*kind, output) {
                Ok((Decision::Reply, out)) => {
                    let mut fx = Fx::new(ctx, ev);
                    fx.comment(out.body);
                    fx.done(&target(ctx, &s.data));
                    Transition::to(st(Done, s.data.clone()), fx.finish(), "replied")
                }
                Ok((_, out)) => fail(ctx, ev, s, &out.body, false),
                Err(e) => fail(ctx, ev, s, &format!("unusable agent output: {e}"), false),
            }
        }
        EventPayload::JobFailed { kind, error, .. } => match kind {
            JobKind::SetProgress | JobKind::PostComment => {
                Transition::ack(&state, "best-effort job failed; ignored")
            }
            JobKind::AgentTask if s.phase == Running => {
                fail(ctx, ev, s, &format!("agent_task failed: {error}"), false)
            }
            _ => Transition::reject(&state, "stale job result"),
        },
        EventPayload::TimerFired { job_id: Some(_) } if s.phase == Running => {
            fail(ctx, ev, s, "the agent run exceeded its deadline", true)
        }
        EventPayload::TimerFired { .. } => Transition::reject(&state, "no timer expected"),
        EventPayload::Unmergeable { .. }
        | EventPayload::DependenciesChanged
        | EventPayload::ReadinessPoll => Transition::reject(&state, "not a change request"),
        EventPayload::Connector(_) => match ev.comment_like() {
            Some(c) if c.mention => match (s.phase, gate(ctx, ev)) {
                (_, Err(why)) => Transition::reject(&state, why),
                (Running, Ok(_)) => Transition::reject(&state, "busy: a task is running"),
                (Cancelled, Ok(_)) => Transition::reject(&state, "no longer active"),
                (Done | Failed, Ok(d)) => begin(ctx, ev, d, "mentioned again"),
            },
            _ => Transition::reject(&state, "event not applicable to a mention"),
        },
    }
}
