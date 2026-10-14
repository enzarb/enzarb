//! The `change_request` workflow (workflows.md state diagram).
//!
//! Differences from the diagram, all additive:
//! * `Merged`/`Closed` are reachable from every non-final state, not only from
//!   `AwaitingChanges`; `Cancelled` is the UI cancel, `Failed` is retryable.
//! * Feedback that arrives while comments are being addressed is remembered and
//!   handled by one more `AddressingComments` round, instead of being dropped.
//! * enzarb's own must-fix review findings (`request_changes`, or an unverified approve
//!   downgraded to `comment`) start an `AddressingComments` round by themselves, at most
//!   `Config::max_auto_rounds` in a row without a human acting; then the item waits for a person.
//! * Feedback during `Reviewing` or `Merging` is queued like during `AddressingComments`; it is
//!   handled before any automatic round once the running job finishes.
//! * Commits pushed by the agent itself are not re-reviewed automatically; any
//!   other new commits (`ChangeRequestUpdated`) are.
//! * A `ChangeRequestOpened` for a `Closed` or `Cancelled` item (reopen) restarts the review.
//!
//! # Merging
//! `AwaitingChanges` becomes `ReadyToMerge` only through [`evaluate`], when [`readiness`]
//! says enzarb approved the current head, CI succeeded on it and it is mergeable.
//! `ReadyToMerge` becomes `Merging` only on the UI `merge` action (write access required),
//! which queues the single `merge_change_request` job and records the request in
//! `CrData::merge_requested`. If the branch was behind its base, the job updates it instead
//! and the request is kept: once the new head is reviewed, CI is green and readiness is ok,
//! [`evaluate`] queues the merge without a second click. Requested changes, a failed check
//! or a refused merge clear the request. Nothing else can start a merge.

use enzarb_connector::{CommentKind, EventType, ItemKind, ItemRef, ProgressTarget};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::fx::{AgentSpec, Fx};
use super::prompt::{AgentOutput, Decision, PromptContext};
use super::stack::{RetargetOutcome, is_enzarb_branch, parse_stack_marker};
use super::title::squash_title;
use super::{Ctx, EngineEvent, EventPayload, JobKind, Transition, UiActionKind, WorkflowState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrPhase {
    Reviewing,
    AwaitingChanges,
    /// Approved, CI green and mergeable on the current head: waiting for a human to merge.
    ReadyToMerge,
    AddressingComments,
    /// The base branch is being merged into the source branch.
    ResolvingConflicts,
    /// The conflict needs a human decision; not an engine error, so not `Failed`.
    ConflictParked,
    /// The user's merge was requested; waiting for the `merge_change_request` job.
    Merging,
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

/// Aggregate CI of `CrData::ci_sha`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiState {
    #[default]
    Unknown,
    Pending,
    Success,
    Failure,
}

impl CiState {
    fn from_wire(s: &str) -> Self {
        match s {
            "success" => CiState::Success,
            "failure" => CiState::Failure,
            "pending" => CiState::Pending,
            _ => CiState::Unknown,
        }
    }
}

/// Automatic resolution attempts allowed per base-branch head.
pub const MAX_RESOLVE_ATTEMPTS: u32 = 2;

/// Attempts made against `base_sha`; a new base head starts again from zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResolveAttempts {
    pub base_sha: String,
    pub count: u32,
}

impl CrData {
    /// Attempts already made against `base_sha`, reading the legacy single-attempt field.
    pub fn resolve_attempts_for(&self, base_sha: &str) -> u32 {
        if self.resolve_attempts.base_sha == base_sha {
            self.resolve_attempts.count
        } else if self.resolve_attempted_base_sha == base_sha {
            1
        } else {
            0
        }
    }

    fn record_resolve_attempt(&mut self, base_sha: &str) {
        let count = self.resolve_attempts_for(base_sha) + 1;
        self.resolve_attempts = ResolveAttempts {
            base_sha: base_sha.to_string(),
            count,
        };
        self.resolve_attempted_base_sha.clear();
    }
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
    /// Latest base-branch head reported by a merge-readiness check.
    pub base_sha: String,
    /// Automatic conflict-resolution attempts against one base head; capped per SHA.
    pub resolve_attempts: ResolveAttempts,
    /// Legacy form of `resolve_attempts` (one attempt per SHA). Only read, never written.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub resolve_attempted_base_sha: String,
    pub trigger_comment_id: Option<u64>,
    pub trigger_comment_kind: CommentKind,
    pub trigger_text: String,
    pub pending: Option<Pending>,
    /// Verdict of the last review: `approve`, `request_changes` or `comment`.
    pub last_review: String,
    pub failed_phase: Option<CrPhase>,
    pub error: String,
    /// CI on `ci_sha`; only trusted while `ci_sha == head_sha`.
    pub ci: CiState,
    pub ci_sha: String,
    /// Whether the provider can merge the head; `None` until it said so.
    pub mergeable: Option<bool>,
    /// Why the last merge attempt did not happen; cleared on the next `ReadyToMerge`.
    pub merge_error: String,
    /// Automatic fix rounds started since a human last acted.
    pub auto_rounds: u32,
    /// The current `AddressingComments` round was started by enzarb's own review.
    pub auto_trigger: bool,
    /// The last review had must-fix findings but `auto_rounds` hit the cap.
    pub auto_capped: bool,
    /// A user clicked merge and the merge has not happened yet. Kept across the branch
    /// update so the merge runs by itself once the new head is reviewed, CI is green and
    /// readiness is ok; cleared by requested changes, a failed check or a refused merge.
    pub merge_requested: bool,
    /// Who clicked merge; recorded on the merge comment.
    pub merge_actor: String,
    /// Dependencies that are not done yet, as the reducer last read them. Refreshed from
    /// [`Ctx::open_dependencies`] on every event; kept here so the views can show it.
    pub open_dependencies: Vec<OpenDependency>,
    /// Stacked change request: work item key of the parent change request this one is based
    /// on (its branch is `base_ref`). Empty once retargeted, parked, or when not stacked.
    pub stacked_on: String,
    /// The parent's branch, `base_ref` while stacked.
    pub stack_parent_ref: String,
    /// The parent head this branch was cut from; `--onto` rebases only the commits after it.
    pub stack_parent_tip: String,
    /// The branch to retarget onto (the Project's default branch when the stack was recorded).
    pub default_base: String,
    /// The parent was merged / closed without merging, as the reducer last read it.
    pub stack_parent_merged: bool,
    pub stack_parent_closed: bool,
    /// A `retarget_change_request` job is in flight.
    pub retargeting: bool,
    /// Retarget jobs queued so far; capped at [`MAX_RETARGET_ATTEMPTS`].
    pub retarget_attempts: u32,
}

/// Automatic retarget attempts allowed per stacked change request.
pub const MAX_RETARGET_ATTEMPTS: u32 = 2;

impl CrData {
    fn clear_stack(&mut self) {
        self.stacked_on.clear();
        self.stack_parent_ref.clear();
        self.stack_parent_tip.clear();
        self.default_base.clear();
        self.stack_parent_merged = false;
        self.stack_parent_closed = false;
        self.retargeting = false;
        self.retarget_attempts = 0;
    }

    /// Keep the stack across a rebuild of the data from the provider's item.
    fn carry_stack(&mut self, from: &CrData) {
        self.stacked_on = from.stacked_on.clone();
        self.stack_parent_ref = from.stack_parent_ref.clone();
        self.stack_parent_tip = from.stack_parent_tip.clone();
        self.default_base = from.default_base.clone();
        self.stack_parent_merged = from.stack_parent_merged;
        self.stack_parent_closed = from.stack_parent_closed;
        self.retargeting = from.retargeting;
        self.retarget_attempts = from.retarget_attempts;
    }
}

/// A dependency that blocks the merge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenDependency {
    /// Work item key of the dependency; empty for a dependency cycle.
    pub key: String,
    /// How the dependency reads from this item: `#12`, or `owner/repo#12` in another repo.
    pub label: String,
    pub url: String,
    /// Why it can never complete as things stand (closed without completing, a cycle);
    /// empty while it is merely not done yet.
    pub problem: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Pending,
    Failed,
    Unknown,
}

/// One line of the readiness checklist shown in the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub status: CheckStatus,
    pub detail: String,
}

fn check(status: CheckStatus, detail: impl Into<String>) -> Check {
    Check {
        status,
        detail: detail.into(),
    }
}

/// Whether the change request may be merged now. The only source of truth for entering,
/// staying in and leaving `ReadyToMerge`, and for the UI checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Readiness {
    pub ci: Check,
    pub review: Check,
    pub mergeable: Check,
    /// Every dependency is done. Never asks the connector, so `needs_check` ignores it.
    pub dependencies: Check,
}

impl Readiness {
    fn all_ok(&self) -> bool {
        [&self.ci, &self.review, &self.mergeable, &self.dependencies]
            .iter()
            .all(|c| c.status == CheckStatus::Ok)
    }

    /// Worth asking the connector: the review approved, nothing has failed and something
    /// is unknown or not yet mergeable.
    fn needs_check(&self) -> bool {
        self.review.status == CheckStatus::Ok
            && self.ci.status != CheckStatus::Failed
            && (self.ci.status == CheckStatus::Unknown || self.mergeable.status != CheckStatus::Ok)
    }
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

pub fn readiness(d: &CrData) -> Readiness {
    let review = if d.last_review.is_empty() || d.reviewed_sha.is_empty() {
        check(CheckStatus::Pending, "awaiting review")
    } else if d.reviewed_sha != d.head_sha {
        check(CheckStatus::Pending, "review of an older commit")
    } else {
        match d.last_review.as_str() {
            "approve" => check(
                CheckStatus::Ok,
                format!("approved {}", short(&d.reviewed_sha)),
            ),
            "request_changes" => check(CheckStatus::Failed, "changes requested"),
            _ => check(CheckStatus::Pending, "review did not approve"),
        }
    };
    let ci = if d.ci_sha.is_empty() || d.ci_sha != d.head_sha {
        check(CheckStatus::Unknown, "checking...")
    } else {
        let sha = short(&d.ci_sha);
        match d.ci {
            CiState::Success => check(CheckStatus::Ok, format!("CI passed on {sha}")),
            CiState::Failure => check(CheckStatus::Failed, format!("CI failed on {sha}")),
            CiState::Pending => check(CheckStatus::Pending, format!("CI running on {sha}")),
            CiState::Unknown => check(CheckStatus::Unknown, "checking..."),
        }
    };
    let mergeable = match d.mergeable {
        Some(true) => check(CheckStatus::Ok, "no conflicts"),
        Some(false) if !d.merge_error.is_empty() => {
            check(CheckStatus::Failed, truncate(&d.merge_error, 120))
        }
        Some(false) => check(CheckStatus::Failed, "conflicts or blocked by branch rules"),
        None => check(CheckStatus::Unknown, "checking..."),
    };
    let dependencies = if d.open_dependencies.is_empty() {
        check(CheckStatus::Ok, "no open dependencies")
    } else if let Some(p) = d.open_dependencies.iter().find(|o| !o.problem.is_empty()) {
        check(CheckStatus::Failed, truncate(&p.problem, 120))
    } else {
        let labels: Vec<&str> = d
            .open_dependencies
            .iter()
            .map(|o| o.label.as_str())
            .collect();
        check(
            CheckStatus::Pending,
            truncate(&format!("waiting on {}", labels.join(", ")), 120),
        )
    };
    // A stacked change request cannot merge before its parent: it would merge into the
    // parent's branch. After the parent merged it waits for the retarget.
    let dependencies = if d.stacked_on.is_empty() || dependencies.status == CheckStatus::Failed {
        dependencies
    } else if d.stack_parent_closed {
        check(
            CheckStatus::Failed,
            format!(
                "stacked on {}, which was closed without merging",
                d.stack_parent_ref
            ),
        )
    } else if d.stack_parent_merged
        && !d.retargeting
        && d.retarget_attempts >= MAX_RETARGET_ATTEMPTS
    {
        check(
            CheckStatus::Failed,
            "automatic retarget failed: rebase onto the default branch by hand",
        )
    } else if d.stack_parent_merged {
        check(CheckStatus::Pending, "retargeting onto the default branch")
    } else {
        check(
            CheckStatus::Pending,
            truncate(
                &format!("stacked on {}: waits for it to merge", d.stack_parent_ref),
                120,
            ),
        )
    };
    Readiness {
        ci,
        review,
        mergeable,
        dependencies,
    }
}

/// Who a change request is waiting on (`workflows_api.work_item_v.waiting_on`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitingOn {
    /// A person has to act: this counts as "Needs you".
    Human,
    /// enzarb is working on it, or will.
    Enzarb,
    Ci,
    Github,
}

/// Why a change request is not progressing, derived from the phase and data alone.
/// `reason` is a stable code, `text` a short label for the UI, `url` an optional link
/// target (a decision comment).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wait {
    pub waiting_on: WaitingOn,
    pub reason: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

fn wait_of(waiting_on: WaitingOn, reason: &'static str, text: &str) -> Option<Wait> {
    Some(Wait {
        waiting_on,
        reason,
        text: text.into(),
        url: None,
    })
}

/// The reason a change request is waiting; `None` in final states. `decision_needed`
/// needs data not stored yet and is reserved in the contract.
pub fn wait(phase: CrPhase, d: &CrData, r: &Readiness) -> Option<Wait> {
    use WaitingOn::*;
    match phase {
        CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled => None,
        CrPhase::Reviewing if d.merge_requested => wait_of(
            Enzarb,
            "updating_branch",
            "Branch updated with the base; reviewing before merging",
        ),
        CrPhase::Reviewing => wait_of(Enzarb, "reviewing", "enzarb is reviewing"),
        CrPhase::AddressingComments if d.auto_trigger => wait_of(
            Enzarb,
            "enzarb_fixing",
            "enzarb is fixing its own review findings",
        ),
        CrPhase::AddressingComments => wait_of(
            Enzarb,
            "addressing_comments",
            "enzarb is addressing review comments",
        ),
        CrPhase::ResolvingConflicts => wait_of(
            Enzarb,
            "resolving_conflict",
            "enzarb is resolving a merge conflict",
        ),
        CrPhase::ReadyToMerge => wait_of(Human, "ready_to_merge", "Ready to merge"),
        CrPhase::Merging => wait_of(Github, "merging", "Merging on GitHub"),
        CrPhase::Failed => wait_of(Human, "enzarb_failed", "enzarb failed"),
        CrPhase::ConflictParked => wait_of(Human, "conflict_parked", "Parked on a merge conflict"),
        CrPhase::AwaitingChanges if d.retargeting => wait_of(
            Enzarb,
            "retargeting",
            "enzarb is retargeting this onto the default branch",
        ),
        CrPhase::AwaitingChanges => {
            let reviewed = !d.last_review.is_empty() && d.reviewed_sha == d.head_sha;
            if !reviewed {
                return wait_of(Enzarb, "reviewing", "enzarb is reviewing");
            }
            if d.auto_capped && d.last_review != "approve" {
                return wait_of(
                    Human,
                    "round_limit",
                    "Automatic fix rounds used up, waiting for you",
                );
            }
            if r.ci.status == CheckStatus::Failed {
                return wait_of(Enzarb, "ci_failed_fixing", "CI failed, enzarb is fixing it");
            }
            match d.last_review.as_str() {
                "request_changes" => wait_of(
                    Enzarb,
                    "changes_requested_fixing",
                    "Changes requested, enzarb is fixing them",
                ),
                "approve" => match (r.ci.status, r.mergeable.status) {
                    (CheckStatus::Pending | CheckStatus::Unknown, _) if d.merge_requested => {
                        wait_of(
                            Ci,
                            "updating_branch",
                            "Branch updated; merging once CI passes",
                        )
                    }
                    (CheckStatus::Pending | CheckStatus::Unknown, _) => {
                        wait_of(Ci, "ci_pending", "Waiting for CI")
                    }
                    (_, CheckStatus::Failed) => wait_of(
                        Human,
                        "conflict_parked",
                        "Conflicts or blocked by branch rules",
                    ),
                    _ => wait_of(Ci, "ci_pending", "Waiting for GitHub to check mergeability"),
                },
                _ => wait_of(Human, "awaiting_review", "Waiting for your review"),
            }
        }
    }
}

/// `work_items.state_data` for a change request: the data plus the derived `checklist`
/// and `wait` (both ignored when the state is loaded again).
pub(super) fn state_data(s: &CrState) -> Value {
    let (phase, d) = (s.phase, &s.data);
    let mut v = serde_json::to_value(d).unwrap_or_default();
    if let Value::Object(m) = &mut v {
        let r = readiness(d);
        m.insert(
            "wait".into(),
            serde_json::to_value(wait(phase, d, &r)).unwrap_or_default(),
        );
        m.insert(
            "checklist".into(),
            serde_json::to_value(r).unwrap_or_default(),
        );
    }
    v
}

/// Reset the automatic-round bookkeeping: a human is involved again.
fn human_acted(d: &mut CrData) {
    d.auto_rounds = 0;
    d.auto_trigger = false;
    d.auto_capped = false;
}

/// Start an `AddressingComments` round for feedback that was queued during another run.
fn start_pending_round(ctx: &Ctx, mut fx: Fx, mut d: CrData, p: Pending) -> Transition {
    d.trigger_comment_id = p.comment_id;
    d.trigger_comment_kind = p.comment_kind;
    d.trigger_text = p.text;
    human_acted(&mut d);
    start_address(ctx, &mut fx, &d);
    Transition::to(
        st(CrPhase::AddressingComments, d),
        fx.finish(),
        "next round",
    )
}

/// Record the head; mergeability belongs to the old head, so it is forgotten.
fn set_head(d: &mut CrData, sha: String) {
    if d.head_sha != sha {
        d.mergeable = None;
    }
    d.head_sha = sha;
}

/// Decide between `AwaitingChanges` and `ReadyToMerge` from the data alone. `fx` carries
/// the effects the caller already queued. Unless `allow_check` is false (the answer to a
/// check never asks again), unknown readiness queues one `check_merge_readiness`.
fn evaluate(mut fx: Fx, from: CrPhase, mut d: CrData, note: &str, allow_check: bool) -> Transition {
    // The parent was merged: move enzarb's own branch onto the default branch first.
    if !d.stacked_on.is_empty()
        && d.stack_parent_merged
        && !d.retargeting
        && d.retarget_attempts < MAX_RETARGET_ATTEMPTS
        && !d.head_sha.is_empty()
    {
        d.retargeting = true;
        d.retarget_attempts += 1;
        d.merge_requested = false;
        fx.retarget_change_request(
            d.number,
            &d.head_ref,
            &d.head_sha,
            &d.stack_parent_tip,
            &d.default_base,
        );
        return Transition::to(
            st(CrPhase::AwaitingChanges, d),
            fx.finish(),
            "parent merged; retargeting",
        );
    }
    let r = readiness(&d);
    if r.ci.status == CheckStatus::Failed || r.review.status == CheckStatus::Failed {
        d.merge_requested = false;
    }
    if r.all_ok() {
        if from != CrPhase::ReadyToMerge {
            d.merge_error.clear();
        }
        if d.merge_requested {
            // The user already asked for this merge: no second click.
            let title = squash_title(&d.title, &d.head_ref);
            fx.merge_change_request(d.number, &title, &d.head_sha, &d.merge_actor);
            return Transition::to(st(CrPhase::Merging, d), fx.finish(), "merge resumed");
        }
        return Transition::to(st(CrPhase::ReadyToMerge, d), fx.finish(), "ready to merge");
    }
    if allow_check && r.needs_check() {
        fx.check_merge_readiness(d.number, &d.head_sha);
    }
    let note = if from == CrPhase::ReadyToMerge {
        "no longer ready to merge"
    } else {
        note
    };
    Transition::to(st(CrPhase::AwaitingChanges, d), fx.finish(), note)
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

fn start_resolve(ctx: &Ctx, fx: &mut Fx, d: &CrData) {
    fx.agent(
        JobKind::AgentResolveConflicts,
        &target(ctx, d),
        AgentSpec {
            base_ref: d.head_ref.clone(),
            branch: d.head_ref.clone(),
            push: true,
        },
        prompt_ctx(ctx, d),
    );
}

fn begin(ctx: &Ctx, ev: &EngineEvent, phase: CrPhase, mut d: CrData, note: &str) -> Transition {
    if phase == CrPhase::AddressingComments {
        d.merge_requested = false;
    }
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx, &d));
    fx.wake_at(None);
    match phase {
        CrPhase::Reviewing => start_review(ctx, &mut fx, &d),
        CrPhase::AddressingComments => start_address(ctx, &mut fx, &d),
        CrPhase::ResolvingConflicts => start_resolve(ctx, &mut fx, &d),
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

fn data_from_item(ctx: &Ctx, item: &ItemRef) -> CrData {
    CrData {
        open_dependencies: ctx.open_dependencies.clone(),
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
            let mut d = data_from_item(ctx, &e.data.item);
            if d.base_ref.is_empty() {
                d.base_ref = ctx.default_branch.clone();
            }
            record_stack(ctx, &mut d);
            begin(ctx, ev, CrPhase::Reviewing, d, "change request opened")
        }
        _ => Transition::reject(state, "not an opening event"),
    }
}

/// Record that a change request opened by enzarb is stacked: its issue wrote the marker, and
/// the change request's base is the parent's enzarb branch. Anything else is not a stack.
fn record_stack(ctx: &Ctx, d: &mut CrData) {
    if !d.author.eq_ignore_ascii_case(&ctx.bot_login) {
        return;
    }
    let Some((parent, parent_ref, tip)) = parse_stack_marker(&d.body) else {
        return;
    };
    if parent_ref != d.base_ref || !is_enzarb_branch(&parent_ref) || !is_enzarb_branch(&d.head_ref)
    {
        return;
    }
    d.stacked_on = parent;
    d.stack_parent_ref = parent_ref;
    d.stack_parent_tip = tip;
    d.default_base = ctx.default_branch.clone();
}

pub(super) fn step(ctx: &Ctx, stored: &CrState, ev: &EngineEvent) -> Transition {
    // Dependencies live in the database, not in the stored state: every event sees them fresh.
    let mut s = stored.clone();
    s.data.open_dependencies = ctx.open_dependencies.clone();
    if let Some(p) = ctx
        .stack_parent
        .as_ref()
        .filter(|p| !s.data.stacked_on.is_empty() && p.key == s.data.stacked_on)
    {
        s.data.stack_parent_merged = p.merged;
        s.data.stack_parent_closed = p.closed;
    }
    if matches!(ev.payload, EventPayload::DependenciesChanged) {
        return dependencies_changed(ctx, ev, stored, &s);
    }
    step_event(ctx, &s, ev)
}

/// A dependency was completed, closed, added or removed: decide again.
fn dependencies_changed(ctx: &Ctx, ev: &EngineEvent, stored: &CrState, s: &CrState) -> Transition {
    use CrPhase::*;
    let state = WorkflowState::ChangeRequest(stored.clone());
    if stored.data.open_dependencies == s.data.open_dependencies
        && stored.data.stack_parent_merged == s.data.stack_parent_merged
        && stored.data.stack_parent_closed == s.data.stack_parent_closed
    {
        return Transition::reject(&state, "dependencies unchanged");
    }
    match s.phase {
        AwaitingChanges | ReadyToMerge => evaluate(
            Fx::new(ctx, ev),
            s.phase,
            s.data.clone(),
            "dependencies changed",
            true,
        ),
        // A run or a merge is in flight: record the set; the run's end evaluates it.
        Reviewing | AddressingComments | ResolvingConflicts | ConflictParked | Merging | Failed => {
            Transition::to(
                st(s.phase, s.data.clone()),
                Vec::new(),
                "dependencies recorded",
            )
        }
        Merged | Closed | Cancelled => Transition::reject(&state, "no longer active"),
    }
}

fn step_event(ctx: &Ctx, s: &CrState, ev: &EngineEvent) -> Transition {
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
                if !ctx.can_cancel() {
                    Transition::reject(&state, "cancel requires write access")
                } else if finished {
                    Transition::reject(&state, "already finished")
                } else {
                    finish(ctx, ev, s, Cancelled, ctx.cancel_comment(&u.actor.login))
                }
            }
            UiActionKind::RepoRemoved if finished => Transition::reject(&state, "already finished"),
            UiActionKind::RepoRemoved => finish(ctx, ev, s, Cancelled, None).repo_removed(),
            UiActionKind::Retry => retry(ctx, ev, s, &state),
            UiActionKind::Approve => Transition::reject(&state, "nothing to approve"),
            UiActionKind::Merge => merge(ctx, ev, s, &state, &u.actor.login),
        },
        EventPayload::JobSucceeded { kind, output, .. } => {
            job_succeeded(ctx, ev, s, &state, *kind, output)
        }
        EventPayload::JobFailed { kind, error, .. } => {
            if *kind == JobKind::RetargetChangeRequest {
                if s.data.retargeting {
                    retarget_park(ctx, ev, s, &format!("the retarget job failed: {error}"))
                } else {
                    Transition::reject(&state, "stale job result")
                }
            } else if matches!(
                kind,
                JobKind::SetProgress | JobKind::PostComment | JobKind::CheckMergeReadiness
            ) {
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
            if matches!(s.phase, Reviewing | AddressingComments | ResolvingConflicts) =>
        {
            fail(ctx, ev, s, "the agent run exceeded its deadline", true)
        }
        EventPayload::TimerFired { job_id: Some(_) } if s.data.retargeting => {
            // Stop the run too: it must not push after the item was handed to a person.
            let mut t = retarget_park(ctx, ev, s, "the retarget exceeded its deadline");
            t.effects.insert(0, super::Effect::CancelJobs);
            t
        }
        EventPayload::TimerFired { .. } => Transition::reject(&state, "no timer expected"),
        EventPayload::Unmergeable { base_sha } => unmergeable(ctx, ev, s, &state, base_sha),
        EventPayload::DependenciesChanged => Transition::reject(&state, "handled by step"),
        EventPayload::Connector(e) => match e.event_type {
            EventType::ChangeRequestOpened => reopened(ctx, ev, s, &state),
            EventType::ChangeRequestUpdated => updated(ctx, ev, s, &state),
            EventType::ReviewSubmitted => review_submitted(ctx, ev, s, &state),
            EventType::CiStatusChanged => ci_changed(ctx, ev, s, &state),
            EventType::CommentAdded | EventType::Mentioned => {
                let c = ev.comment_like().expect("comment-like");
                // An explicit @mention by a writer resumes a Failed item like the UI retry.
                if c.mention
                    && matches!(s.phase, CrPhase::Failed | CrPhase::ConflictParked)
                    && ctx.can_write()
                {
                    return retry(ctx, ev, s, &state);
                }
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
    let mut d = data_from_item(ctx, item);
    if d.base_ref.is_empty() {
        d.base_ref = ctx.default_branch.clone();
    }
    d.carry_stack(&s.data);
    begin(ctx, ev, CrPhase::Reviewing, d, "reopened")
}

/// A merge-readiness check found the change request unmergeable against `base_sha`.
fn unmergeable(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &CrState,
    state: &WorkflowState,
    base_sha: &str,
) -> Transition {
    use CrPhase::*;
    if !s.data.author.eq_ignore_ascii_case(&ctx.bot_login) {
        return Transition::reject(state, "not a change request opened by enzarb");
    }
    if base_sha.is_empty() {
        return Transition::reject(state, "base commit unknown");
    }
    match s.phase {
        AwaitingChanges | ReadyToMerge | ConflictParked => {
            // The cap lives in state: job idempotency keys derive from the event id.
            if s.data.resolve_attempts_for(base_sha) >= MAX_RESOLVE_ATTEMPTS {
                return Transition::reject(state, "already attempted for this base commit");
            }
            let mut d = s.data.clone();
            d.base_sha = base_sha.to_string();
            d.record_resolve_attempt(base_sha);
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            d.trigger_text.clear();
            begin(ctx, ev, ResolvingConflicts, d, "unmergeable")
        }
        ResolvingConflicts | AddressingComments | Reviewing => {
            // Remember the newer base; a later readiness check decides on a new attempt.
            if s.data.base_sha == base_sha {
                return Transition::reject(state, "base commit already recorded");
            }
            let mut d = s.data.clone();
            d.base_sha = base_sha.to_string();
            Transition::to(
                st(s.phase, d),
                Vec::new(),
                "base commit recorded during a run",
            )
        }
        Merging => Transition::reject(state, "busy: a merge is running"),
        Merged | Closed | Failed | Cancelled => Transition::reject(state, "no longer active"),
    }
}

fn updated(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    use CrPhase::*;
    let item = &ev.connector().expect("connector").data.item;
    let sha = item.head_sha.clone().unwrap_or_default();
    match s.phase {
        AwaitingChanges | ReadyToMerge | Reviewing | ConflictParked => {
            let under_review = s.phase == Reviewing && sha == s.data.head_sha;
            if (sha == s.data.reviewed_sha || under_review) && !sha.is_empty() {
                return Transition::reject(state, "duplicate: this commit was already reviewed");
            }
            let mut d = data_from_item(ctx, item);
            d.base_ref = if d.base_ref.is_empty() {
                s.data.base_ref.clone()
            } else {
                d.base_ref
            };
            d.base_sha = s.data.base_sha.clone();
            d.carry_stack(&s.data);
            d.resolve_attempts = s.data.resolve_attempts.clone();
            d.resolve_attempted_base_sha = s.data.resolve_attempted_base_sha.clone();
            d.pending = s.data.pending.clone();
            if s.phase == Reviewing {
                // The branch update's own commit: the merge request stays.
                d.merge_requested = s.data.merge_requested;
                d.merge_actor = s.data.merge_actor.clone();
            }
            // A review already running is for an older commit: restart it.
            let mut t = begin(ctx, ev, Reviewing, d, "new commits");
            if s.phase == Reviewing {
                t.effects.insert(0, super::Effect::CancelJobs);
            }
            t
        }
        AddressingComments | ResolvingConflicts => {
            // Most likely the agent's own push; it is reviewed when the job finishes.
            let mut d = s.data.clone();
            set_head(&mut d, sha);
            Transition::to(st(s.phase, d), Vec::new(), "new commits during a run")
        }
        Merging => {
            // The merge is pinned to the head it was requested for; if this is a different
            // commit the merge reports `head_moved` and the new head is reviewed.
            if sha.is_empty() || sha == s.data.head_sha {
                return Transition::reject(state, "duplicate: this commit is already the head");
            }
            let mut d = s.data.clone();
            set_head(&mut d, sha);
            Transition::to(st(Merging, d), Vec::new(), "new commits during a merge")
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
    if rs == "approved" {
        return human_approved(ctx, ev, s, state);
    }
    if rs == "dismissed" {
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

/// A human `approved` review counts as the review check for the current head. Only an
/// idle `AwaitingChanges` records it; while enzarb has a round in flight (or the item is
/// past review) the event is acknowledged without touching the state. The transition
/// rewrites `state_data.wait` (`ci_pending` while CI is not green).
fn human_approved(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    if !ctx.can_write() {
        return Transition::reject(state, "approval requires write access");
    }
    match s.phase {
        CrPhase::AwaitingChanges => {
            let mut d = s.data.clone();
            d.last_review = "approve".into();
            d.reviewed_sha = d.head_sha.clone();
            human_acted(&mut d);
            evaluate(
                Fx::new(ctx, ev),
                CrPhase::AwaitingChanges,
                d,
                "review approved",
                true,
            )
        }
        _ => Transition::ack(state, "approval ignored in this phase"),
    }
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
    if enzarb_connector::command::has_approve_command(text) {
        return Transition::reject(state, "approve command is not feedback");
    }
    if !ctx.can_write() {
        return Transition::reject(state, "addressing feedback requires write access");
    }
    match s.phase {
        AwaitingChanges | ReadyToMerge => {
            let mut d = s.data.clone();
            d.trigger_comment_id = comment.map(|c| c.0);
            d.trigger_comment_kind = comment.map(|c| c.1).unwrap_or_default();
            d.trigger_text = text.to_string();
            human_acted(&mut d);
            begin(ctx, ev, AddressingComments, d, "feedback received")
        }
        AddressingComments | ResolvingConflicts | Reviewing | Merging => {
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
            Transition::to(st(s.phase, d), fx.finish(), "queued for the next round")
        }
        ConflictParked => Transition::reject(
            state,
            "parked on a merge conflict: mention enzarb to retry or resolve it by hand",
        ),
        Merged | Closed | Failed | Cancelled => Transition::reject(state, "no longer active"),
    }
}

fn retry(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    if !ctx.can_write() {
        return Transition::reject(state, "retry requires write access");
    }
    if !matches!(s.phase, CrPhase::Failed | CrPhase::ConflictParked) {
        return Transition::reject(state, "nothing to retry");
    }
    let mut d = s.data.clone();
    d.error.clear();
    let phase = if s.phase == CrPhase::ConflictParked {
        CrPhase::ResolvingConflicts
    } else {
        d.failed_phase.take().unwrap_or(CrPhase::Reviewing)
    };
    match phase {
        CrPhase::ResolvingConflicts => {
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            begin(ctx, ev, CrPhase::ResolvingConflicts, d, "retry")
        }
        CrPhase::AddressingComments => {
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            begin(ctx, ev, CrPhase::AddressingComments, d, "retry")
        }
        CrPhase::Merging => {
            // A retry never merges: look at the readiness again and let the user decide.
            d.ci = CiState::Unknown;
            d.mergeable = None;
            evaluate(Fx::new(ctx, ev), CrPhase::Merging, d, "retry", true)
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
        CrPhase::ResolvingConflicts => Some(JobKind::AgentResolveConflicts),
        CrPhase::Merging => Some(JobKind::MergeChangeRequest),
        _ => None,
    }
}

/// A `CiStatusChanged` for one check or status of the head. A failure or a running check
/// is taken as is; a success says nothing about the other checks, so the aggregate is
/// asked for.
fn ci_changed(ctx: &Ctx, ev: &EngineEvent, s: &CrState, state: &WorkflowState) -> Transition {
    use CrPhase::*;
    let Some(ci) = &ev.connector().expect("connector").data.ci else {
        return Transition::reject(state, "ci data missing");
    };
    match s.phase {
        Merged | Closed | Failed | Cancelled => {
            return Transition::reject(state, "no longer active");
        }
        Merging => return Transition::reject(state, "busy: a merge is running"),
        Reviewing | AwaitingChanges | ReadyToMerge | AddressingComments | ResolvingConflicts
        | ConflictParked => {}
    }
    if ci.head_sha.is_empty() || ci.head_sha != s.data.head_sha {
        return Transition::reject(state, "ci result is for another commit");
    }
    let mut d = s.data.clone();
    match ci.state {
        enzarb_connector::CiState::Failure => {
            d.ci = CiState::Failure;
            d.merge_requested = false;
        }
        enzarb_connector::CiState::Pending => d.ci = CiState::Pending,
        enzarb_connector::CiState::Success => {
            if !(d.ci == CiState::Success && d.ci_sha == ci.head_sha) {
                d.ci = CiState::Unknown;
            }
        }
    }
    d.ci_sha = ci.head_sha.clone();
    match s.phase {
        AwaitingChanges | ReadyToMerge => {
            evaluate(Fx::new(ctx, ev), s.phase, d, "ci status changed", true)
        }
        _ => Transition::to(st(s.phase, d), Vec::new(), "ci status recorded"),
    }
}

/// The result of `check_merge_readiness`: applied only to the head it was asked about.
fn readiness_checked(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &CrState,
    state: &WorkflowState,
    output: &Value,
) -> Transition {
    use CrPhase::*;
    if matches!(s.phase, Merging | Merged | Closed | Failed | Cancelled) {
        return Transition::ack(state, "readiness check ignored");
    }
    let sha = output.get("head_sha").and_then(Value::as_str).unwrap_or("");
    if sha.is_empty() || sha != s.data.head_sha {
        return Transition::ack(state, "stale readiness check");
    }
    let mut d = s.data.clone();
    d.ci = CiState::from_wire(output.get("ci").and_then(Value::as_str).unwrap_or(""));
    d.ci_sha = sha.to_string();
    d.mergeable = output.get("mergeable").and_then(Value::as_bool);
    match s.phase {
        // The answer never asks again: still-unknown readiness holds until the next event.
        AwaitingChanges | ReadyToMerge => {
            evaluate(Fx::new(ctx, ev), s.phase, d, "readiness checked", false)
        }
        _ => Transition::to(st(s.phase, d), Vec::new(), "readiness recorded"),
    }
}

/// The UI `merge` action: the only way into `Merging`.
fn merge(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &CrState,
    state: &WorkflowState,
    actor: &str,
) -> Transition {
    if !ctx.can_write() {
        return Transition::reject(state, "merge requires write access");
    }
    if s.phase != CrPhase::ReadyToMerge || !readiness(&s.data).all_ok() {
        return Transition::reject(state, "not ready to merge");
    }
    let mut d = s.data.clone();
    d.merge_requested = true;
    d.merge_actor = actor.to_string();
    let title = squash_title(&d.title, &d.head_ref);
    let mut fx = Fx::new(ctx, ev);
    fx.merge_change_request(d.number, &title, &d.head_sha, actor);
    Transition::to(st(CrPhase::Merging, d), fx.finish(), "merge requested")
}

fn merge_finished(ctx: &Ctx, ev: &EngineEvent, s: &CrState, output: &Value) -> Transition {
    let text = |k: &str| output.get(k).and_then(Value::as_str).unwrap_or("");
    let mut d = s.data.clone();
    match text("outcome") {
        "merged" | "already_merged" => {
            let mut fx = Fx::new(ctx, ev);
            fx.wake_at(None);
            let actor = text("actor");
            fx.comment(if actor.is_empty() {
                "enzarb: merged.".to_string()
            } else {
                format!("enzarb: merged by @{actor}.")
            });
            fx.done(&target(ctx, &d));
            d.merge_error.clear();
            // Nothing left to address once merged.
            d.pending = None;
            d.merge_requested = false;
            Transition::to(st(CrPhase::Merged, d), fx.finish(), "merged")
        }
        "head_moved" => {
            d.merge_error = "the branch changed while merging; reviewing the new commits".into();
            d.merge_requested = false;
            d.last_review.clear();
            d.ci = CiState::Unknown;
            d.mergeable = None;
            if let Some(p) = d.pending.take() {
                // The human's request goes first; the new head is reviewed after the round.
                let mut fx = Fx::new(ctx, ev);
                fx.received(&ctx.progress_target(p.comment_id, p.comment_kind));
                fx.wake_at(None);
                return start_pending_round(ctx, fx, d, p);
            }
            begin(ctx, ev, CrPhase::Reviewing, d, "head moved during merge")
        }
        "updated_branch" => {
            // Behind its base: the base was merged into the branch instead of merging. The
            // new head is reviewed, CI reruns, and the user merges once it is ready again.
            d.merge_error = "updating branch with the base; waiting for review and CI".into();
            d.last_review.clear();
            d.ci = CiState::Unknown;
            d.mergeable = None;
            begin(
                ctx,
                ev,
                CrPhase::Reviewing,
                d,
                "branch updated before merge",
            )
        }
        "not_mergeable" => {
            let reason = text("reason");
            d.merge_error = if reason.is_empty() {
                "the change request cannot be merged".into()
            } else {
                truncate(reason, 300)
            };
            d.mergeable = Some(false);
            d.merge_requested = false;
            if let Some(p) = d.pending.take() {
                let mut fx = Fx::new(ctx, ev);
                fx.wake_at(None);
                return start_pending_round(ctx, fx, d, p);
            }
            Transition::to(st(CrPhase::AwaitingChanges, d), Vec::new(), "not mergeable")
        }
        other => fail(
            ctx,
            ev,
            s,
            &format!("unusable merge result {other:?}"),
            false,
        ),
    }
}

/// Give up retargeting: nothing was pushed that could be wrong, but a person has to move the
/// branch. The stack is forgotten so the engine does not try again by itself.
fn retarget_park(ctx: &Ctx, ev: &EngineEvent, s: &CrState, reason: &str) -> Transition {
    let mut d = s.data.clone();
    let onto = if d.default_base.is_empty() {
        ctx.default_branch.clone()
    } else {
        d.default_base.clone()
    };
    d.clear_stack();
    d.merge_requested = false;
    d.merge_error = truncate(reason, 300);
    let mut fx = Fx::new(ctx, ev);
    if matches!(
        s.phase,
        CrPhase::Reviewing | CrPhase::AddressingComments | CrPhase::ResolvingConflicts
    ) {
        fx.cancel_jobs();
    }
    fx.wake_at(None);
    fx.comment(format!(
        "enzarb did not retarget this change request onto `{onto}` after its parent was \
         merged: {}\n\nNothing was merged and nothing was force-pushed. A maintainer needs to \
         rebase this branch onto `{onto}` and change its base.",
        truncate(reason, 300)
    ));
    Transition::to(
        st(CrPhase::ConflictParked, d),
        fx.finish(),
        "retarget parked",
    )
}

/// The result of `retarget_change_request`. Applies in whatever phase the item is in: the
/// push can start a review of the new head before the result arrives.
fn retarget_finished(ctx: &Ctx, ev: &EngineEvent, s: &CrState, output: &Value) -> Transition {
    let state = WorkflowState::ChangeRequest(s.clone());
    if !s.data.retargeting {
        return Transition::reject(&state, "stale job result");
    }
    let onto = s.data.default_base.clone();
    let mut d = s.data.clone();
    d.retargeting = false;
    match RetargetOutcome::from_output(output) {
        RetargetOutcome::Retargeted { head_sha } => {
            d.clear_stack();
            d.base_ref = onto.clone();
            if !head_sha.is_empty() {
                set_head(&mut d, head_sha);
            }
            // The rebased head is a new commit: review it, then the usual checks decide.
            d.last_review.clear();
            d.ci = CiState::Unknown;
            d.mergeable = None;
            d.merge_error.clear();
            let note = format!("enzarb: retargeted onto `{onto}` after the parent was merged.");
            let mut fx = Fx::new(ctx, ev);
            fx.comment(note);
            let mut t = if s.phase == CrPhase::Reviewing {
                Transition::to(st(CrPhase::Reviewing, d), Vec::new(), "retargeted")
            } else {
                begin(ctx, ev, CrPhase::Reviewing, d, "retargeted")
            };
            t.effects.splice(0..0, fx.finish());
            t
        }
        RetargetOutcome::Already => {
            d.clear_stack();
            d.base_ref = onto;
            match s.phase {
                CrPhase::AwaitingChanges | CrPhase::ReadyToMerge => {
                    evaluate(Fx::new(ctx, ev), s.phase, d, "already retargeted", true)
                }
                phase => Transition::to(st(phase, d), Vec::new(), "already retargeted"),
            }
        }
        RetargetOutcome::BaseNotSet { head_sha, reason } => {
            let mut p = s.clone();
            if !head_sha.is_empty() {
                set_head(&mut d, head_sha);
            }
            p.data = d;
            retarget_park(
                ctx,
                ev,
                &p,
                &format!("the branch was rebased but the base could not be changed ({reason})"),
            )
        }
        RetargetOutcome::LeaseFailed { reason } => {
            // The branch moved under the lease: review the new commits; the retarget runs
            // again from the head the review ends on (up to the attempt cap).
            let mut p = s.clone();
            p.data = d.clone();
            if d.retarget_attempts >= MAX_RETARGET_ATTEMPTS {
                return retarget_park(ctx, ev, &p, &format!("the branch kept changing ({reason})"));
            }
            d.last_review.clear();
            d.ci = CiState::Unknown;
            d.mergeable = None;
            d.merge_requested = false;
            begin(ctx, ev, CrPhase::Reviewing, d, "head moved during retarget")
        }
        RetargetOutcome::Conflict { reason } => {
            let mut p = s.clone();
            p.data = d;
            retarget_park(
                ctx,
                ev,
                &p,
                &format!("the rebase onto `{onto}` conflicts ({reason})"),
            )
        }
        RetargetOutcome::Refused { reason } => {
            let mut p = s.clone();
            p.data = d;
            retarget_park(ctx, ev, &p, &reason)
        }
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
    if kind == JobKind::CheckMergeReadiness {
        return readiness_checked(ctx, ev, s, state, output);
    }
    if kind == JobKind::RetargetChangeRequest {
        return retarget_finished(ctx, ev, s, output);
    }
    if awaited(s.phase) != Some(kind) {
        return Transition::reject(state, "stale job result");
    }
    if kind == JobKind::MergeChangeRequest {
        return merge_finished(ctx, ev, s, output);
    }
    let (mut decision, mut out) = match AgentOutput::from_job_output(kind, output) {
        Ok(x) => x,
        Err(e) => return fail(ctx, ev, s, &format!("unusable agent output: {e}"), false),
    };
    // An approval needs verified checks; otherwise it is only a comment.
    let mut downgraded = false;
    if kind == JobKind::AgentReview && decision == Decision::Approve && !out.checks_verified() {
        decision = Decision::Comment;
        downgraded = true;
        out.body.push_str(
            "\n\n> Downgraded from `approve` to `comment`: the review did not report all checks as passed, so the change is unverified.",
        );
    }
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
            d.auto_capped = false;
            d.auto_trigger = false;
            // A human's request that arrived during the review comes first.
            if let Some(p) = d.pending.take() {
                return start_pending_round(ctx, fx, d, p);
            }
            // Only must-fix findings: a plain comment is advisory and waits for a person.
            if decision == Decision::RequestChanges || downgraded {
                if d.auto_rounds < ctx.config.max_auto_rounds {
                    d.auto_rounds += 1;
                    d.auto_trigger = true;
                    d.trigger_text = out.body.clone();
                    start_address(ctx, &mut fx, &d);
                    return Transition::to(
                        st(AddressingComments, d),
                        fx.finish(),
                        "review findings: automatic round",
                    );
                }
                d.auto_capped = ctx.config.max_auto_rounds > 0;
            }
            evaluate(fx, Reviewing, d, "review posted", true)
        }
        (AddressingComments, Decision::Done) => {
            fx.comment(out.body);
            fx.done(&tgt);
            // Record the pushed head; `reviewed_sha` only moves when a review is posted.
            if let Some(sha) = output
                .get("head_sha")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                set_head(&mut d, sha.to_string());
            }
            let pushed = d.head_sha != d.reviewed_sha;
            d.auto_trigger = false;
            match d.pending.take() {
                Some(p) => start_pending_round(ctx, fx, d, p),
                None => {
                    d.trigger_comment_id = None;
                    d.trigger_comment_kind = CommentKind::default();
                    d.trigger_text.clear();
                    if pushed {
                        // The agent's fixes are reviewed like any other new head.
                        start_review(ctx, &mut fx, &d);
                        Transition::to(st(Reviewing, d), fx.finish(), "fixes pushed")
                    } else {
                        evaluate(fx, AddressingComments, d, "comments addressed", true)
                    }
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
        (ResolvingConflicts, Decision::Done) => {
            fx.comment(out.body);
            fx.done(&tgt);
            if let Some(sha) = output
                .get("head_sha")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                d.head_sha = sha.to_string();
            }
            d.trigger_comment_id = None;
            d.trigger_comment_kind = CommentKind::default();
            d.trigger_text.clear();
            match d.pending.take() {
                // Feedback queued during the merge; the merged head is reviewed afterwards.
                Some(p) => start_pending_round(ctx, fx, d, p),
                // `reviewed_sha` is left alone: the merge commit is reviewed like any new head.
                None if d.head_sha != d.reviewed_sha => {
                    start_review(ctx, &mut fx, &d);
                    Transition::to(st(Reviewing, d), fx.finish(), "conflicts resolved")
                }
                None => Transition::to(st(AwaitingChanges, d), fx.finish(), "nothing to merge"),
            }
        }
        (ResolvingConflicts, Decision::Blocked) => {
            let base_sha = d.base_sha.clone();
            if !base_sha.is_empty() && d.resolve_attempts_for(&base_sha) < MAX_RESOLVE_ATTEMPTS {
                // Second try against the same base head before a person is asked.
                fx.done(&tgt);
                d.record_resolve_attempt(&base_sha);
                start_resolve(ctx, &mut fx, &d);
                return Transition::to(
                    st(ResolvingConflicts, d),
                    fx.finish(),
                    "conflict resolution retried",
                );
            }
            fx.comment(format!(
                "enzarb could not resolve the merge conflict with `{}` on its own and left this change request for a person.\n\n{}\n\nMention enzarb to try again after the conflict is resolved or the base moves on.",
                d.base_ref, out.body
            ));
            fx.done(&tgt);
            d.pending = None;
            Transition::to(st(ConflictParked, d), fx.finish(), "conflict parked")
        }
        _ => unreachable!("decision validated against the job kind"),
    }
}
