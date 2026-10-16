//! `graph:change_request` (ADR-0020): what the kernel does for a change request graph.
//!
//! The change request's facts are the compiled machine's [`CrData`] (head, review, CI,
//! mergeability, merge request, ...), kept in `state_data.cr.data` and exposed to conditions
//! as item facts. Events resume the `wait` node that lists them:
//!
//! * `ci`: CI on the head became `success` or `failure` (or mergeability became known while
//!   CI is green), once per head and result: `{event, status, failing, head_sha, mergeable}`.
//! * `review`: a human GitHub review: `{event, author, state, body, can_write}`.
//! * `pushed`: a new head the running job did not push: `{event, head_sha, author}`.
//! * `merge_requested`: the UI merge (write access, no open dependency):
//!   `{event, author, delete_branch}`. The click is stored, like the compiled machine's
//!   `merge_requested`, so a later wait on `merge_requested` resumes without a second click
//!   until a push by a person, a CI failure, feedback or a refused merge clears it.
//! * `conflict`: `MergeConflictDetected {base_sha}` for a change request enzarb opened, at most
//!   once per base commit: `{event, base_sha}`.
//! * `comment` and `approval` as for issues.
//!
//! An event that arrives while a job runs is recorded and resumes the next wait that lists it
//! (feedback first, then a push, a stored merge click, a conflict, CI), so nothing waits on an
//! event that already happened. Closing the change request ends the task (`Merged`/`Closed`).

use enzarb_connector::{CommentKind, EventType, ItemKind};
use enzarb_graph::{Definition, NodeKind, Prompt, WaitFrom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::change_request::{self as crm, CiState, CrData};
use super::super::fx::Fx;
use super::super::prompt::{AgentOutput, Decision};
use super::super::title::squash_title;
use super::super::{Ctx, EngineEvent, EventPayload, JobKind, Transition, WorkflowState};
use super::{
    CR_TRIGGER, GraphData, GraphPhase, GraphPin, GraphSource, GraphTask, advance, advance_with,
    enter_start, failed, load_for, rejected_definition, st, target, wait_json,
};

/// What led to the next run, so its trigger and round count are right.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    #[default]
    None,
    /// enzarb's own review had must-fix findings (`trigger_text` is its body).
    Review,
    /// CI failed on the head.
    Ci,
    /// A person's comment or review (`trigger_text`).
    Human,
}

/// Feedback that arrived while a job ran; it resumes the next wait on its event.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Queued {
    /// `comment` or `review`.
    pub event: String,
    pub output: Value,
    pub text: String,
    pub comment_id: Option<u64>,
    pub comment_kind: CommentKind,
}

/// `state_data.cr` of a change request graph.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CrGraph {
    /// The compiled machine's facts about the change request.
    pub data: CrData,
    /// Must-fix findings of the latest review (`request_changes`: 1; a downgraded approve:
    /// the checks it could not verify).
    pub must_fix: u32,
    /// The CI result the task last resumed on (or acted on), `<sha>:<status>[:<mergeable>]`.
    pub ci_seen: String,
    /// The base commit the last `conflict` resumed for.
    pub conflict_base: String,
    /// A conflict was reported while no wait took it.
    pub conflict_pending: bool,
    /// A person pushed a new head that no wait has taken yet.
    pub push_pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<Queued>,
    pub trigger: Trigger,
    /// The head the latest segment ended at (what its review saw).
    pub review_head: String,
    /// The head the stored merge click was given for; the click counts only at this head.
    pub merge_head: String,
    /// The next head change is the base merged in by the merge job's `updated_branch` (or an
    /// `update_branch` action): the click moves to that head. Any other head change drops it.
    pub update_allowed: bool,
    /// The head when the running segment started.
    pub run_head: String,
    /// `post_review`: its output, given to the graph once the comment is posted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_output: Option<Value>,
}

fn ci_wire(d: &CrData) -> &'static str {
    if d.ci_sha.is_empty() || d.ci_sha != d.head_sha {
        return "unknown";
    }
    match d.ci {
        CiState::Success => "success",
        CiState::Failure => "failure",
        CiState::Pending => "pending",
        CiState::Unknown => "unknown",
    }
}

/// enzarb's latest review covers the current head.
fn review_current(d: &CrData) -> bool {
    !d.last_review.is_empty() && !d.head_sha.is_empty() && d.reviewed_sha == d.head_sha
}

fn ci_fix_due(d: &CrData) -> bool {
    ci_wire(d) == "failure"
        && review_current(d)
        && d.ci_fix_sha != d.head_sha
        && !crm::ci_failed_external(d)
}

/// The item facts conditions read (`_item`).
pub(super) fn facts(ctx: &Ctx, c: &CrGraph) -> Value {
    let d = &c.data;
    let current = review_current(d);
    json!({
        "review": if current { d.last_review.as_str() } else { "" },
        "must_fix": if current { c.must_fix } else { 0 },
        "auto_rounds": d.auto_rounds,
        "ci": ci_wire(d),
        "ci_fix_due": ci_fix_due(d),
        "mergeable": d.mergeable,
        "head_sha": d.head_sha,
        "stacked": !d.stacked_on.is_empty(),
        "dependencies_open": !ctx.open_dependencies.is_empty(),
    })
}

/// The dedupe key of a CI resume: a failure once per head, a success once per mergeability.
fn ci_key(d: &CrData) -> String {
    match ci_wire(d) {
        // Once more when an Actions job joins external-only failures (it becomes fixable).
        "failure" if crm::ci_failed_external(d) => format!("{}:failure:external", d.head_sha),
        "failure" => format!("{}:failure", d.head_sha),
        s => format!("{}:{s}:{:?}", d.head_sha, d.mergeable),
    }
}

fn ci_output(d: &CrData) -> Value {
    let current = d.ci_failures_sha == d.head_sha;
    // Only GitHub Actions jobs are fixable (the agent reads their log); the rest is `external`.
    let failing: Vec<Value> = if current {
        d.ci_failures
            .iter()
            .filter(|f| crm::actionable(f))
            .map(|f| {
                let ids = crm::actions_ids(&f.url);
                json!({
                    "name": f.name,
                    "url": f.url,
                    "run_id": ids.as_ref().map(|i| i.0.clone()),
                    "job_id": ids.and_then(|i| i.1),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let external: Vec<&str> = if current {
        d.ci_failures
            .iter()
            .filter(|f| !crm::actionable(f))
            .map(|f| f.name.as_str())
            .collect()
    } else {
        Vec::new()
    };
    json!({
        "event": "ci",
        "status": ci_wire(d),
        "failing": failing,
        "actionable": !crm::ci_failed_external(d),
        "external": external,
        "head_sha": d.head_sha,
        "mergeable": d.mergeable,
    })
}

/// The CI resume due now, if any: a decided result not acted on yet. Marks it seen.
fn take_ci(c: &mut CrGraph) -> Option<Value> {
    let status = ci_wire(&c.data);
    if !matches!(status, "success" | "failure") || ci_key(&c.data) == c.ci_seen {
        return None;
    }
    // Green but known not to merge: nothing new for a wait on CI (a conflict comes as one).
    if status == "success" && c.data.mergeable == Some(false) {
        return None;
    }
    c.ci_seen = ci_key(&c.data);
    c.trigger = if status == "failure" {
        Trigger::Ci
    } else {
        Trigger::None
    };
    Some(ci_output(&c.data))
}

fn mark_ci_seen(c: &mut CrGraph) {
    if matches!(ci_wire(&c.data), "success" | "failure") {
        c.ci_seen = ci_key(&c.data);
    }
}

fn wait_events<'a>(def: &'a Definition, s: &GraphTask) -> Option<(&'a [String], WaitFrom)> {
    match def.node(&s.data.graph.node).map(|n| &n.kind) {
        Some(NodeKind::Wait { events, from, .. }) if s.phase == GraphPhase::Waiting => {
            Some((events.as_slice(), *from))
        }
        _ => None,
    }
}

fn at_wait(def: &Definition, s: &GraphTask, event: &str) -> bool {
    wait_events(def, s).is_some_and(|(e, _)| e.iter().any(|x| x == event))
}

fn crg(t: &mut GraphTask) -> &mut CrGraph {
    t.data.cr.as_mut().expect("a change request graph")
}

/// Records a new head. A stored merge click survives only the base update the merge job made
/// (`update_allowed`); any other new commit needs a new click.
fn set_head(c: &mut CrGraph, sha: String) {
    if c.data.head_sha != sha {
        if c.update_allowed && c.data.merge_requested {
            c.merge_head = sha.clone();
        } else {
            clear_click(c);
        }
        c.update_allowed = false;
    }
    crm::set_head(&mut c.data, sha);
}

fn clear_click(c: &mut CrGraph) {
    c.data.merge_requested = false;
    c.merge_head.clear();
    c.update_allowed = false;
}

/// The base was merged into the branch by enzarb: the click moves to the new head. When its
/// webhook already moved the head during the job (`update_allowed` was provisional), the
/// click is already on it.
fn allow_update(c: &mut CrGraph) {
    // `run_head` is the head the job started at: unchanged means the update commit is next.
    c.update_allowed = c.data.merge_requested && c.data.head_sha == c.run_head;
}

/// A stored click for the current head.
fn click_valid(c: &CrGraph) -> bool {
    c.data.merge_requested && !c.data.head_sha.is_empty() && c.merge_head == c.data.head_sha
}

fn clear_trigger(d: &mut CrData) {
    d.trigger_text.clear();
    d.trigger_comment_id = None;
    d.trigger_comment_kind = CommentKind::default();
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

/// Starts the change request graph when [`Ctx::graph`] holds a usable one for this trigger,
/// else `legacy`. A stacked change request stays on the compiled machine: retargeting is
/// compiled-only.
pub(super) fn start_or(
    ctx: &Ctx,
    state: &WorkflowState,
    ev: &EngineEvent,
    legacy: fn(&Ctx, &WorkflowState, &EngineEvent) -> Transition,
) -> Transition {
    let Some(src) = &ctx.graph else {
        return legacy(ctx, state, ev);
    };
    let Some(e) = ev.connector().filter(|e| {
        e.event_type == EventType::ChangeRequestOpened
            && e.data.item.kind == ItemKind::ChangeRequest
    }) else {
        return legacy(ctx, state, ev);
    };
    let mut d = crm::data_from_item(ctx, &e.data.item);
    if d.base_ref.is_empty() {
        d.base_ref = ctx.default_branch.clone();
    }
    crm::record_stack(ctx, &mut d);
    if !d.stacked_on.is_empty() {
        let mut t = legacy(ctx, state, ev);
        if t.accepted {
            t.note = format!("{}; stacked, so on the built-in workflow", t.note);
        }
        return t;
    }
    match load_for(src, true) {
        Ok(def) if def.on.iter().any(|t| t == CR_TRIGGER) => start(ctx, ev, src, &def, d),
        Err(err) if !super::not_for(src, CR_TRIGGER) => {
            rejected_definition(ctx, ev, src, &err, legacy(ctx, state, ev))
        }
        _ => legacy(ctx, state, ev),
    }
}

fn start(
    ctx: &Ctx,
    ev: &EngineEvent,
    src: &GraphSource,
    def: &Definition,
    mut data: CrData,
) -> Transition {
    // Nothing is reviewed yet (the compiled machine uses `reviewed_sha` for duplicates).
    data.reviewed_sha.clear();
    let mut d = GraphData {
        graph: GraphPin {
            name: def.name.clone(),
            path: src.path.clone(),
            sha: src.sha.clone(),
            source: src.source.clone(),
            prompts: src.prompts.clone(),
            ..Default::default()
        },
        number: data.number,
        url: data.url.clone(),
        title: data.title.clone(),
        body: data.body.clone(),
        author: data.author.clone(),
        cr: Some(CrGraph {
            data,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut fx = Fx::new(ctx, ev);
    fx.received(&target(ctx));
    let phase = enter_start(ctx, &mut fx, def, &mut d);
    Transition::to(
        st(phase, d),
        fx.finish(),
        format!("graph `{}` started", def.name),
    )
}

/// A reopened change request: its facts from the event's item, the rest kept.
pub(super) fn reopened(ctx: &Ctx, ev: &EngineEvent, d: &mut GraphData) {
    let Some(e) = ev.connector() else { return };
    let Some(c) = &mut d.cr else { return };
    let mut fresh = crm::data_from_item(ctx, &e.data.item);
    if fresh.base_ref.is_empty() {
        fresh.base_ref = c.data.base_ref.clone();
    }
    fresh.reviewed_sha.clear();
    *c = CrGraph {
        data: fresh,
        ..Default::default()
    };
}

/// `WorkItemClosed`: the task ends `Merged` or `Closed`, whatever node it is at.
pub(super) fn closed(ctx: &Ctx, ev: &EngineEvent, s: &GraphTask, merged: bool) -> Transition {
    let mut d = s.data.clone();
    d.waiting_on = None;
    d.wait = None;
    if let Some(c) = &mut d.cr {
        c.data.merge_requested = false;
        c.queued = None;
    }
    let mut fx = Fx::new(ctx, ev);
    fx.cancel_jobs();
    fx.wake_at(None);
    let phase = if merged {
        GraphPhase::Merged
    } else {
        GraphPhase::Closed
    };
    Transition::to(st(phase, d), fx.finish(), "finished")
}

// ---------------------------------------------------------------------------
// Runs and kernel nodes
// ---------------------------------------------------------------------------

/// The named prompt of an agent node, if it is one.
fn prompt_name<'a>(def: &'a Definition, id: &str) -> &'a str {
    match def.node(id).map(|n| &n.kind) {
        Some(NodeKind::Agent {
            prompt: Prompt::Text(t),
        }) => t.trim(),
        _ => "",
    }
}

/// Before a segment from `id` is queued: write the run's trigger (a person's feedback, the
/// review's findings or the failing checks), count an automatic round, and say why the item
/// waits on enzarb. Returns `state_data.wait`.
pub(super) fn prepare_run(c: &mut CrGraph, def: &Definition, id: &str) -> Value {
    let prompt = prompt_name(def, id);
    c.run_head = c.data.head_sha.clone();
    let trigger = std::mem::take(&mut c.trigger);
    let d = &mut c.data;
    if prompt == "address_comments" {
        d.merge_requested = false;
        c.merge_head.clear();
        d.stats.fix_rounds += 1;
        match trigger {
            Trigger::Human => d.auto_trigger = false,
            Trigger::Review => {
                d.auto_rounds += 1;
                d.auto_trigger = true;
            }
            _ if ci_fix_due(d) => {
                d.auto_rounds += 1;
                crm::begin_ci_fix(d);
                c.ci_seen = ci_key(&c.data);
            }
            _ => {
                d.auto_trigger = false;
                clear_trigger(d);
            }
        }
    } else {
        d.auto_trigger = false;
        clear_trigger(d);
    }
    if prompt == "review" {
        c.push_pending = false;
    }
    let d = &c.data;
    match prompt {
        "review" if d.merge_requested => wait_json(
            "enzarb",
            "updating_branch",
            "Branch updated with the base; reviewing before merging",
        ),
        "review" => wait_json("enzarb", "reviewing", "enzarb is reviewing"),
        "address_comments" if d.auto_trigger => wait_json(
            "enzarb",
            "enzarb_fixing",
            "enzarb is fixing its own review findings",
        ),
        "address_comments" => wait_json(
            "enzarb",
            "addressing_comments",
            "enzarb is addressing review comments",
        ),
        "resolve_conflicts" => wait_json(
            "enzarb",
            "resolving_conflict",
            "enzarb is resolving a merge conflict",
        ),
        _ => wait_json("enzarb", "working", "enzarb is working on this"),
    }
}

/// A segment finished. Its review saw the head it started at; the sandbox's head is taken
/// only when the segment pushed and nobody moved the head during the run (a person's push
/// recorded meanwhile wins and is reviewed next).
pub(super) fn segment_finished(c: &mut CrGraph, head_sha: &str, pushed: bool) {
    c.review_head = if c.run_head.is_empty() {
        c.data.head_sha.clone()
    } else {
        c.run_head.clone()
    };
    if pushed && !head_sha.is_empty() && c.data.head_sha == c.run_head {
        set_head(c, head_sha.to_string());
        // What the run pushed is what a review later in the same segment saw.
        c.review_head = head_sha.to_string();
    }
}

/// Entering a wait: what arrived while a job ran and the wait takes, in this order.
pub(super) fn on_enter_wait(
    ctx: &Ctx,
    d: &mut GraphData,
    node: &enzarb_graph::Node,
) -> Option<Value> {
    let NodeKind::Wait { events, .. } = &node.kind else {
        return None;
    };
    let wants = |e: &str| events.iter().any(|x| x == e);
    let c = d.cr.as_mut()?;
    if let Some(q) = c.queued.take_if(|q| wants(&q.event)) {
        c.trigger = Trigger::Human;
        crm::human_acted(&mut c.data);
        c.data.trigger_text = q.text;
        c.data.trigger_comment_id = q.comment_id;
        c.data.trigger_comment_kind = q.comment_kind;
        c.data.merge_requested = false;
        return Some(q.output);
    }
    if wants("pushed") && c.push_pending && c.data.head_sha != c.data.reviewed_sha {
        c.push_pending = false;
        c.trigger = Trigger::None;
        return Some(json!({"event": "pushed", "head_sha": c.data.head_sha, "author": ""}));
    }
    if wants("merge_requested") && c.data.merge_requested {
        if !click_valid(c) || !ctx.open_dependencies.is_empty() {
            // A click for another head, or dependencies left: a person clicks again.
            clear_click(c);
            return None;
        }
        return Some(json!({
            "event": "merge_requested",
            "author": c.data.merge_actor,
            "delete_branch": c.data.merge_delete_branch,
        }));
    }
    if wants("conflict") && c.conflict_pending && c.data.base_sha != c.conflict_base {
        c.conflict_pending = false;
        c.conflict_base = c.data.base_sha.clone();
        c.trigger = Trigger::None;
        return Some(json!({"event": "conflict", "base_sha": c.data.base_sha}));
    }
    if wants("ci") {
        return take_ci(c);
    }
    None
}

/// The change request actions.
pub(super) fn action(
    ctx: &Ctx,
    fx: &mut Fx,
    def: &Definition,
    d: &mut GraphData,
    id: &str,
    action: &str,
) -> GraphPhase {
    if d.cr.is_none() {
        return failed(
            ctx,
            fx,
            d,
            &format!("node `{id}`: `{action}` needs a change request"),
        );
    }
    match action {
        "post_review" => post_review(ctx, fx, d, id),
        "check_merge_readiness" => {
            let c = d.cr.as_ref().expect("change request");
            fx.check_merge_readiness(c.data.number, &c.data.head_sha);
            d.waiting_on = Some(JobKind::CheckMergeReadiness);
            d.wait = Some(wait_json(
                "ci",
                "ci_pending",
                "Checking CI and mergeability",
            ));
            GraphPhase::Running
        }
        "update_branch" => {
            let c = d.cr.as_mut().expect("change request");
            c.run_head = c.data.head_sha.clone();
            fx.update_branch(c.data.number, &c.data.head_sha);
            d.waiting_on = Some(JobKind::UpdateBranch);
            d.wait = Some(wait_json(
                "enzarb",
                "updating_branch",
                "Updating the branch with the base",
            ));
            GraphPhase::Running
        }
        "merge_change_request" => {
            // Validation puts a wait on `merge_requested` before every merge; this is only
            // defence in depth: no stored click for this head, no merge.
            if !click_valid(d.cr.as_ref().expect("change request")) {
                return failed(ctx, fx, d, &format!("node `{id}`: no merge was requested"));
            }
            let c = d.cr.as_mut().expect("change request");
            if !ctx.open_dependencies.is_empty() {
                // Dependencies reopened since the click: no merge; the graph decides as for a
                // refused merge.
                clear_click(c);
                c.data.merge_error = "dependencies are not done".into();
                let out =
                    json!({"outcome": "not_mergeable", "reason": "dependencies are not done"});
                return super::resume(ctx, fx, def, d, out);
            }
            c.run_head = c.data.head_sha.clone();
            let c = &c.data;
            let title = squash_title(&c.title, &c.head_ref);
            let delete = c.merge_delete_branch.then_some(c.head_ref.as_str());
            fx.merge_change_request(c.number, &title, &c.head_sha, &c.merge_actor, delete);
            d.waiting_on = Some(JobKind::MergeChangeRequest);
            d.wait = Some(wait_json("github", "merging", "Merging on GitHub"));
            GraphPhase::Running
        }
        other => failed(
            ctx,
            fx,
            d,
            &format!("action `{other}` is not supported yet"),
        ),
    }
}

/// Posts the latest agent message (the review) as the review comment and records its
/// verdict, as the compiled machine does after `Reviewing`. An approval whose checks were not
/// all verified is downgraded to `comment` with must-fix findings.
fn post_review(ctx: &Ctx, fx: &mut Fx, d: &mut GraphData, id: &str) -> GraphPhase {
    let mut out = match AgentOutput::parse(&d.summary) {
        Ok(o) => o,
        Err(e) => {
            return failed(
                ctx,
                fx,
                d,
                &format!("node `{id}`: the review gave no usable result: {e}"),
            );
        }
    };
    let Some(mut decision) = Decision::allowed(JobKind::AgentReview)
        .iter()
        .copied()
        .find(|x| x.as_str() == out.decision)
    else {
        let why = format!("node `{id}`: `{}` is not a review decision", out.decision);
        return failed(ctx, fx, d, &why);
    };
    let c = d.cr.as_mut().expect("change request");
    let data = &mut c.data;
    let ci_green =
        data.ci == CiState::Success && !data.ci_sha.is_empty() && data.ci_sha == data.head_sha;
    let mut downgraded = false;
    if decision == Decision::Approve && !out.checks_verified(ci_green) {
        decision = Decision::Comment;
        downgraded = true;
        out.body.push_str(
            "\n\n> Downgraded from `approve` to `comment`: the review did not report all checks as passed, so the change is unverified.",
        );
    }
    if decision != Decision::Approve {
        // Mirrors the compiled `evaluate`: requested changes drop a stored merge click.
        clear_click(c);
    }
    let data = &mut c.data;
    data.last_review = decision.as_str().to_string();
    data.reviewed_sha = if c.review_head.is_empty() {
        data.head_sha.clone()
    } else {
        c.review_head.clone()
    };
    data.auto_capped = false;
    data.stats.review_rounds += 1;
    data.prior_review = crm::PriorReview {
        sha: data.reviewed_sha.clone(),
        base_sha: data.base_sha.clone(),
        verdict: decision.as_str().to_string(),
    };
    data.unverified_checks = if decision == Decision::Comment {
        let mut list: Vec<crm::UnverifiedCheck> = out
            .unverified_checks(ci_green)
            .into_iter()
            .map(|x| crm::UnverifiedCheck {
                name: x.name.clone(),
                status: x.status.clone(),
                reason: x.reason.clone(),
            })
            .collect();
        if out.checks.is_empty() {
            list.push(crm::UnverifiedCheck {
                name: "no checks reported".into(),
                status: "not_run".into(),
                reason: "not_attempted".into(),
            });
        }
        list
    } else {
        Vec::new()
    };
    c.must_fix = match decision {
        Decision::RequestChanges => 1,
        _ if downgraded => (data.unverified_checks.len() as u32).max(1),
        _ => 0,
    };
    clear_trigger(data);
    if c.must_fix > 0 {
        data.trigger_text = out.body.clone();
        c.trigger = Trigger::Review;
    } else {
        c.trigger = Trigger::None;
    }
    if data.reviewed_sha == data.head_sha {
        c.push_pending = false;
    }
    mark_ci_seen(c);
    fx.comment(format!(
        "### enzarb review: {}\n\n{}",
        decision.as_str(),
        out.body
    ));
    c.review_output = Some(json!({
        "decision": decision.as_str(),
        "must_fix": c.must_fix,
        "body": out.body,
    }));
    d.waiting_on = Some(JobKind::PostComment);
    d.wait = Some(wait_json(
        "enzarb",
        "reviewing",
        "enzarb is posting its review",
    ));
    GraphPhase::Running
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// The events a change request graph handles itself; `None` leaves the rest (segments,
/// cancel, retry, dependencies, timers, other jobs) to the generic kernel.
pub(super) fn step(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Option<Transition> {
    use super::super::UiActionKind as Ui;
    Some(match &ev.payload {
        EventPayload::Ui(u) => match u.action {
            Ui::Merge => merge(
                ctx,
                ev,
                s,
                state,
                def,
                &u.actor.login,
                u.data.get("delete_branch").and_then(Value::as_bool) == Some(true),
            ),
            Ui::Approve => approval(ctx, ev, s, state, def),
            Ui::Comment => Transition::reject(state, "comments from the UI are for issues"),
            _ => return None,
        },
        EventPayload::Connector(e) => match e.event_type {
            EventType::ChangeRequestUpdated => updated(ctx, ev, s, state, def),
            EventType::ReviewSubmitted => review(ctx, ev, s, state, def),
            EventType::CiStatusChanged => ci_changed(ctx, ev, s, state, def),
            EventType::CommentAdded | EventType::Mentioned => comment(ctx, ev, s, state, def),
            EventType::ApprovalGranted => approval(ctx, ev, s, state, def),
            EventType::ChangeRequestOpened => {
                Transition::reject(state, "duplicate: already tracked")
            }
            _ => return None,
        },
        EventPayload::Unmergeable { base_sha } => conflict(ctx, ev, s, state, def, base_sha),
        EventPayload::ReadinessPoll => poll(ctx, ev, s, state),
        EventPayload::JobSucceeded { kind, output, .. } if s.data.waiting_on == Some(*kind) => {
            match kind {
                JobKind::PostComment if s.data.cr.as_ref()?.review_output.is_some() => {
                    let mut task = s.clone();
                    let out = crg(&mut task).review_output.take().unwrap_or_default();
                    advance(ctx, ev, &task, def, out, "review posted")
                }
                JobKind::CheckMergeReadiness => readiness(ctx, ev, s, def, output, true),
                JobKind::MergeChangeRequest => merge_finished(ctx, ev, s, def, output),
                JobKind::UpdateBranch => {
                    let mut task = s.clone();
                    let c = crg(&mut task);
                    if output.get("outcome").and_then(Value::as_str) == Some("updated") {
                        allow_update(c);
                    } else {
                        c.update_allowed = false;
                    }
                    advance(
                        ctx,
                        ev,
                        &task,
                        def,
                        output.clone(),
                        "branch update finished",
                    )
                }
                _ => return None,
            }
        }
        EventPayload::JobSucceeded {
            kind: JobKind::CheckMergeReadiness,
            output,
            ..
        } => readiness(ctx, ev, s, def, output, false),
        _ => return None,
    })
}

fn approve(c: &mut CrGraph) {
    c.data.last_review = "approve".into();
    c.data.reviewed_sha = c.data.head_sha.clone();
    c.data.unverified_checks.clear();
    c.must_fix = 0;
    crm::human_acted(&mut c.data);
}

/// `/approve`, the label or the UI approve, from a writer: enzarb's review check counts as
/// passed for the current head (as a human approval does in the compiled machine).
fn approval(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Transition {
    if ev.actor().is_some_and(|a| ctx.is_bot(a)) {
        return Transition::reject(state, "bot actor ignored");
    }
    if !ctx.can_write() {
        return Transition::reject(state, "approval requires write access");
    }
    if s.phase != GraphPhase::Waiting {
        return Transition::reject(state, "nothing to approve");
    }
    let mut task = s.clone();
    approve(crg(&mut task));
    if at_wait(def, s, "approval") {
        let login = ev.actor().map(|a| a.login.clone()).unwrap_or_default();
        let out = json!({"event": "approval", "author": login});
        return advance(ctx, ev, &task, def, out, "approved");
    }
    Transition::to(WorkflowState::Graph(task), Vec::new(), "approval recorded")
}

/// The UI merge: only at a wait on `merge_requested`, with write access and no dependency
/// left. The readiness re-check stays in the merge job.
fn merge(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
    actor: &str,
    delete_branch: bool,
) -> Transition {
    if !ctx.can_write() {
        return Transition::reject(state, "merge requires write access");
    }
    if !at_wait(def, s, "merge_requested") {
        return Transition::reject(state, "not ready to merge");
    }
    if !ctx.open_dependencies.is_empty() {
        return Transition::reject(state, "not ready to merge: dependencies are not done");
    }
    let mut task = s.clone();
    let c = crg(&mut task);
    c.data.merge_requested = true;
    c.merge_head = c.data.head_sha.clone();
    c.update_allowed = false;
    c.data.merge_actor = actor.to_string();
    c.data.merge_delete_branch = delete_branch;
    let out = json!({"event": "merge_requested", "author": actor, "delete_branch": delete_branch});
    advance(ctx, ev, &task, def, out, "merge requested")
}

fn updated(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Transition {
    let e = ev.connector().expect("connector");
    let sha = e.data.item.head_sha.clone().unwrap_or_default();
    let c = s.data.cr.as_ref().expect("change request");
    if sha.is_empty() || sha == c.data.head_sha {
        return Transition::reject(state, "duplicate: this commit is already the head");
    }
    let mut task = s.clone();
    let c = crg(&mut task);
    let person = !ctx.is_bot(&e.data.actor);
    // enzarb's own base update, pushed while the merge job (or `update_branch`) that made it
    // still runs: the click may follow it to this head (the job's outcome confirms or clears).
    if !person
        && matches!(
            s.data.waiting_on,
            Some(JobKind::MergeChangeRequest | JobKind::UpdateBranch)
        )
    {
        c.update_allowed = c.data.merge_requested && c.data.head_sha == c.run_head;
    }
    set_head(c, sha.clone());
    if person {
        crm::human_acted(&mut c.data);
        clear_click(c);
    }
    if at_wait(def, s, "pushed") && sha != c.data.reviewed_sha {
        c.push_pending = false;
        c.trigger = Trigger::None;
        let out = json!({"event": "pushed", "head_sha": sha, "author": e.data.actor.login});
        return advance(ctx, ev, &task, def, out, "new commits");
    }
    if person {
        c.push_pending = true;
    }
    Transition::to(WorkflowState::Graph(task), Vec::new(), "new head recorded")
}

/// Feedback text and its event output, or why it is not feedback.
fn feedback_allowed(
    ctx: &Ctx,
    s: &GraphTask,
    def: &Definition,
    event: &str,
) -> Result<(), &'static str> {
    if s.phase == GraphPhase::Running {
        return if ctx.can_write() {
            Ok(())
        } else {
            Err("addressing feedback requires write access")
        };
    }
    let Some((events, from)) = wait_events(def, s) else {
        return Err("not waiting for feedback");
    };
    if !events.iter().any(|e| e == event) {
        return Err("not waiting for this event");
    }
    if from == WaitFrom::Writers && !ctx.can_write() {
        return Err("this wait takes feedback from writers only");
    }
    Ok(())
}

/// A person's feedback: resumes a wait on `event`, or is queued while a job runs.
#[allow(clippy::too_many_arguments)]
fn feedback(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    def: &Definition,
    event: &str,
    output: Value,
    text: String,
    comment: Option<(u64, CommentKind)>,
) -> Transition {
    let mut task = s.clone();
    let c = crg(&mut task);
    if s.phase == GraphPhase::Running {
        c.queued = Some(Queued {
            event: event.to_string(),
            output,
            text,
            comment_id: comment.map(|x| x.0),
            comment_kind: comment.map(|x| x.1).unwrap_or_default(),
        });
        let mut fx = Fx::new(ctx, ev);
        fx.received(&ctx.progress_target(
            comment.map(|x| x.0),
            comment.map(|x| x.1).unwrap_or_default(),
        ));
        return Transition::to(
            WorkflowState::Graph(task),
            fx.finish(),
            "queued for the next round",
        );
    }
    c.trigger = Trigger::Human;
    crm::human_acted(&mut c.data);
    c.data.merge_requested = false;
    c.data.trigger_text = text;
    c.data.trigger_comment_id = comment.map(|x| x.0);
    c.data.trigger_comment_kind = comment.map(|x| x.1).unwrap_or_default();
    let mut fx = Fx::new(ctx, ev);
    fx.received(&ctx.progress_target(
        comment.map(|x| x.0),
        comment.map(|x| x.1).unwrap_or_default(),
    ));
    advance_with(ctx, fx, &task, def, output, &format!("{event} received"))
}

fn comment(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Transition {
    let Some(cl) = ev.comment_like() else {
        return Transition::reject(state, "not a comment");
    };
    if ctx.is_bot(cl.actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    if enzarb_connector::command::has_approve_command(cl.text, &ctx.bot_handle) {
        return Transition::reject(state, "approve command is not feedback");
    }
    if let Err(why) = feedback_allowed(ctx, s, def, "comment") {
        return Transition::reject(state, why);
    }
    let output = json!({
        "event": "comment",
        "author": cl.actor.login,
        "body": cl.text,
        "can_write": ctx.can_write(),
    });
    let id = cl.comment.map(|x| (x.id, x.kind));
    feedback(ctx, ev, s, def, "comment", output, cl.text.to_string(), id)
}

fn review(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Transition {
    let e = ev.connector().expect("connector");
    if ctx.is_bot(&e.data.actor) {
        return Transition::reject(state, "bot actor ignored");
    }
    let Some(r) = &e.data.review else {
        return Transition::reject(state, "review missing");
    };
    let rs = r.state.to_ascii_lowercase();
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
    let output = json!({
        "event": "review",
        "author": e.data.actor.login,
        "state": rs,
        "body": text,
        "can_write": ctx.can_write(),
    });
    if rs == "approved" {
        if !ctx.can_write() {
            return Transition::reject(state, "approval requires write access");
        }
        let mut task = s.clone();
        approve(crg(&mut task));
        if feedback_allowed(ctx, s, def, "review").is_ok() && s.phase == GraphPhase::Waiting {
            return advance(ctx, ev, &task, def, output, "review approved");
        }
        return Transition::to(WorkflowState::Graph(task), Vec::new(), "approval recorded");
    }
    if let Err(why) = feedback_allowed(ctx, s, def, "review") {
        return Transition::reject(state, why);
    }
    feedback(ctx, ev, s, def, "review", output, text, None)
}

fn ci_changed(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
) -> Transition {
    let Some(ci) = &ev.connector().expect("connector").data.ci else {
        return Transition::reject(state, "ci data missing");
    };
    let c = s.data.cr.as_ref().expect("change request");
    if s.data.waiting_on == Some(JobKind::MergeChangeRequest) {
        return Transition::reject(state, "busy: a merge is running");
    }
    if ci.head_sha.is_empty() || ci.head_sha != c.data.head_sha {
        return Transition::reject(state, "ci result is for another commit");
    }
    if ci
        .context
        .as_deref()
        .is_some_and(|n| ctx.config.ci_ignored(n))
    {
        return Transition::ack(state, "ignored check");
    }
    let mut task = s.clone();
    let c = crg(&mut task);
    crm::apply_ci(&mut c.data, ci);
    let mut fx = Fx::new(ctx, ev);
    // One check passed: ask for the aggregate.
    if c.data.ci == CiState::Unknown {
        fx.check_merge_readiness(c.data.number, &c.data.head_sha);
    }
    if at_wait(def, s, "ci")
        && let Some(out) = take_ci(c)
    {
        return advance_with(ctx, fx, &task, def, out, "ci status changed");
    }
    Transition::to(
        WorkflowState::Graph(task),
        fx.finish(),
        "ci status recorded",
    )
}

/// A `check_merge_readiness` result: the node's output when `awaited`, else a background
/// check (after a passing check or a poll) that may resume a wait on `ci`.
fn readiness(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    def: &Definition,
    output: &Value,
    awaited: bool,
) -> Transition {
    let state = WorkflowState::Graph(s.clone());
    let sha = output.get("head_sha").and_then(Value::as_str).unwrap_or("");
    let mut task = s.clone();
    let c = crg(&mut task);
    let current = !sha.is_empty() && sha == c.data.head_sha;
    if current {
        c.data.ci = CiState::from_wire(output.get("ci").and_then(Value::as_str).unwrap_or(""));
        c.data.ci_sha = sha.to_string();
        c.data.mergeable = output.get("mergeable").and_then(Value::as_bool);
        // The base the review covered, for delta re-reviews (as the compiled check does).
        if let Some(base) = output
            .get("base_sha")
            .and_then(Value::as_str)
            .filter(|b| !b.is_empty())
        {
            c.data.base_sha = base.to_string();
            let p = &mut c.data.prior_review;
            if p.base_sha.is_empty() && p.sha == sha {
                p.base_sha = base.to_string();
            }
        }
        // As the compiled `evaluate`: a red CI or requested changes drop a stored click.
        if c.data.ci == CiState::Failure || c.data.last_review == "request_changes" {
            clear_click(c);
        }
    }
    if awaited {
        mark_ci_seen(c);
        let mut out = output.clone();
        if let Value::Object(m) = &mut out {
            m.insert("current".into(), json!(current));
        }
        return advance(ctx, ev, &task, def, out, "readiness checked");
    }
    if !current {
        return Transition::ack(&state, "stale readiness check");
    }
    if at_wait(def, s, "ci")
        && let Some(out) = take_ci(c)
    {
        return advance(ctx, ev, &task, def, out, "readiness checked");
    }
    Transition::to(WorkflowState::Graph(task), Vec::new(), "readiness recorded")
}

/// The ticker's poll of a task that waits on CI: ask the connector again.
fn poll(ctx: &Ctx, ev: &EngineEvent, s: &GraphTask, state: &WorkflowState) -> Transition {
    let c = s.data.cr.as_ref().expect("change request");
    let decided = matches!(ci_wire(&c.data), "success" | "failure") && c.data.mergeable.is_some();
    if s.phase != GraphPhase::Waiting || c.data.head_sha.is_empty() || decided {
        return Transition::reject(state, "no readiness poll needed");
    }
    let mut fx = Fx::new(ctx, ev);
    fx.check_merge_readiness(c.data.number, &c.data.head_sha);
    Transition::to(state.clone(), fx.finish(), "readiness polled")
}

fn conflict(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    state: &WorkflowState,
    def: &Definition,
    base_sha: &str,
) -> Transition {
    let c = s.data.cr.as_ref().expect("change request");
    if !c.data.author.eq_ignore_ascii_case(&ctx.bot_login) {
        return Transition::reject(state, "not a change request opened by enzarb");
    }
    if base_sha.is_empty() {
        return Transition::reject(state, "base commit unknown");
    }
    if base_sha == c.conflict_base {
        return Transition::reject(state, "already attempted for this base commit");
    }
    if s.data.waiting_on == Some(JobKind::MergeChangeRequest) {
        return Transition::reject(state, "busy: a merge is running");
    }
    let mut task = s.clone();
    let c = crg(&mut task);
    c.data.base_sha = base_sha.to_string();
    c.data.record_resolve_attempt(base_sha);
    if at_wait(def, s, "conflict") {
        c.conflict_base = base_sha.to_string();
        c.conflict_pending = false;
        c.trigger = Trigger::None;
        let out = json!({"event": "conflict", "base_sha": base_sha});
        return advance(ctx, ev, &task, def, out, "unmergeable");
    }
    if s.data
        .cr
        .as_ref()
        .is_some_and(|x| x.conflict_pending && x.data.base_sha == base_sha)
    {
        return Transition::reject(state, "base commit already recorded");
    }
    c.conflict_pending = true;
    Transition::to(
        WorkflowState::Graph(task),
        Vec::new(),
        "base commit recorded",
    )
}

fn merge_finished(
    ctx: &Ctx,
    ev: &EngineEvent,
    s: &GraphTask,
    def: &Definition,
    output: &Value,
) -> Transition {
    let text = |k: &str| output.get(k).and_then(Value::as_str).unwrap_or("");
    let mut task = s.clone();
    let c = crg(&mut task);
    match text("outcome") {
        "merged" | "already_merged" => {
            let mut d = task.data.clone();
            // Record the merge's output; whatever the edges say, the change request is merged.
            let mut pos = d.graph.position();
            let _ = enzarb_graph::advance(def, &mut pos, output.clone());
            d.graph.set_position(pos);
            d.waiting_on = None;
            d.wait = None;
            if let Some(c) = &mut d.cr {
                c.data.merge_requested = false;
                c.data.merge_error.clear();
                c.queued = None;
            }
            let mut fx = Fx::new(ctx, ev);
            fx.wake_at(None);
            let actor = text("actor");
            fx.comment(if actor.is_empty() {
                "enzarb: merged.".to_string()
            } else {
                format!("enzarb: merged by @{actor}.")
            });
            fx.done(&target(ctx));
            let note = match text("branch") {
                "" => "merged".to_string(),
                branch => format!("merged; {branch}"),
            };
            Transition::to(st(GraphPhase::Merged, d), fx.finish(), note)
        }
        outcome => {
            if outcome != "updated_branch" {
                clear_click(c);
            }
            match outcome {
                "head_moved" => {
                    let d = &mut c.data;
                    d.merge_error =
                        "the branch changed while merging; reviewing the new commits".into();
                    d.merge_requested = false;
                    d.last_review.clear();
                    d.ci = CiState::Unknown;
                    d.mergeable = None;
                }
                "updated_branch" => {
                    // The click stays for the update commit only: the next wait on
                    // `merge_requested` at that head resumes by itself.
                    allow_update(c);
                    let d = &mut c.data;
                    d.merge_error =
                        "updating branch with the base; waiting for review and CI".into();
                    d.last_review.clear();
                    d.ci = CiState::Unknown;
                    d.mergeable = None;
                }
                "not_mergeable" => {
                    let d = &mut c.data;
                    let reason = text("reason");
                    d.merge_error = if reason.is_empty() {
                        "the change request cannot be merged".into()
                    } else {
                        super::truncate(reason, 300)
                    };
                    d.mergeable = Some(false);
                    d.merge_requested = false;
                }
                other => {
                    let mut d = s.data.clone();
                    let mut fx = Fx::new(ctx, ev);
                    let why = format!("unusable merge result {other:?}");
                    let phase = failed(ctx, &mut fx, &mut d, &why);
                    return Transition::to(st(phase, d), fx.finish(), "failed");
                }
            }
            advance(ctx, ev, &task, def, output.clone(), outcome)
        }
    }
}
