//! Pure workflow state machines: `issue`, `change_request` and `mention`.
//!
//! No I/O happens here. The reducer loads a work item, builds a [`Ctx`] (with the
//! actor's permission already resolved through `Connector::check_permission`), calls
//! [`transition`], and persists the result: the next state, and the [`Effect`]s in
//! order (so `CancelJobs` applies before the jobs that follow it in the same list).
//!
//! # Persistence
//! * `work_items.state` is [`WorkflowState::state_name`], `work_items.state_data` is
//!   [`WorkflowState::state_data`], and `work_items.workflow_version` is
//!   [`WORKFLOW_VERSION`]. Load with [`WorkflowState::from_row`].
//! * `transitions.effects` is `serde_json::to_value(&transition.effects)`.
//!
//! # Events
//! [`EngineEvent::from_row`] parses a `workflows.events` row. `JobSucceeded` /
//! `JobFailed` / `TimerFired` rows are engine-internal and carry the payloads
//! documented on [`EventPayload`].
//!
//! # Job inputs (`jobs.input`)
//! * `set_progress`: `{"target": ProgressTarget, "progress": Progress}`
//! * `post_comment`: `{"item": WorkItemKey, "body": str}`
//! * `open_change_request`: `{"spec": ChangeRequestSpec}`
//! * `agent_*`: [`AgentJobInput`]; see [`prompt`] for the output contract.
//!
//! Job outputs that the machines read: agent jobs store the `RunFinished` body
//! (`summary` is Claude's result text, plus `branch`, `head_sha`, `pushed`),
//! `post_comment` stores a `CommentRef`, `open_change_request` an `ItemRef`.

mod change_request;
mod fx;
mod issue;
mod mention;
pub mod prompt;

#[cfg(test)]
mod tests;

use chrono::{DateTime, Utc};
use enzarb_connector::{
    Actor, CommentKind, CommentRef, Event, EventType, ItemKind, Permission, ProgressTarget,
    WorkItemKey,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use change_request::{CrData, CrPhase, CrState};
pub use fx::{
    AgentJobInput, PRIORITY_AGENT, PRIORITY_COMMENT, PRIORITY_PROGRESS, PRIORITY_WORKING,
};
pub use issue::{IssueData, IssuePhase, IssueState};
pub use mention::{MentionData, MentionPhase, MentionState};
pub use prompt::{AgentOutput, Decision, OutputError, PromptContext, render_prompt};

/// Version of the `state_data` shapes written by this module. Readers of older
/// versions must be added to [`WorkflowState::from_row`] when this is bumped.
pub const WORKFLOW_VERSION: i32 = 1;

// ---------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    SetProgress,
    PostComment,
    OpenChangeRequest,
    AgentTriage,
    AgentPlan,
    AgentImplement,
    AgentReview,
    AgentAddressComments,
    AgentTask,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::SetProgress => "set_progress",
            JobKind::PostComment => "post_comment",
            JobKind::OpenChangeRequest => "open_change_request",
            JobKind::AgentTriage => "agent_triage",
            JobKind::AgentPlan => "agent_plan",
            JobKind::AgentImplement => "agent_implement",
            JobKind::AgentReview => "agent_review",
            JobKind::AgentAddressComments => "agent_address_comments",
            JobKind::AgentTask => "agent_task",
        }
    }

    pub fn is_agent(self) -> bool {
        !matches!(
            self,
            JobKind::SetProgress | JobKind::PostComment | JobKind::OpenChangeRequest
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiActionKind {
    Approve,
    Cancel,
    Retry,
}

/// A `workflows_api.submit_action` event (source `ui`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiAction {
    pub actor: Actor,
    pub action: UiActionKind,
    #[serde(default)]
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventPayload {
    /// A normalized connector event (`workflows.events.payload`).
    Connector(Box<Event>),
    /// `ApprovalGranted` / `CancelRequested` / `RetryRequested` from the UI.
    Ui(UiAction),
    /// Event type `JobSucceeded`, payload `{job_id, kind, output}`.
    JobSucceeded {
        job_id: i64,
        kind: JobKind,
        output: Value,
    },
    /// Event type `JobFailed`, payload `{job_id, kind, error}`.
    JobFailed {
        job_id: i64,
        kind: JobKind,
        error: String,
    },
    /// Event type `TimerFired`, payload `{job_id?}`. Without `job_id` the work
    /// item's `wake_at` is due; with it, that running job passed its deadline.
    TimerFired { job_id: Option<i64> },
}

/// One input to the engine. `id` is unique per event (the `events.id` as text) and
/// feeds the deterministic job idempotency keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EngineEvent {
    pub id: String,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    UnknownType(String),
    BadPayload(String, String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownType(t) => write!(f, "unknown event type {t:?}"),
            ParseError::BadPayload(t, e) => write!(f, "bad payload for {t}: {e}"),
        }
    }
}
impl std::error::Error for ParseError {}

impl EngineEvent {
    /// Parse a `workflows.events` row. UI actions are recognized by their
    /// `{actor, action, data}` payload shape.
    pub fn from_row(
        id: impl Into<String>,
        event_type: &str,
        payload: &Value,
    ) -> Result<Self, ParseError> {
        let bad = |e: serde_json::Error| ParseError::BadPayload(event_type.into(), e.to_string());
        let payload = match event_type {
            "JobSucceeded" | "JobFailed" => {
                #[derive(Deserialize)]
                struct P {
                    job_id: i64,
                    kind: JobKind,
                    #[serde(default)]
                    output: Value,
                    #[serde(default)]
                    error: String,
                }
                let p: P = serde_json::from_value(payload.clone()).map_err(bad)?;
                if event_type == "JobSucceeded" {
                    EventPayload::JobSucceeded {
                        job_id: p.job_id,
                        kind: p.kind,
                        output: p.output,
                    }
                } else {
                    EventPayload::JobFailed {
                        job_id: p.job_id,
                        kind: p.kind,
                        error: p.error,
                    }
                }
            }
            "TimerFired" => {
                #[derive(Deserialize)]
                struct P {
                    #[serde(default)]
                    job_id: Option<i64>,
                }
                let p: P = serde_json::from_value(payload.clone()).map_err(bad)?;
                EventPayload::TimerFired { job_id: p.job_id }
            }
            "CancelRequested" | "RetryRequested" => {
                EventPayload::Ui(serde_json::from_value(payload.clone()).map_err(bad)?)
            }
            "ApprovalGranted" if payload.get("action").is_some() => {
                EventPayload::Ui(serde_json::from_value(payload.clone()).map_err(bad)?)
            }
            t if EventType::ALL.iter().any(|e| e.as_str() == t) => EventPayload::Connector(
                Box::new(serde_json::from_value(payload.clone()).map_err(bad)?),
            ),
            other => return Err(ParseError::UnknownType(other.into())),
        };
        Ok(Self {
            id: id.into(),
            payload,
        })
    }

    /// The connector event, if this is one.
    fn connector(&self) -> Option<&Event> {
        match &self.payload {
            EventPayload::Connector(e) => Some(e),
            _ => None,
        }
    }

    fn actor(&self) -> Option<&Actor> {
        match &self.payload {
            EventPayload::Connector(e) => Some(&e.data.actor),
            EventPayload::Ui(u) => Some(&u.actor),
            _ => None,
        }
    }

    fn is_approval(&self) -> bool {
        match &self.payload {
            EventPayload::Connector(e) => e.event_type == EventType::ApprovalGranted,
            EventPayload::Ui(u) => u.action == UiActionKind::Approve,
            _ => false,
        }
    }

    /// `Some(merged)` for `WorkItemClosed`.
    fn closed(&self) -> Option<bool> {
        let e = self.connector()?;
        (e.event_type == EventType::WorkItemClosed).then(|| e.data.merged.unwrap_or(false))
    }

    /// `CommentAdded` and `Mentioned` look the same to workflows that already own
    /// the item (workflows.md: a mention on an item with a workflow is a comment).
    fn comment_like(&self) -> Option<CommentLike<'_>> {
        let e = self.connector()?;
        if !matches!(e.event_type, EventType::CommentAdded | EventType::Mentioned) {
            return None;
        }
        Some(CommentLike {
            actor: &e.data.actor,
            comment: e.data.comment.as_ref(),
            text: e
                .data
                .comment
                .as_ref()
                .map(|c| c.body.as_str())
                .unwrap_or(e.data.item.body.as_str()),
            mention: e.event_type == EventType::Mentioned,
        })
    }
}

struct CommentLike<'a> {
    actor: &'a Actor,
    comment: Option<&'a CommentRef>,
    text: &'a str,
    mention: bool,
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// Timeouts and limits. All durations are seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// `AwaitingAnswers`: nudge the author after this long without an answer.
    pub nudge_after_secs: i64,
    /// `AwaitingAnswers`: after the nudge, mark the issue stale after this long.
    pub stale_after_secs: i64,
    /// Deadline handed to agent runs (`run_start.timeout_secs`).
    pub agent_timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            nudge_after_secs: 3 * 24 * 3600,
            stale_after_secs: 4 * 24 * 3600,
            agent_timeout_secs: 3600,
        }
    }
}

/// Which Project triggers are enabled (`Project.spec.triggers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triggers {
    pub issues: bool,
    pub change_requests: bool,
    pub mentions: bool,
}

impl Default for Triggers {
    fn default() -> Self {
        Self {
            issues: true,
            change_requests: true,
            mentions: true,
        }
    }
}

/// Everything a transition needs besides the state and the event.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub work_item: WorkItemKey,
    /// Time the reducer is processing the event; used for `wake_at`.
    pub now: DateTime<Utc>,
    /// `ENZARB_BOT_LOGIN`; its events are ignored.
    pub bot_login: String,
    /// `ENZARB_BOT_HANDLE`, only used in prompts.
    pub bot_handle: String,
    /// The event actor's repository permission, resolved by the reducer
    /// (`Permission::None` for events without an actor).
    pub actor_permission: Permission,
    pub config: Config,
    pub triggers: Triggers,
    /// Default branch of the work item's repository.
    pub default_branch: String,
}

impl Ctx {
    fn progress_target(
        &self,
        comment_id: Option<u64>,
        comment_kind: CommentKind,
    ) -> ProgressTarget {
        ProgressTarget {
            item: self.work_item.clone(),
            comment_id,
            comment_kind,
        }
    }

    fn can_write(&self) -> bool {
        self.actor_permission.can_write()
    }

    /// True for the bot itself and for other bot accounts.
    fn is_bot(&self, a: &Actor) -> bool {
        a.is_bot || a.login.eq_ignore_ascii_case(&self.bot_login)
    }
}

// ---------------------------------------------------------------------------
// Effects and results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowKind {
    Issue,
    ChangeRequest,
    Mention,
}

impl WorkflowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkflowKind::Issue => "issue",
            WorkflowKind::ChangeRequest => "change_request",
            WorkflowKind::Mention => "mention",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum Effect {
    /// Insert a job (`ON CONFLICT (idempotency_key) DO NOTHING`).
    Job {
        kind: JobKind,
        priority: i32,
        idempotency_key: String,
        input: Value,
    },
    /// Cancel this work item's queued and running jobs (applied in effect order).
    CancelJobs,
    /// Set (or clear) `work_items.wake_at`.
    SetWakeAt { at: Option<DateTime<Utc>> },
    /// Create another work item and feed it `event` as its first event: the reducer
    /// inserts it into `workflows.events` (dedupe key `handoff:<event.id>`) in the
    /// same transaction. Used for the issue → change_request handoff.
    StartWorkflow {
        key: WorkItemKey,
        workflow: WorkflowKind,
        event: Box<Event>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub next: WorkflowState,
    pub effects: Vec<Effect>,
    /// False when the event was not applicable: unauthorized, a duplicate, from
    /// the bot, or stale. Then `next` equals the input state and `effects` is empty.
    pub accepted: bool,
    pub note: String,
}

impl Transition {
    pub(crate) fn reject(state: &WorkflowState, note: impl Into<String>) -> Self {
        Self {
            next: state.clone(),
            effects: Vec::new(),
            accepted: false,
            note: note.into(),
        }
    }

    /// Accepted but nothing to do (for example the ack of a finished `post_comment`).
    pub(crate) fn ack(state: &WorkflowState, note: impl Into<String>) -> Self {
        Self {
            next: state.clone(),
            effects: Vec::new(),
            accepted: true,
            note: note.into(),
        }
    }

    pub(crate) fn to(next: WorkflowState, effects: Vec<Effect>, note: impl Into<String>) -> Self {
        Self {
            next,
            effects,
            accepted: true,
            note: note.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowState {
    /// No work item yet; the first event decides whether one is created.
    New(WorkflowKind),
    Issue(IssueState),
    ChangeRequest(CrState),
    Mention(MentionState),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateError(pub String);

impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for StateError {}

fn name_of<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => unreachable!("phase enums serialize to strings"),
    }
}

fn phase_from<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, StateError> {
    serde_json::from_value(Value::String(s.to_string()))
        .map_err(|_| StateError(format!("unknown state {s:?}")))
}

fn data_from<T: for<'de> Deserialize<'de>>(v: &Value) -> Result<T, StateError> {
    serde_json::from_value(v.clone()).map_err(|e| StateError(format!("bad state_data: {e}")))
}

impl WorkflowState {
    pub fn workflow(&self) -> WorkflowKind {
        match self {
            WorkflowState::New(k) => *k,
            WorkflowState::Issue(_) => WorkflowKind::Issue,
            WorkflowState::ChangeRequest(_) => WorkflowKind::ChangeRequest,
            WorkflowState::Mention(_) => WorkflowKind::Mention,
        }
    }

    /// `work_items.state`.
    pub fn state_name(&self) -> String {
        match self {
            WorkflowState::New(_) => "New".into(),
            WorkflowState::Issue(s) => name_of(&s.phase),
            WorkflowState::ChangeRequest(s) => name_of(&s.phase),
            WorkflowState::Mention(s) => name_of(&s.phase),
        }
    }

    /// `work_items.state_data`.
    pub fn state_data(&self) -> Value {
        let v = match self {
            WorkflowState::New(_) => serde_json::json!({}),
            WorkflowState::Issue(s) => serde_json::to_value(&s.data).unwrap_or_default(),
            WorkflowState::ChangeRequest(s) => serde_json::to_value(&s.data).unwrap_or_default(),
            WorkflowState::Mention(s) => serde_json::to_value(&s.data).unwrap_or_default(),
        };
        if v.is_null() {
            serde_json::json!({})
        } else {
            v
        }
    }

    /// Done states cannot change except by a restart (`Failed` and `Stale` can
    /// be retried, so they are not terminal here).
    pub fn is_terminal(&self) -> bool {
        match self {
            WorkflowState::New(_) => false,
            WorkflowState::Issue(s) => matches!(
                s.phase,
                IssuePhase::Done | IssuePhase::Duplicate | IssuePhase::Cancelled
            ),
            WorkflowState::ChangeRequest(s) => matches!(
                s.phase,
                CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled
            ),
            WorkflowState::Mention(s) => matches!(s.phase, MentionPhase::Cancelled),
        }
    }

    /// Rebuild from a `work_items` row. `version` selects the `state_data` shape.
    pub fn from_row(
        workflow: &str,
        version: i32,
        state: &str,
        data: &Value,
    ) -> Result<Self, StateError> {
        if version != WORKFLOW_VERSION {
            return Err(StateError(format!(
                "unsupported workflow_version {version} (this build reads {WORKFLOW_VERSION})"
            )));
        }
        Ok(match workflow {
            "issue" => WorkflowState::Issue(IssueState {
                phase: phase_from(state)?,
                data: data_from(data)?,
            }),
            "change_request" => WorkflowState::ChangeRequest(CrState {
                phase: phase_from(state)?,
                data: data_from(data)?,
            }),
            "mention" => WorkflowState::Mention(MentionState {
                phase: phase_from(state)?,
                data: data_from(data)?,
            }),
            other => return Err(StateError(format!("unknown workflow {other:?}"))),
        })
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Which workflow a first event for an unknown work item starts, honoring the
/// Project triggers. `None` means the event is ignored (the reducer records an
/// `ignored` transition without creating a work item).
pub fn route(ctx: &Ctx, ev: &EngineEvent) -> Option<WorkflowKind> {
    let e = ev.connector()?;
    match e.event_type {
        EventType::WorkItemOpened if e.data.item.kind == ItemKind::Issue && ctx.triggers.issues => {
            Some(WorkflowKind::Issue)
        }
        EventType::ChangeRequestOpened
            if e.data.item.kind == ItemKind::ChangeRequest && ctx.triggers.change_requests =>
        {
            Some(WorkflowKind::ChangeRequest)
        }
        EventType::Mentioned if ctx.triggers.mentions => Some(WorkflowKind::Mention),
        _ => None,
    }
}

/// The pure transition function.
pub fn transition(ctx: &Ctx, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    match state {
        WorkflowState::New(kind) => match (kind, route(ctx, ev)) {
            (k, Some(r)) if *k == r => match k {
                WorkflowKind::Issue => issue::start(ctx, state, ev),
                WorkflowKind::ChangeRequest => change_request::start(ctx, state, ev),
                WorkflowKind::Mention => mention::start(ctx, state, ev),
            },
            _ => Transition::reject(state, "event does not start this workflow"),
        },
        WorkflowState::Issue(s) => issue::step(ctx, s, ev),
        WorkflowState::ChangeRequest(s) => change_request::step(ctx, s, ev),
        WorkflowState::Mention(s) => mention::step(ctx, s, ev),
    }
}
