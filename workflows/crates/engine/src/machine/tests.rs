//! Table-driven tests for the three state machines.

mod auto_rounds;
mod ci_fix;
mod dependencies;
mod epic;
mod graph;
mod graph_cr;
mod graph_epics;
mod human_approval;
mod merge;

use chrono::{DateTime, TimeZone, Utc};
use enzarb_connector::{
    Actor, CommentRef, Event, EventData, EventType, ItemKind, ItemRef, Permission, RepoKey, Review,
    WorkItemKey,
};
use serde_json::{Value, json};

use super::*;

mod orphan;
mod resolve;
mod stacking;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn repo() -> RepoKey {
    RepoKey::new("github", "enzarb", "enzarb-ui")
}

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap()
}

fn ctx_for(number: u64, perm: Permission) -> Ctx {
    Ctx {
        work_item: WorkItemKey::new(repo(), number),
        now: now(),
        bot_login: "enzarb[bot]".into(),
        bot_handle: "enzarb".into(),
        actor_permission: perm,
        config: Config::default(),
        triggers: Triggers::default(),
        default_branch: "main".into(),
        orphaned: false,
        orphan_owner: false,
        open_dependencies: Vec::new(),
        stack_parent: None,
        epic_parent: None,
        graph: None,
        live_agent_jobs: false,
        blocking_dependencies: Vec::new(),
    }
}

fn ctx(perm: Permission) -> Ctx {
    ctx_for(42, perm)
}

fn actor(login: &str) -> Actor {
    Actor {
        login: login.into(),
        id: 7,
        is_bot: false,
    }
}

fn issue_item() -> ItemRef {
    ItemRef {
        kind: ItemKind::Issue,
        number: 42,
        url: "https://github.com/enzarb/enzarb-ui/issues/42".into(),
        title: "Add dark mode".into(),
        body: "Please add a dark theme.".into(),
        author: "alice".into(),
        head_ref: None,
        base_ref: None,
        head_sha: None,
    }
}

fn pr_item(sha: &str) -> ItemRef {
    ItemRef {
        kind: ItemKind::ChangeRequest,
        number: 43,
        url: "https://github.com/enzarb/enzarb-ui/pull/43".into(),
        title: "Dark mode".into(),
        body: "Adds dark mode".into(),
        author: "alice".into(),
        head_ref: Some("feat/dark".into()),
        base_ref: Some("main".into()),
        head_sha: Some(sha.into()),
    }
}

fn conn(
    id: &str,
    ty: EventType,
    item: ItemRef,
    who: Actor,
    comment: Option<(u64, &str)>,
) -> EngineEvent {
    let data = EventData {
        repo: repo(),
        actor: who,
        item,
        comment: comment.map(|(id, body)| CommentRef {
            id,
            kind: Default::default(),
            body: body.into(),
            url: String::new(),
        }),
        merged: None,
        review: None,
        ci: None,
        label: None,
    };
    EngineEvent {
        id: id.into(),
        payload: EventPayload::Connector(Box::new(Event::new("github", id, ty, now(), data))),
    }
}

fn opened(id: &str) -> EngineEvent {
    conn(
        id,
        EventType::WorkItemOpened,
        issue_item(),
        actor("alice"),
        None,
    )
}

fn comment_by(id: &str, who: &str, cid: u64, text: &str) -> EngineEvent {
    conn(
        id,
        EventType::CommentAdded,
        issue_item(),
        actor(who),
        Some((cid, text)),
    )
}

fn approval_comment(id: &str, who: &str) -> EngineEvent {
    conn(
        id,
        EventType::ApprovalGranted,
        issue_item(),
        actor(who),
        Some((900, "/approve")),
    )
}

fn approval_label(id: &str, who: &str) -> EngineEvent {
    conn(
        id,
        EventType::ApprovalGranted,
        issue_item(),
        actor(who),
        None,
    )
}

fn closed(id: &str, item: ItemRef, merged: bool) -> EngineEvent {
    let mut e = conn(id, EventType::WorkItemClosed, item, actor("alice"), None);
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.merged = Some(merged);
    }
    e
}

fn ui(id: &str, who: &str, action: UiActionKind) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::Ui(UiAction {
            actor: actor(who),
            action,
            data: Value::Null,
        }),
    }
}

fn agent_output(decision: &str, body: &str) -> Value {
    let block = json!({ "decision": decision, "body": body });
    json!({
        "summary": format!("All done.\n\n```json\n{block}\n```\n"),
        "branch": "enzarb/issue-42",
        "head_sha": "sha2",
        "pushed": true,
    })
}

fn agent_ok_checks(id: &str, kind: JobKind, decision: &str, checks: Value) -> EngineEvent {
    let block = json!({ "decision": decision, "body": "LGTM", "checks": checks });
    ok(
        id,
        kind,
        json!({ "summary": format!("Done.\n\n```json\n{block}\n```\n") }),
    )
}

fn ok(id: &str, kind: JobKind, output: Value) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::JobSucceeded {
            job_id: 1,
            kind,
            output,
        },
    }
}

fn agent_ok(id: &str, kind: JobKind, decision: &str, body: &str) -> EngineEvent {
    ok(id, kind, agent_output(decision, body))
}

fn failed(id: &str, kind: JobKind) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::JobFailed {
            job_id: 1,
            kind,
            error: "boom".into(),
        },
    }
}

fn timer(id: &str, job_id: Option<i64>) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::TimerFired { job_id },
    }
}

fn pr_opened_output() -> Value {
    serde_json::to_value(pr_item("sha2")).unwrap()
}

// -- state construction ------------------------------------------------------

fn issue_data() -> IssueData {
    IssueData {
        number: 42,
        url: issue_item().url,
        title: "Add dark mode".into(),
        body: "Please add a dark theme.".into(),
        author: "alice".into(),
        triage_summary: "SUM".into(),
        plan: "PLAN".into(),
        ..Default::default()
    }
}

fn issue_in(phase: IssuePhase) -> WorkflowState {
    WorkflowState::Issue(IssueState {
        phase,
        data: issue_data(),
    })
}

fn issue_implementing_pr() -> WorkflowState {
    WorkflowState::Issue(IssueState {
        phase: IssuePhase::Implementing,
        data: IssueData {
            implemented: true,
            ..issue_data()
        },
    })
}

fn cr_data() -> CrData {
    CrData {
        number: 43,
        url: pr_item("sha1").url,
        title: "Dark mode".into(),
        body: "Adds dark mode".into(),
        author: "alice".into(),
        head_ref: "feat/dark".into(),
        base_ref: "main".into(),
        head_sha: "sha1".into(),
        reviewed_sha: "sha1".into(),
        ..Default::default()
    }
}

fn cr_in(phase: CrPhase) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState {
        phase,
        data: cr_data(),
    })
}

fn mention_data() -> MentionData {
    MentionData {
        number: 42,
        url: issue_item().url,
        title: "Add dark mode".into(),
        body: "body".into(),
        author: "alice".into(),
        trigger_comment_id: Some(5),
        trigger_text: "@enzarb how does X work?".into(),
        ..Default::default()
    }
}

fn mention_in(phase: MentionPhase) -> WorkflowState {
    WorkflowState::Mention(MentionState {
        phase,
        data: mention_data(),
    })
}

// -- assertions --------------------------------------------------------------

fn run(perm: Permission, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    transition(&ctx(perm), state, ev)
}

fn run_cr(perm: Permission, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    transition(&ctx_for(43, perm), state, ev)
}

/// Like `run_cr` with automatic fix rounds disabled.
fn run_cr_manual(perm: Permission, state: &WorkflowState, ev: &EngineEvent) -> Transition {
    let mut c = ctx_for(43, perm);
    c.config.max_auto_rounds = 0;
    transition(&c, state, ev)
}

#[track_caller]
fn rejected(t: &Transition, before: &WorkflowState) {
    assert!(!t.accepted, "expected rejection, got {:?}", t);
    assert!(t.effects.is_empty(), "rejected events have no effects");
    assert_eq!(&t.next, before, "rejected events do not change the state");
    assert!(!t.note.is_empty());
}

fn jobs(t: &Transition) -> Vec<(JobKind, i32, String, Value)> {
    t.effects
        .iter()
        .filter_map(|e| match e {
            Effect::Job {
                kind,
                priority,
                idempotency_key,
                input,
            } => Some((*kind, *priority, idempotency_key.clone(), input.clone())),
            _ => None,
        })
        .collect()
}

fn kinds(t: &Transition) -> Vec<JobKind> {
    jobs(t).into_iter().map(|j| j.0).collect()
}

fn job_of(t: &Transition, kind: JobKind) -> (i32, String, Value) {
    let found: Vec<_> = jobs(t).into_iter().filter(|j| j.0 == kind).collect();
    assert_eq!(found.len(), 1, "expected one {kind:?} in {:?}", kinds(t));
    let (_, p, k, i) = found.into_iter().next().unwrap();
    (p, k, i)
}

fn first_progress(t: &Transition) -> (i32, String, Value) {
    let (_, p, k, i) = jobs(t)
        .into_iter()
        .find(|j| j.0 == JobKind::SetProgress)
        .expect("a set_progress job");
    (p, k, i)
}

fn progress_values(t: &Transition) -> Vec<String> {
    jobs(t)
        .into_iter()
        .filter(|j| j.0 == JobKind::SetProgress)
        .map(|j| j.3["progress"].as_str().unwrap().to_string())
        .collect()
}

fn has(t: &Transition, f: impl Fn(&Effect) -> bool) -> bool {
    t.effects.iter().any(f)
}

fn cancels(t: &Transition) -> bool {
    has(t, |e| matches!(e, Effect::CancelJobs))
}

fn wake(t: &Transition) -> Option<Option<DateTime<Utc>>> {
    t.effects.iter().find_map(|e| match e {
        Effect::SetWakeAt { at } => Some(*at),
        _ => None,
    })
}

#[track_caller]
fn assert_issue(t: &Transition, phase: IssuePhase) -> IssueData {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::Issue(s) => {
            assert_eq!(s.phase, phase, "note: {}", t.note);
            s.data.clone()
        }
        other => panic!("not an issue state: {other:?}"),
    }
}

#[track_caller]
fn assert_cr(t: &Transition, phase: CrPhase) -> CrData {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::ChangeRequest(s) => {
            assert_eq!(s.phase, phase, "note: {}", t.note);
            s.data.clone()
        }
        other => panic!("not a change request state: {other:?}"),
    }
}

#[track_caller]
fn assert_mention(t: &Transition, phase: MentionPhase) -> MentionData {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::Mention(s) => {
            assert_eq!(s.phase, phase, "note: {}", t.note);
            s.data.clone()
        }
        other => panic!("not a mention state: {other:?}"),
    }
}

fn comment_bodies(t: &Transition) -> Vec<String> {
    jobs(t)
        .into_iter()
        .filter(|j| j.0 == JobKind::PostComment)
        .map(|j| j.3["body"].as_str().unwrap().to_string())
        .collect()
}

const ALL_ISSUE_PHASES: [IssuePhase; 12] = [
    IssuePhase::Triaging,
    IssuePhase::AwaitingAnswers,
    IssuePhase::Stale,
    IssuePhase::Duplicate,
    IssuePhase::Planning,
    IssuePhase::AwaitingApproval,
    IssuePhase::Implementing,
    IssuePhase::Decomposing,
    IssuePhase::AwaitingChildren,
    IssuePhase::Done,
    IssuePhase::Failed,
    IssuePhase::Cancelled,
];

const ALL_CR_PHASES: [CrPhase; 11] = [
    CrPhase::Reviewing,
    CrPhase::AwaitingChanges,
    CrPhase::ReadyToMerge,
    CrPhase::AddressingComments,
    CrPhase::ResolvingConflicts,
    CrPhase::ConflictParked,
    CrPhase::Merging,
    CrPhase::Merged,
    CrPhase::Closed,
    CrPhase::Failed,
    CrPhase::Cancelled,
];

const ALL_MENTION_PHASES: [MentionPhase; 4] = [
    MentionPhase::Running,
    MentionPhase::Done,
    MentionPhase::Failed,
    MentionPhase::Cancelled,
];

// ---------------------------------------------------------------------------
// Routing and starting
// ---------------------------------------------------------------------------

#[test]
fn route_table() {
    let c = ctx(Permission::Write);
    let mention = conn(
        "m",
        EventType::Mentioned,
        issue_item(),
        actor("bob"),
        Some((1, "@enzarb hi")),
    );
    let table: Vec<(EngineEvent, Option<WorkflowKind>)> = vec![
        (opened("1"), Some(WorkflowKind::Issue)),
        (
            conn(
                "2",
                EventType::WorkItemOpened,
                pr_item("s"),
                actor("a"),
                None,
            ),
            None,
        ),
        (
            conn(
                "3",
                EventType::ChangeRequestOpened,
                pr_item("s"),
                actor("a"),
                None,
            ),
            Some(WorkflowKind::ChangeRequest),
        ),
        (
            conn(
                "4",
                EventType::ChangeRequestOpened,
                issue_item(),
                actor("a"),
                None,
            ),
            None,
        ),
        (mention, Some(WorkflowKind::Mention)),
        (comment_by("5", "bob", 1, "hi"), None),
        (approval_label("6", "bob"), None),
        (ui("7", "bob", UiActionKind::Approve), None),
        (timer("8", None), None),
    ];
    for (ev, want) in table {
        assert_eq!(route(&c, &ev), want, "{:?}", ev.payload);
    }
}

#[test]
fn triggers_gate_routing() {
    let mut c = ctx(Permission::Write);
    c.triggers = Triggers {
        issues: false,
        change_requests: false,
        mentions: false,
    };
    let mention = conn(
        "m",
        EventType::Mentioned,
        issue_item(),
        actor("bob"),
        Some((1, "@enzarb hi")),
    );
    let pr = conn(
        "3",
        EventType::ChangeRequestOpened,
        pr_item("s"),
        actor("a"),
        None,
    );
    for ev in [opened("1"), pr, mention] {
        assert_eq!(route(&c, &ev), None);
    }
    // Starting a workflow of the wrong kind, or with the trigger off, is rejected.
    let new_issue = WorkflowState::New(WorkflowKind::Issue);
    let t = transition(&c, &new_issue, &opened("1"));
    rejected(&t, &new_issue);
    let new_cr = WorkflowState::New(WorkflowKind::ChangeRequest);
    rejected(
        &transition(&ctx(Permission::Write), &new_cr, &opened("1")),
        &new_cr,
    );
}

#[test]
fn new_states_reject_events_that_do_not_start_them() {
    let c = ctx(Permission::Write);
    for kind in [
        WorkflowKind::Issue,
        WorkflowKind::ChangeRequest,
        WorkflowKind::Mention,
    ] {
        let s = WorkflowState::New(kind);
        for ev in [
            comment_by("c", "bob", 1, "hi"),
            timer("t", None),
            failed("f", JobKind::AgentTriage),
        ] {
            rejected(&transition(&c, &s, &ev), &s);
        }
    }
}

#[test]
fn start_functions_reject_foreign_or_bot_events() {
    let c = ctx(Permission::Write);
    // Called directly: the router normally prevents these.
    let s = WorkflowState::New(WorkflowKind::Issue);
    rejected(&issue::start(&c, &s, &timer("t", None)), &s);
    let pr = conn(
        "p",
        EventType::WorkItemOpened,
        pr_item("s"),
        actor("a"),
        None,
    );
    rejected(&issue::start(&c, &s, &pr), &s);
    let bot = conn(
        "b",
        EventType::WorkItemOpened,
        issue_item(),
        actor("enzarb[bot]"),
        None,
    );
    rejected(&issue::start(&c, &s, &bot), &s);
    let cr = WorkflowState::New(WorkflowKind::ChangeRequest);
    rejected(&change_request::start(&c, &cr, &opened("1")), &cr);
    let m = WorkflowState::New(WorkflowKind::Mention);
    rejected(&mention::start(&c, &m, &opened("1")), &m);
}

// ---------------------------------------------------------------------------
// Issue workflow
// ---------------------------------------------------------------------------

#[test]
fn issue_opened_starts_triage_with_progress_and_keys() {
    let t = run(
        Permission::None,
        &WorkflowState::New(WorkflowKind::Issue),
        &opened("e1"),
    );
    let d = assert_issue(&t, IssuePhase::Triaging);
    assert_eq!((d.number, d.author.as_str()), (42, "alice"));
    assert_eq!(
        kinds(&t),
        vec![
            JobKind::SetProgress,
            JobKind::SetProgress,
            JobKind::AgentTriage
        ]
    );
    assert_eq!(progress_values(&t), vec!["Received", "Working"]);
    let js = jobs(&t);
    // Received outranks Working outranks the agent job.
    assert_eq!(js[0].1, PRIORITY_PROGRESS);
    assert_eq!(js[1].1, PRIORITY_WORKING);
    assert_eq!(js[2].1, PRIORITY_AGENT);
    // Deterministic keys: <work_item>:<event_id>:<kind>, `#2` for a second one.
    assert_eq!(js[0].2, "github:enzarb/enzarb-ui#42:e1:set_progress");
    assert_eq!(js[1].2, "github:enzarb/enzarb-ui#42:e1:set_progress#2");
    assert_eq!(js[2].2, "github:enzarb/enzarb-ui#42:e1:agent_triage");
    assert_eq!(js[0].3["target"]["comment_id"], Value::Null);
    // The agent job carries a rendered prompt.
    let input: AgentJobInput = serde_json::from_value(js[2].3.clone()).unwrap();
    assert_eq!(input.kind, JobKind::AgentTriage);
    assert!(!input.push);
    assert_eq!(input.base_ref, "main");
    assert!(input.prompt.contains("Add dark mode") && input.prompt.contains("dark theme"));
    assert_eq!(input.context.repo, "github:enzarb/enzarb-ui");
    // Same input, same output.
    let again = run(
        Permission::None,
        &WorkflowState::New(WorkflowKind::Issue),
        &opened("e1"),
    );
    assert_eq!(t, again);
}

#[test]
fn issue_happy_path_to_change_request_handoff() {
    let new = WorkflowState::New(WorkflowKind::Issue);
    let t = run(Permission::None, &new, &opened("1"));
    let mut state = t.next;

    // triage -> ready -> plan
    let t = run(
        Permission::None,
        &state,
        &agent_ok("2", JobKind::AgentTriage, "ready", "SUMMARY"),
    );
    let d = assert_issue(&t, IssuePhase::Planning);
    assert_eq!(d.triage_summary, "SUMMARY");
    assert_eq!(kinds(&t), vec![JobKind::SetProgress, JobKind::AgentPlan]);
    assert_eq!(progress_values(&t), vec!["Working"]);
    let input: AgentJobInput = serde_json::from_value(job_of(&t, JobKind::AgentPlan).2).unwrap();
    assert!(input.prompt.contains("SUMMARY"));
    state = t.next;

    // plan -> posted, awaiting approval
    let t = run(
        Permission::None,
        &state,
        &agent_ok("3", JobKind::AgentPlan, "plan", "## Plan\nDo it"),
    );
    let d = assert_issue(&t, IssuePhase::AwaitingApproval);
    assert_eq!(d.plan, "## Plan\nDo it");
    assert_eq!(comment_bodies(&t), vec!["## Plan\nDo it".to_string()]);
    assert_eq!(progress_values(&t), vec!["Done"]);
    state = t.next;

    // approval from a writer -> implementing, with a pushing run on the issue branch
    let t = run(Permission::Write, &state, &approval_comment("4", "carol"));
    assert_issue(&t, IssuePhase::Implementing);
    assert_eq!(progress_values(&t), vec!["Received", "Working"]);
    let (_, _, input) = first_progress(&t);
    assert_eq!(input["target"]["comment_id"], 900);
    let input: AgentJobInput =
        serde_json::from_value(job_of(&t, JobKind::AgentImplement).2).unwrap();
    assert!(input.push);
    assert_eq!(input.branch, "enzarb/issue-42");
    assert!(input.prompt.contains("## Plan\nDo it"));
    state = t.next;

    // agent done -> open the change request, still Implementing
    let t = run(
        Permission::None,
        &state,
        &agent_ok("5", JobKind::AgentImplement, "done", "PR text"),
    );
    let d = assert_issue(&t, IssuePhase::Implementing);
    assert!(d.implemented);
    assert_eq!(kinds(&t), vec![JobKind::OpenChangeRequest]);
    let spec = &job_of(&t, JobKind::OpenChangeRequest).2["spec"];
    assert_eq!(spec["head_ref"], "enzarb/issue-42");
    assert_eq!(spec["base_ref"], "main");
    assert_eq!(spec["title"], "feat(ui): add dark mode");
    assert!(
        spec["body"]
            .as_str()
            .unwrap()
            .ends_with("PR text\n\nCloses #42")
    );
    state = t.next;

    // PR opened -> Done and hand off
    let t = run(
        Permission::None,
        &state,
        &ok("6", JobKind::OpenChangeRequest, pr_opened_output()),
    );
    let d = assert_issue(&t, IssuePhase::Done);
    assert_eq!(d.change_request, Some(43));
    assert_eq!(progress_values(&t), vec!["Done"]);
    let start = t
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::StartWorkflow {
                key,
                workflow,
                event,
            } => Some((key, workflow, event)),
            _ => None,
        })
        .expect("StartWorkflow");
    assert_eq!(start.0.to_string(), "github:enzarb/enzarb-ui#43");
    assert_eq!(*start.1, WorkflowKind::ChangeRequest);
    assert_eq!(start.2.event_type, EventType::ChangeRequestOpened);
    assert_eq!(start.2.data.item.kind, ItemKind::ChangeRequest);
    assert_eq!(start.2.data.item.head_ref.as_deref(), Some("feat/dark"));
    assert_eq!(start.2.subject.to_string(), "github:enzarb/enzarb-ui#43");
    // ...and that event really does start the change_request workflow.
    let handoff = EngineEvent {
        id: "h".into(),
        payload: EventPayload::Connector(start.2.clone()),
    };
    let c = ctx_for(43, Permission::None);
    assert_eq!(route(&c, &handoff), Some(WorkflowKind::ChangeRequest));
    let t2 = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &handoff,
    );
    assert_cr(&t2, CrPhase::Reviewing);
}

#[test]
fn issue_triage_questions_then_author_answers() {
    let t = run(
        Permission::None,
        &issue_in(IssuePhase::Triaging),
        &agent_ok("2", JobKind::AgentTriage, "questions", "1. Which pages?"),
    );
    let d = assert_issue(&t, IssuePhase::AwaitingAnswers);
    assert!(!d.nudged);
    assert_eq!(comment_bodies(&t), vec!["1. Which pages?".to_string()]);
    assert_eq!(progress_values(&t), vec!["Done"]);
    assert_eq!(
        wake(&t),
        Some(Some(
            now() + chrono::Duration::seconds(Config::default().nudge_after_secs)
        ))
    );

    // Only the author's reply re-triages (case-insensitive), anyone else is ignored.
    let state = t.next;
    let t = run(
        Permission::Read,
        &state,
        &comment_by("3", "Alice", 77, "All pages"),
    );
    let d = assert_issue(&t, IssuePhase::Triaging);
    assert_eq!(d.trigger_comment_id, Some(77));
    assert_eq!(progress_values(&t), vec!["Received", "Working"]);
    let (_, _, p) = first_progress(&t);
    assert_eq!(p["target"]["comment_id"], 77);
    assert_eq!(wake(&t), Some(None));
    let input: AgentJobInput = serde_json::from_value(job_of(&t, JobKind::AgentTriage).2).unwrap();
    assert!(input.prompt.contains("All pages"));

    // A maintainer is not the author.
    rejected(
        &run(
            Permission::Admin,
            &state,
            &comment_by("4", "carol", 78, "hi"),
        ),
        &state,
    );
    // Bots never are.
    let mut bot = comment_by("5", "alice", 79, "hi");
    if let EventPayload::Connector(e) = &mut bot.payload {
        e.data.actor.is_bot = true;
    }
    rejected(&run(Permission::Write, &state, &bot), &state);
    rejected(
        &run(
            Permission::Write,
            &state,
            &comment_by("6", "enzarb[bot]", 80, "hi"),
        ),
        &state,
    );
}

#[test]
fn issue_triage_duplicate() {
    let t = run(
        Permission::None,
        &issue_in(IssuePhase::Triaging),
        &agent_ok("2", JobKind::AgentTriage, "duplicate", "Duplicate of #3"),
    );
    assert_issue(&t, IssuePhase::Duplicate);
    assert_eq!(comment_bodies(&t), vec!["Duplicate of #3".to_string()]);
    assert_eq!(progress_values(&t), vec!["Done"]);
}

#[test]
fn issue_nudge_then_stale_then_resume() {
    let mut s = issue_in(IssuePhase::AwaitingAnswers);
    let t = run(Permission::None, &s, &timer("t1", None));
    let d = assert_issue(&t, IssuePhase::AwaitingAnswers);
    assert!(d.nudged);
    assert!(comment_bodies(&t)[0].contains("@alice"));
    assert_eq!(
        wake(&t),
        Some(Some(
            now() + chrono::Duration::seconds(Config::default().stale_after_secs)
        ))
    );
    s = t.next;

    let t = run(Permission::None, &s, &timer("t2", None));
    assert_issue(&t, IssuePhase::Stale);
    assert_eq!(wake(&t), Some(None));
    s = t.next;

    // The author can resume a stale issue; others cannot.
    rejected(
        &run(Permission::Write, &s, &comment_by("c1", "carol", 1, "hi")),
        &s,
    );
    let t = run(
        Permission::None,
        &s,
        &comment_by("c2", "alice", 2, "sorry, here"),
    );
    assert_issue(&t, IssuePhase::Triaging);
    // So can a maintainer's retry.
    let t = run(
        Permission::Write,
        &s,
        &ui("r", "carol", UiActionKind::Retry),
    );
    assert_issue(&t, IssuePhase::Triaging);
    rejected(
        &run(Permission::Read, &s, &ui("r2", "bob", UiActionKind::Retry)),
        &s,
    );
}

fn ui_comment(id: &str, who: &str, body: &str) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::Ui(UiAction {
            actor: actor(who),
            action: UiActionKind::Comment,
            data: json!({ "body": body }),
        }),
    }
}

#[test]
fn ui_comment_on_a_plan_replans_and_posts_the_text() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    let t = run(
        Permission::Write,
        &s,
        &ui_comment("1", "carol", "  use CSS vars  "),
    );
    let d = assert_issue(&t, IssuePhase::Planning);
    assert_eq!(d.trigger_text, "use CSS vars");
    assert_eq!(d.trigger_comment_id, None);
    assert!(kinds(&t).contains(&JobKind::AgentPlan));
    let (_, _, posted) = job_of(&t, JobKind::PostComment);
    let body = posted["body"].as_str().unwrap();
    assert!(
        body.starts_with("use CSS vars") && body.contains("@carol"),
        "{body}"
    );
    assert_eq!(posted["item"], json!(WorkItemKey::new(repo(), 42)));
    // The author without write access may too, a stranger may not.
    assert_issue(
        &run(Permission::Read, &s, &ui_comment("2", "alice", "x")),
        IssuePhase::Planning,
    );
    rejected(
        &run(Permission::Read, &s, &ui_comment("3", "mallory", "x")),
        &s,
    );
}

#[test]
fn ui_comment_answers_questions_but_only_for_the_author() {
    let s = issue_in(IssuePhase::AwaitingAnswers);
    let t = run(Permission::Read, &s, &ui_comment("1", "alice", "yes, v2"));
    assert_issue(&t, IssuePhase::Triaging);
    assert!(kinds(&t).contains(&JobKind::PostComment));
    rejected(
        &run(Permission::Write, &s, &ui_comment("2", "carol", "x")),
        &s,
    );
}

#[test]
fn ui_comment_is_rejected_when_empty_or_not_applicable() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    rejected(
        &run(Permission::Write, &s, &ui_comment("1", "carol", "  ")),
        &s,
    );
    // An /approve command goes through the approve action, not through feedback.
    rejected(
        &run(Permission::Write, &s, &ui_comment("2", "carol", "/approve")),
        &s,
    );
    let busy = issue_in(IssuePhase::Planning);
    rejected(
        &run(Permission::Write, &busy, &ui_comment("3", "carol", "x")),
        &busy,
    );
    let m = mention_in(MentionPhase::Running);
    rejected(
        &run(Permission::Write, &m, &ui_comment("4", "carol", "x")),
        &m,
    );
}

#[test]
fn issue_plan_feedback_replans() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    // Author without write access may give feedback.
    let t = run(
        Permission::Read,
        &s,
        &comment_by("1", "alice", 5, "use CSS vars"),
    );
    let d = assert_issue(&t, IssuePhase::Planning);
    assert_eq!(d.trigger_text, "use CSS vars");
    assert_eq!(d.plan, "PLAN", "the previous plan is kept for the re-plan");
    let input: AgentJobInput = serde_json::from_value(job_of(&t, JobKind::AgentPlan).2).unwrap();
    assert!(input.prompt.contains("use CSS vars") && input.prompt.contains("PLAN"));
    // A maintainer may too.
    let t = run(
        Permission::Write,
        &s,
        &comment_by("2", "carol", 6, "also tests"),
    );
    assert_issue(&t, IssuePhase::Planning);
    // A mention counts as a comment.
    let m = conn(
        "3",
        EventType::Mentioned,
        issue_item(),
        actor("carol"),
        Some((7, "@enzarb please also add tests")),
    );
    assert_issue(&run(Permission::Write, &s, &m), IssuePhase::Planning);
    // A stranger may not, nor the bot.
    rejected(
        &run(Permission::Read, &s, &comment_by("4", "mallory", 8, "x")),
        &s,
    );
    rejected(
        &run(
            Permission::Write,
            &s,
            &comment_by("5", "enzarb[bot]", 9, "x"),
        ),
        &s,
    );
    // The revised plan is posted again and waits for approval again.
    let replanning = run(Permission::Write, &s, &comment_by("6", "carol", 10, "x")).next;
    let t = run(
        Permission::None,
        &replanning,
        &agent_ok("7", JobKind::AgentPlan, "plan", "v2"),
    );
    assert_eq!(assert_issue(&t, IssuePhase::AwaitingApproval).plan, "v2");
}

#[test]
fn issue_approval_authorization_table() {
    let awaiting = issue_in(IssuePhase::AwaitingApproval);
    // (perm, event, accepted)
    let table: Vec<(Permission, EngineEvent, bool)> = vec![
        (Permission::Write, approval_comment("1", "carol"), true),
        (Permission::Maintain, approval_label("2", "carol"), true),
        (
            Permission::Admin,
            ui("3", "carol", UiActionKind::Approve),
            true,
        ),
        (Permission::Triage, approval_comment("4", "carol"), false),
        (Permission::Read, approval_label("5", "bob"), false),
        (
            Permission::None,
            ui("6", "bob", UiActionKind::Approve),
            false,
        ),
        (
            Permission::Write,
            approval_comment("7", "enzarb[bot]"),
            false,
        ),
    ];
    for (perm, ev, want) in table {
        let t = run(perm, &awaiting, &ev);
        assert_eq!(t.accepted, want, "{perm:?} {:?} -> {}", ev.payload, t.note);
        if want {
            assert_issue(&t, IssuePhase::Implementing);
        } else {
            rejected(&t, &awaiting);
        }
    }
    // Label approval targets the issue, a comment approval the comment.
    let t = run(Permission::Write, &awaiting, &approval_label("2", "carol"));
    assert_eq!(first_progress(&t).2["target"]["comment_id"], Value::Null);
}

#[test]
fn issue_approval_outside_awaiting_approval_is_ignored() {
    for phase in ALL_ISSUE_PHASES {
        if phase == IssuePhase::AwaitingApproval {
            continue;
        }
        let s = issue_in(phase);
        for ev in [
            approval_comment("1", "carol"),
            ui("2", "carol", UiActionKind::Approve),
        ] {
            rejected(&run(Permission::Write, &s, &ev), &s);
        }
    }
}

#[test]
fn issue_close_cancels_from_every_active_state() {
    for phase in ALL_ISSUE_PHASES {
        let s = issue_in(phase);
        let ev = closed("c", issue_item(), false);
        let t = run(Permission::None, &s, &ev);
        match phase {
            IssuePhase::Done | IssuePhase::Duplicate | IssuePhase::Cancelled => rejected(&t, &s),
            _ => {
                assert_issue(&t, IssuePhase::Cancelled);
                assert!(cancels(&t), "{phase:?}");
                assert_eq!(wake(&t), Some(None));
                assert_eq!(t.effects[0], Effect::CancelJobs, "cancel comes first");
            }
        }
    }
}

#[test]
fn issue_ui_cancel() {
    for phase in ALL_ISSUE_PHASES {
        let s = issue_in(phase);
        let t = run(
            Permission::Write,
            &s,
            &ui("c", "carol", UiActionKind::Cancel),
        );
        match phase {
            IssuePhase::Done | IssuePhase::Duplicate | IssuePhase::Cancelled => rejected(&t, &s),
            _ => {
                assert_issue(&t, IssuePhase::Cancelled);
                assert!(cancels(&t));
                assert!(comment_bodies(&t)[0].contains("@carol"));
            }
        }
        rejected(
            &run(Permission::Read, &s, &ui("c2", "bob", UiActionKind::Cancel)),
            &s,
        );
    }
}

#[test]
fn issue_job_failures_and_retry() {
    let table = [
        (IssuePhase::Triaging, JobKind::AgentTriage),
        (IssuePhase::Planning, JobKind::AgentPlan),
        (IssuePhase::Implementing, JobKind::AgentImplement),
        (IssuePhase::Implementing, JobKind::OpenChangeRequest),
    ];
    for (phase, kind) in table {
        let s = issue_in(phase);
        let t = run(Permission::None, &s, &failed("f", kind));
        let d = assert_issue(&t, IssuePhase::Failed);
        assert_eq!(d.failed_phase, Some(phase));
        assert!(d.error.contains("boom"));
        assert_eq!(progress_values(&t), vec!["Failed"]);
        assert!(comment_bodies(&t)[0].contains("boom"));
        assert!(!cancels(&t));

        // Retry resumes the failed phase.
        let t2 = run(
            Permission::Write,
            &t.next,
            &ui("r", "carol", UiActionKind::Retry),
        );
        let d2 = assert_issue(&t2, phase);
        assert_eq!(d2.failed_phase, None);
        assert!(!d2.implemented);
        let agent = match phase {
            IssuePhase::Triaging => JobKind::AgentTriage,
            IssuePhase::Planning => JobKind::AgentPlan,
            _ => JobKind::AgentImplement,
        };
        assert!(kinds(&t2).contains(&agent));
        assert_eq!(progress_values(&t2), vec!["Received", "Working"]);
        // No permission, no retry.
        rejected(
            &run(
                Permission::Read,
                &t.next,
                &ui("r", "bob", UiActionKind::Retry),
            ),
            &t.next,
        );
    }
    // A failed state without a remembered phase restarts triage.
    let mut s = issue_in(IssuePhase::Failed);
    if let WorkflowState::Issue(i) = &mut s {
        i.data.failed_phase = None;
    }
    let t = run(
        Permission::Write,
        &s,
        &ui("r", "carol", UiActionKind::Retry),
    );
    assert_issue(&t, IssuePhase::Triaging);
    // Nothing to retry elsewhere.
    for phase in [
        IssuePhase::Triaging,
        IssuePhase::AwaitingApproval,
        IssuePhase::Done,
        IssuePhase::Cancelled,
    ] {
        let s = issue_in(phase);
        rejected(
            &run(
                Permission::Write,
                &s,
                &ui("r", "carol", UiActionKind::Retry),
            ),
            &s,
        );
    }
}

#[test]
fn issue_best_effort_job_results_are_acked_and_stale_ones_rejected() {
    for phase in ALL_ISSUE_PHASES {
        let s = issue_in(phase);
        for kind in [JobKind::SetProgress, JobKind::PostComment] {
            let t = run(Permission::None, &s, &ok("a", kind, json!({})));
            assert!(t.accepted && t.effects.is_empty() && t.next == s);
            let t = run(Permission::None, &s, &failed("b", kind));
            assert!(t.accepted && t.effects.is_empty() && t.next == s);
        }
    }
    // A result for a job the phase is not waiting for is stale.
    let table = [
        (IssuePhase::Planning, JobKind::AgentTriage),
        (IssuePhase::Triaging, JobKind::AgentPlan),
        (IssuePhase::Done, JobKind::AgentImplement),
        (IssuePhase::AwaitingApproval, JobKind::AgentPlan),
        (IssuePhase::Cancelled, JobKind::AgentTriage),
        (IssuePhase::Triaging, JobKind::AgentReview),
    ];
    for (phase, kind) in table {
        let s = issue_in(phase);
        rejected(
            &run(Permission::None, &s, &agent_ok("x", kind, "plan", "b")),
            &s,
        );
        rejected(&run(Permission::None, &s, &failed("y", kind)), &s);
    }
}

#[test]
fn issue_duplicate_job_results_do_not_apply_twice() {
    // The implement result arrives again after the PR is being opened.
    let s = issue_implementing_pr();
    rejected(
        &run(
            Permission::None,
            &s,
            &agent_ok("5b", JobKind::AgentImplement, "done", "x"),
        ),
        &s,
    );
    // The PR result arrives before the implement result.
    let s = issue_in(IssuePhase::Implementing);
    rejected(
        &run(
            Permission::None,
            &s,
            &ok("6", JobKind::OpenChangeRequest, pr_opened_output()),
        ),
        &s,
    );
    // And a second PR result after Done.
    let done = issue_in(IssuePhase::Done);
    rejected(
        &run(
            Permission::None,
            &done,
            &ok("7", JobKind::OpenChangeRequest, pr_opened_output()),
        ),
        &done,
    );
}

#[test]
fn issue_bad_agent_output_fails_the_item() {
    let cases: Vec<(IssuePhase, JobKind, Value)> = vec![
        (
            IssuePhase::Triaging,
            JobKind::AgentTriage,
            json!({ "summary": "no block" }),
        ),
        (IssuePhase::Triaging, JobKind::AgentTriage, json!({})),
        (
            IssuePhase::Triaging,
            JobKind::AgentTriage,
            agent_output("plan", "x"),
        ),
        (
            IssuePhase::Planning,
            JobKind::AgentPlan,
            agent_output("plan", "  "),
        ),
        (
            IssuePhase::Implementing,
            JobKind::AgentImplement,
            agent_output("blocked", "cannot build"),
        ),
    ];
    for (phase, kind, output) in cases {
        let s = issue_in(phase);
        let t = run(Permission::None, &s, &ok("x", kind, output));
        let d = assert_issue(&t, IssuePhase::Failed);
        assert_eq!(d.failed_phase, Some(phase));
    }
    // Commits that were not pushed cannot become a PR.
    let mut out = agent_output("done", "x");
    out["pushed"] = json!(false);
    let t = run(
        Permission::None,
        &issue_in(IssuePhase::Implementing),
        &ok("x", JobKind::AgentImplement, out),
    );
    assert!(
        assert_issue(&t, IssuePhase::Failed)
            .error
            .contains("not pushed")
    );
    // As can an unreadable PR result.
    let t = run(
        Permission::None,
        &issue_implementing_pr(),
        &ok("x", JobKind::OpenChangeRequest, json!({ "nope": 1 })),
    );
    assert_issue(&t, IssuePhase::Failed);
}

#[test]
fn issue_implement_uses_the_runs_branch_when_reported() {
    let mut out = agent_output("done", "x");
    out["branch"] = json!("enzarb/other");
    let t = run(
        Permission::None,
        &issue_in(IssuePhase::Implementing),
        &ok("x", JobKind::AgentImplement, out),
    );
    assert_eq!(
        job_of(&t, JobKind::OpenChangeRequest).2["spec"]["head_ref"],
        "enzarb/other"
    );
    // Without it, the conventional branch.
    let mut out = agent_output("done", "x");
    out.as_object_mut().unwrap().remove("branch");
    let t = run(
        Permission::None,
        &issue_in(IssuePhase::Implementing),
        &ok("x", JobKind::AgentImplement, out),
    );
    assert_eq!(
        job_of(&t, JobKind::OpenChangeRequest).2["spec"]["head_ref"],
        "enzarb/issue-42"
    );
}

#[test]
fn issue_implement_pr_title_is_conventional() {
    let title_for = |agent: Option<&str>| {
        let block = match agent {
            Some(t) => json!({ "decision": "done", "body": "x", "title": t }),
            None => json!({ "decision": "done", "body": "x" }),
        };
        let out = json!({
            "summary": format!("Done.\n\n```json\n{block}\n```\n"),
            "branch": "enzarb/issue-42",
            "head_sha": "sha2",
            "pushed": true,
        });
        let t = run(
            Permission::None,
            &issue_in(IssuePhase::Implementing),
            &ok("x", JobKind::AgentImplement, out),
        );
        job_of(&t, JobKind::OpenChangeRequest).2["spec"]["title"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        title_for(Some("fix(ui): handle empty")),
        "fix(ui): handle empty"
    );
    assert_eq!(title_for(Some("Add dark mode")), "feat(ui): add dark mode");
    assert_eq!(title_for(None), "feat(ui): add dark mode");
}

#[test]
fn issue_timers() {
    // A job deadline fails the busy phases and cancels the job.
    for phase in [
        IssuePhase::Triaging,
        IssuePhase::Planning,
        IssuePhase::Implementing,
    ] {
        let t = run(Permission::None, &issue_in(phase), &timer("t", Some(9)));
        assert_issue(&t, IssuePhase::Failed);
        assert!(cancels(&t));
    }
    // wake_at timers only mean something while awaiting answers.
    for phase in ALL_ISSUE_PHASES {
        if phase == IssuePhase::AwaitingAnswers {
            continue;
        }
        let s = issue_in(phase);
        rejected(&run(Permission::None, &s, &timer("t", None)), &s);
    }
    // A deadline for a job when nothing is running is stale.
    let s = issue_in(IssuePhase::AwaitingApproval);
    rejected(&run(Permission::None, &s, &timer("t", Some(1))), &s);
}

#[test]
fn issue_busy_and_finished_states_ignore_comments() {
    for phase in [
        IssuePhase::Triaging,
        IssuePhase::Planning,
        IssuePhase::Implementing,
        IssuePhase::Duplicate,
        IssuePhase::Done,
        IssuePhase::Failed,
        IssuePhase::Cancelled,
    ] {
        let s = issue_in(phase);
        rejected(
            &run(Permission::Write, &s, &comment_by("c", "alice", 1, "hi")),
            &s,
        );
    }
}

#[test]
fn issue_ignores_events_meant_for_other_workflows() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    let table = [
        opened("dup"),
        conn(
            "a",
            EventType::ChangeRequestOpened,
            pr_item("s"),
            actor("a"),
            None,
        ),
        conn(
            "b",
            EventType::ChangeRequestUpdated,
            pr_item("s"),
            actor("a"),
            None,
        ),
        conn(
            "c",
            EventType::ReviewSubmitted,
            pr_item("s"),
            actor("a"),
            None,
        ),
    ];
    for ev in table {
        rejected(&run(Permission::Write, &s, &ev), &s);
    }
}

#[test]
fn issue_merged_close_event_also_cancels() {
    let t = run(
        Permission::None,
        &issue_in(IssuePhase::AwaitingApproval),
        &closed("c", issue_item(), true),
    );
    assert_issue(&t, IssuePhase::Cancelled);
}

#[test]
fn long_errors_are_truncated() {
    let ev = EngineEvent {
        id: "f".into(),
        payload: EventPayload::JobFailed {
            job_id: 1,
            kind: JobKind::AgentPlan,
            error: "x".repeat(2000),
        },
    };
    let t = run(Permission::None, &issue_in(IssuePhase::Planning), &ev);
    let d = assert_issue(&t, IssuePhase::Failed);
    assert!(d.error.chars().count() < 600 && d.error.ends_with("..."));
}

// ---------------------------------------------------------------------------
// change_request workflow
// ---------------------------------------------------------------------------

fn pr_opened(id: &str, sha: &str) -> EngineEvent {
    conn(
        id,
        EventType::ChangeRequestOpened,
        pr_item(sha),
        actor("alice"),
        None,
    )
}

fn pr_updated(id: &str, sha: &str) -> EngineEvent {
    conn(
        id,
        EventType::ChangeRequestUpdated,
        pr_item(sha),
        actor("alice"),
        None,
    )
}

fn pr_comment(id: &str, who: &str, cid: u64, text: &str) -> EngineEvent {
    conn(
        id,
        EventType::CommentAdded,
        pr_item("sha1"),
        actor(who),
        Some((cid, text)),
    )
}

fn pr_review_comment(id: &str, who: &str, cid: u64, text: &str) -> EngineEvent {
    let mut e = pr_comment(id, who, cid, text);
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.comment.as_mut().unwrap().kind = enzarb_connector::CommentKind::Review;
    }
    e
}

fn review(id: &str, who: &str, state: &str, body: &str) -> EngineEvent {
    let mut e = conn(
        id,
        EventType::ReviewSubmitted,
        pr_item("sha1"),
        actor(who),
        None,
    );
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.review = Some(Review {
            id: 1,
            state: state.into(),
            body: body.into(),
        });
    }
    e
}

#[test]
fn cr_opened_starts_review_even_for_bots_and_strangers() {
    let new = WorkflowState::New(WorkflowKind::ChangeRequest);
    let mut ev = pr_opened("1", "sha1");
    if let EventPayload::Connector(e) = &mut ev.payload {
        e.data.actor = Actor {
            login: "dependabot[bot]".into(),
            id: 1,
            is_bot: true,
        };
    }
    let t = run_cr(Permission::None, &new, &ev);
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        (
            d.head_ref.as_str(),
            d.base_ref.as_str(),
            d.head_sha.as_str()
        ),
        ("feat/dark", "main", "sha1")
    );
    assert_eq!(progress_values(&t), vec!["Received", "Working"]);
    let input: AgentJobInput = serde_json::from_value(job_of(&t, JobKind::AgentReview).2).unwrap();
    assert!(!input.push);
    assert_eq!(input.branch, "feat/dark");
    assert!(input.prompt.contains("feat/dark") && input.prompt.contains("sha1"));

    // Missing base_ref falls back to the default branch.
    let mut item = pr_item("sha1");
    item.base_ref = None;
    let ev = conn("2", EventType::ChangeRequestOpened, item, actor("a"), None);
    let t = run_cr(Permission::None, &new, &ev);
    assert_eq!(assert_cr(&t, CrPhase::Reviewing).base_ref, "main");
}

#[test]
fn cr_review_decisions_all_await_changes() {
    for decision in ["approve", "request_changes", "comment"] {
        let t = run_cr_manual(
            Permission::None,
            &cr_in(CrPhase::Reviewing),
            &agent_ok_checks(
                "r",
                JobKind::AgentReview,
                decision,
                json!([{ "name": "cargo test", "status": "passed" }]),
            ),
        );
        let d = assert_cr(&t, CrPhase::AwaitingChanges);
        assert_eq!(d.last_review, decision);
        assert_eq!(d.reviewed_sha, "sha1");
        assert!(comment_bodies(&t)[0].contains(decision) && comment_bodies(&t)[0].contains("LGTM"));
        assert_eq!(progress_values(&t), vec!["Done"]);
    }
}

#[test]
fn cr_unverified_approve_is_downgraded_to_comment() {
    let cases = [
        json!([]),
        json!([{ "name": "cargo test", "status": "failed" }]),
        json!([
            { "name": "cargo fmt", "status": "passed" },
            { "name": "cargo test", "status": "not_run" }
        ]),
    ];
    for checks in cases {
        let t = run_cr_manual(
            Permission::None,
            &cr_in(CrPhase::Reviewing),
            &agent_ok_checks("r", JobKind::AgentReview, "approve", checks),
        );
        let d = assert_cr(&t, CrPhase::AwaitingChanges);
        assert_eq!(d.last_review, "comment");
        assert!(comment_bodies(&t)[0].contains("Downgraded"));
    }
    // Field absent entirely (older output shape) is unverified too.
    let t = run_cr_manual(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok("r", JobKind::AgentReview, "approve", "LGTM"),
    );
    assert_eq!(
        assert_cr(&t, CrPhase::AwaitingChanges).last_review,
        "comment"
    );
    // request_changes is never altered.
    let t = run_cr_manual(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok("r", JobKind::AgentReview, "request_changes", "no"),
    );
    assert_eq!(
        assert_cr(&t, CrPhase::AwaitingChanges).last_review,
        "request_changes"
    );
}

#[test]
fn cr_feedback_is_addressed_then_awaits_again() {
    let s = cr_in(CrPhase::AwaitingChanges);
    // Comment, mention and changes-requested review all start a round.
    let mention = conn(
        "m",
        EventType::Mentioned,
        pr_item("sha1"),
        actor("carol"),
        Some((12, "@enzarb fix the typo")),
    );
    let events = [
        pr_comment("c", "carol", 11, "rename this"),
        mention,
        review("v", "carol", "CHANGES_REQUESTED", "needs work"),
        review("v2", "carol", "commented", "a thought"),
    ];
    for ev in events {
        let t = run_cr(Permission::Write, &s, &ev);
        assert_cr(&t, CrPhase::AddressingComments);
        let input: AgentJobInput =
            serde_json::from_value(job_of(&t, JobKind::AgentAddressComments).2).unwrap();
        assert!(input.push);
        assert_eq!(input.branch, "feat/dark");
        assert_eq!(progress_values(&t), vec!["Received", "Working"]);
    }
    let t = run_cr(
        Permission::Write,
        &s,
        &pr_comment("c", "carol", 11, "rename this"),
    );
    assert_eq!(first_progress(&t).2["target"]["comment_id"], 11);
    let input: AgentJobInput =
        serde_json::from_value(job_of(&t, JobKind::AgentAddressComments).2).unwrap();
    assert!(input.prompt.contains("rename this"));

    // An inline review comment carries its kind into the progress target.
    let t = run_cr(
        Permission::Write,
        &s,
        &pr_review_comment("rc", "carol", 12, "rename this"),
    );
    assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(first_progress(&t).2["target"]["comment_id"], 12);
    assert_eq!(first_progress(&t).2["target"]["comment_kind"], "review");
    let t = run_cr(
        Permission::Write,
        &s,
        &pr_comment("c2", "carol", 13, "rename this"),
    );
    assert_eq!(
        first_progress(&t).2["target"]["comment_kind"],
        "conversation"
    );

    // Empty changes-requested review mentions the inline comments.
    let t = run_cr(
        Permission::Write,
        &s,
        &review("v", "carol", "changes_requested", " "),
    );
    assert!(
        assert_cr(&t, CrPhase::AddressingComments)
            .trigger_text
            .contains("inline")
    );

    // Done: commits pushed, the agent's head is reviewed.
    let busy = t.next;
    let mut out = agent_output("done", "Fixed it");
    out["head_sha"] = json!("sha9");
    let t = run_cr(
        Permission::None,
        &busy,
        &ok("d", JobKind::AgentAddressComments, out),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        (d.head_sha.as_str(), d.reviewed_sha.as_str()),
        ("sha9", "sha1")
    );
    assert!(kinds(&t).contains(&JobKind::AgentReview));
    assert_eq!(comment_bodies(&t), vec!["Fixed it".to_string()]);
    // The round is done and the review of the pushed head has started.
    assert_eq!(progress_values(&t), vec!["Done", "Working"]);
    // The webhook for that same push while it is under review is a duplicate.
    rejected(
        &run_cr(Permission::None, &t.next, &pr_updated("u", "sha9")),
        &t.next,
    );
    // A different push restarts the review.
    let t2 = run_cr(Permission::None, &t.next, &pr_updated("u2", "sha10"));
    assert_cr(&t2, CrPhase::Reviewing);
    assert_eq!(t2.effects[0], Effect::CancelJobs);

    // Once that review is posted, the same push is a duplicate too.
    let t3 = run_cr_manual(
        Permission::None,
        &t.next,
        &agent_ok("r", JobKind::AgentReview, "request_changes", "nits"),
    );
    let d = assert_cr(&t3, CrPhase::AwaitingChanges);
    assert_eq!(d.reviewed_sha, "sha9");
    assert!(!kinds(&t3).contains(&JobKind::AgentAddressComments));
    rejected(
        &run_cr(Permission::None, &t3.next, &pr_updated("u3", "sha9")),
        &t3.next,
    );
}

#[test]
fn cr_address_without_a_push_awaits_changes() {
    let busy = cr_in(CrPhase::AddressingComments);
    // `agent_ok` reports a new head (`sha2`), so build the output without one.
    let mut out = agent_output("done", "nothing to do");
    out.as_object_mut().unwrap().remove("head_sha");
    let t = run_cr(
        Permission::None,
        &busy,
        &ok("d", JobKind::AgentAddressComments, out),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert!(!kinds(&t).contains(&JobKind::AgentReview));
    let mut out = agent_output("done", "no change");
    out["head_sha"] = json!("sha1");
    let t = run_cr(
        Permission::None,
        &busy,
        &ok("d2", JobKind::AgentAddressComments, out),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert!(!kinds(&t).contains(&JobKind::AgentReview));
}

#[test]
fn cr_feedback_authorization_table() {
    let s = cr_in(CrPhase::AwaitingChanges);
    let table: Vec<(Permission, EngineEvent, bool)> = vec![
        (Permission::Write, pr_comment("1", "carol", 1, "x"), true),
        (Permission::Read, pr_comment("2", "bob", 1, "x"), false),
        (Permission::None, pr_comment("3", "alice", 1, "x"), false),
        (
            Permission::Write,
            pr_comment("4", "enzarb[bot]", 1, "x"),
            false,
        ),
        (
            Permission::Write,
            review("5", "enzarb[bot]", "commented", "x"),
            false,
        ),
        (
            Permission::Read,
            review("6", "bob", "changes_requested", "x"),
            false,
        ),
        (
            Permission::Write,
            review("7", "carol", "approved", "x"),
            true,
        ),
        (
            Permission::Write,
            review("8", "carol", "dismissed", "x"),
            false,
        ),
        (
            Permission::Write,
            review("9", "carol", "commented", ""),
            false,
        ),
    ];
    for (perm, ev, want) in table {
        let t = run_cr(perm, &s, &ev);
        assert_eq!(t.accepted, want, "{perm:?} {:?}: {}", ev.payload, t.note);
        if !want {
            rejected(&t, &s);
        }
    }
    // A review event without its payload is malformed.
    let mut bad = review("10", "carol", "commented", "x");
    if let EventPayload::Connector(c) = &mut bad.payload {
        c.data.review = None;
    }
    rejected(&run_cr(Permission::Write, &s, &bad), &s);
}

#[test]
fn cr_feedback_during_a_round_is_queued_and_run_next() {
    let busy = cr_in(CrPhase::AddressingComments);
    let t = run_cr(
        Permission::Write,
        &busy,
        &pr_comment("c2", "carol", 21, "and this"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(d.pending.as_ref().unwrap().text, "and this");
    assert_eq!(kinds(&t), vec![JobKind::SetProgress]);
    assert_eq!(progress_values(&t), vec!["Received"]);
    // Strangers cannot queue work.
    rejected(
        &run_cr(Permission::Read, &busy, &pr_comment("c3", "bob", 22, "x")),
        &busy,
    );

    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok("d", JobKind::AgentAddressComments, "done", "round 1"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(d.pending, None);
    assert_eq!(d.trigger_comment_id, Some(21));
    assert_eq!(progress_values(&t), vec!["Done", "Working"]);
    assert!(kinds(&t).contains(&JobKind::AgentAddressComments));
    let mut out = agent_output("done", "round 2");
    out["head_sha"] = json!("sha5");
    let t = run_cr(
        Permission::None,
        &t.next,
        &ok("d2", JobKind::AgentAddressComments, out),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        (d.head_sha.as_str(), d.reviewed_sha.as_str()),
        ("sha5", "sha1")
    );
    assert!(kinds(&t).contains(&JobKind::AgentReview));
}

#[test]
fn cr_new_commits_trigger_a_re_review() {
    let s = cr_in(CrPhase::AwaitingChanges);
    let t = run_cr(Permission::None, &s, &pr_updated("u", "sha2"));
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        (d.head_sha.as_str(), d.reviewed_sha.as_str()),
        ("sha2", "sha2")
    );
    assert!(kinds(&t).contains(&JobKind::AgentReview));
    assert!(!cancels(&t));
    // Same commit again: duplicate delivery.
    rejected(&run_cr(Permission::None, &s, &pr_updated("u2", "sha1")), &s);

    // A push during a review restarts it.
    let t2 = run_cr(Permission::None, &t.next, &pr_updated("u3", "sha3"));
    assert_cr(&t2, CrPhase::Reviewing);
    assert_eq!(t2.effects[0], Effect::CancelJobs);
    rejected(
        &run_cr(Permission::None, &t.next, &pr_updated("u4", "sha2")),
        &t.next,
    );

    // A push during addressing is noted, with no new work.
    let t3 = run_cr(
        Permission::None,
        &cr_in(CrPhase::AddressingComments),
        &pr_updated("u5", "sha7"),
    );
    let d = assert_cr(&t3, CrPhase::AddressingComments);
    assert_eq!(d.head_sha, "sha7");
    assert!(t3.effects.is_empty());

    for phase in [
        CrPhase::Merged,
        CrPhase::Closed,
        CrPhase::Failed,
        CrPhase::Cancelled,
    ] {
        let s = cr_in(phase);
        rejected(&run_cr(Permission::None, &s, &pr_updated("u6", "sha8")), &s);
    }
}

#[test]
fn cr_close_and_merge_from_every_state() {
    for phase in ALL_CR_PHASES {
        let s = cr_in(phase);
        let finished = matches!(
            phase,
            CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled
        );
        for (merged, want) in [(true, CrPhase::Merged), (false, CrPhase::Closed)] {
            let t = run_cr(Permission::None, &s, &closed("c", pr_item("sha1"), merged));
            if finished {
                rejected(&t, &s);
            } else {
                assert_cr(&t, want);
                assert!(cancels(&t));
                assert_eq!(wake(&t), Some(None));
            }
        }
    }
}

#[test]
fn cr_ui_cancel() {
    for phase in ALL_CR_PHASES {
        let s = cr_in(phase);
        let t = run_cr(
            Permission::Write,
            &s,
            &ui("c", "carol", UiActionKind::Cancel),
        );
        if matches!(
            phase,
            CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled
        ) {
            rejected(&t, &s);
        } else {
            assert_cr(&t, CrPhase::Cancelled);
            assert!(cancels(&t) && comment_bodies(&t)[0].contains("@carol"));
        }
        rejected(
            &run_cr(Permission::Read, &s, &ui("c2", "bob", UiActionKind::Cancel)),
            &s,
        );
    }
}

#[test]
fn cr_failures_timers_and_retry() {
    for (phase, kind) in [
        (CrPhase::Reviewing, JobKind::AgentReview),
        (CrPhase::AddressingComments, JobKind::AgentAddressComments),
        (CrPhase::ResolvingConflicts, JobKind::AgentResolveConflicts),
    ] {
        let s = cr_in(phase);
        let t = run_cr(Permission::None, &s, &failed("f", kind));
        let d = assert_cr(&t, CrPhase::Failed);
        assert_eq!(d.failed_phase, Some(phase));
        assert_eq!(progress_values(&t), vec!["Failed"]);
        let r = run_cr(
            Permission::Write,
            &t.next,
            &ui("r", "carol", UiActionKind::Retry),
        );
        assert_cr(&r, phase);
        assert!(kinds(&r).contains(&kind));
        rejected(
            &run_cr(
                Permission::Read,
                &t.next,
                &ui("r", "bob", UiActionKind::Retry),
            ),
            &t.next,
        );

        // Deadline expiry.
        let t = run_cr(Permission::None, &s, &timer("t", Some(1)));
        assert_cr(&t, CrPhase::Failed);
        assert!(cancels(&t));
    }
    // Failed without a remembered phase retries the review.
    let mut s = cr_in(CrPhase::Failed);
    if let WorkflowState::ChangeRequest(c) = &mut s {
        c.data.failed_phase = None;
    }
    assert_cr(
        &run_cr(
            Permission::Write,
            &s,
            &ui("r", "carol", UiActionKind::Retry),
        ),
        CrPhase::Reviewing,
    );
    for phase in [CrPhase::AwaitingChanges, CrPhase::Merged] {
        let s = cr_in(phase);
        rejected(
            &run_cr(
                Permission::Write,
                &s,
                &ui("r", "carol", UiActionKind::Retry),
            ),
            &s,
        );
    }
    // wake_at timers do not exist for change requests.
    for phase in ALL_CR_PHASES {
        let s = cr_in(phase);
        rejected(&run_cr(Permission::None, &s, &timer("t", None)), &s);
    }
    // A deadline outside a run is stale.
    let s = cr_in(CrPhase::AwaitingChanges);
    rejected(&run_cr(Permission::None, &s, &timer("t", Some(1))), &s);
}

#[test]
fn cr_job_results_stale_ack_and_bad_output() {
    for phase in ALL_CR_PHASES {
        let s = cr_in(phase);
        for kind in [JobKind::SetProgress, JobKind::PostComment] {
            let t = run_cr(Permission::None, &s, &ok("a", kind, json!({})));
            assert!(t.accepted && t.effects.is_empty());
            let t = run_cr(Permission::None, &s, &failed("a", kind));
            assert!(t.accepted && t.effects.is_empty());
        }
    }
    let table = [
        (CrPhase::AwaitingChanges, JobKind::AgentReview),
        (CrPhase::Reviewing, JobKind::AgentAddressComments),
        (CrPhase::Merged, JobKind::AgentReview),
        (CrPhase::AddressingComments, JobKind::AgentTriage),
    ];
    for (phase, kind) in table {
        let s = cr_in(phase);
        rejected(
            &run_cr(Permission::None, &s, &agent_ok("x", kind, "done", "b")),
            &s,
        );
        rejected(&run_cr(Permission::None, &s, &failed("y", kind)), &s);
    }
    // Garbage output and a "blocked" round fail the item.
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &ok("x", JobKind::AgentReview, json!({})),
    );
    assert_cr(&t, CrPhase::Failed);
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::AddressingComments),
        &agent_ok("x", JobKind::AgentAddressComments, "blocked", "conflict"),
    );
    assert!(assert_cr(&t, CrPhase::Failed).error.contains("conflict"));
}

#[test]
fn cr_reopen_and_other_irrelevant_events() {
    for phase in ALL_CR_PHASES {
        let s = cr_in(phase);
        let t = run_cr(Permission::None, &s, &pr_opened("o", "sha5"));
        if matches!(phase, CrPhase::Closed | CrPhase::Cancelled) {
            let d = assert_cr(&t, CrPhase::Reviewing);
            assert_eq!(d.head_sha, "sha5");
        } else {
            rejected(&t, &s);
        }
        // `/approve` is a review approval while the item awaits changes (human_approval.rs).
        let mut events = vec![opened("w")];
        if phase != CrPhase::AwaitingChanges {
            events.push(approval_comment("a", "carol"));
            events.push(ui("b", "carol", UiActionKind::Approve));
        }
        for ev in events {
            rejected(&run_cr(Permission::Write, &s, &ev), &s);
        }
    }
    // Comments on finished items are not acted on.
    for phase in [
        CrPhase::Merged,
        CrPhase::Closed,
        CrPhase::Failed,
        CrPhase::Cancelled,
    ] {
        let s = cr_in(phase);
        rejected(
            &run_cr(Permission::Write, &s, &pr_comment("c", "carol", 1, "x")),
            &s,
        );
    }
}

// ---------------------------------------------------------------------------
// mention workflow
// ---------------------------------------------------------------------------

fn mention_ev(id: &str, who: &str, item: ItemRef) -> EngineEvent {
    conn(
        id,
        EventType::Mentioned,
        item,
        actor(who),
        Some((5, "@enzarb how does X work?")),
    )
}

#[test]
fn mention_runs_one_task_and_replies() {
    let new = WorkflowState::New(WorkflowKind::Mention);
    let t = run(
        Permission::Write,
        &new,
        &mention_ev("1", "carol", issue_item()),
    );
    let d = assert_mention(&t, MentionPhase::Running);
    assert_eq!(d.trigger_comment_id, Some(5));
    assert_eq!(progress_values(&t), vec!["Received", "Working"]);
    assert_eq!(first_progress(&t).2["target"]["comment_id"], 5);
    let input: AgentJobInput = serde_json::from_value(job_of(&t, JobKind::AgentTask).2).unwrap();
    assert!(!input.push && input.base_ref == "main");
    assert!(input.prompt.contains("how does X work"));

    let t = run(
        Permission::None,
        &t.next,
        &agent_ok("2", JobKind::AgentTask, "reply", "Like so"),
    );
    assert_mention(&t, MentionPhase::Done);
    assert_eq!(comment_bodies(&t), vec!["Like so".to_string()]);
    assert_eq!(progress_values(&t), vec!["Done"]);

    // A later mention starts another task.
    let t = run(
        Permission::Write,
        &t.next,
        &mention_ev("3", "carol", issue_item()),
    );
    assert_mention(&t, MentionPhase::Running);
}

#[test]
fn mention_in_the_item_body_targets_the_item_and_prs_use_their_branch() {
    let new = WorkflowState::New(WorkflowKind::Mention);
    let mut ev = mention_ev("1", "carol", pr_item("sha1"));
    if let EventPayload::Connector(e) = &mut ev.payload {
        e.data.comment = None;
        e.data.item.body = "@enzarb summarize this".into();
    }
    let t = run_cr(Permission::Write, &new, &ev);
    let d = assert_mention(&t, MentionPhase::Running);
    assert!(d.change_request && d.trigger_text == "@enzarb summarize this");
    assert_eq!(first_progress(&t).2["target"]["comment_id"], Value::Null);
    let input: AgentJobInput = serde_json::from_value(job_of(&t, JobKind::AgentTask).2).unwrap();
    assert_eq!(input.base_ref, "feat/dark");
    assert_eq!(input.context.item_kind, "change request");
}

#[test]
fn mention_authorization() {
    let new = WorkflowState::New(WorkflowKind::Mention);
    for perm in [Permission::None, Permission::Read, Permission::Triage] {
        rejected(
            &run(perm, &new, &mention_ev("1", "bob", issue_item())),
            &new,
        );
    }
    rejected(
        &run(
            Permission::Write,
            &new,
            &mention_ev("2", "enzarb[bot]", issue_item()),
        ),
        &new,
    );
    let s = mention_in(MentionPhase::Done);
    rejected(
        &run(Permission::Read, &s, &mention_ev("3", "bob", issue_item())),
        &s,
    );
    rejected(
        &run(
            Permission::Write,
            &s,
            &mention_ev("4", "enzarb[bot]", issue_item()),
        ),
        &s,
    );
    // Plain comments are not mentions.
    rejected(
        &run(Permission::Write, &s, &comment_by("5", "carol", 1, "hi")),
        &s,
    );
}

#[test]
fn mention_busy_cancelled_closed_and_cancel() {
    let running = mention_in(MentionPhase::Running);
    rejected(
        &run(
            Permission::Write,
            &running,
            &mention_ev("1", "carol", issue_item()),
        ),
        &running,
    );
    let cancelled = mention_in(MentionPhase::Cancelled);
    rejected(
        &run(
            Permission::Write,
            &cancelled,
            &mention_ev("2", "carol", issue_item()),
        ),
        &cancelled,
    );

    for phase in ALL_MENTION_PHASES {
        let s = mention_in(phase);
        let t = run(Permission::None, &s, &closed("c", issue_item(), false));
        let t2 = run(
            Permission::Write,
            &s,
            &ui("u", "carol", UiActionKind::Cancel),
        );
        if phase == MentionPhase::Cancelled {
            rejected(&t, &s);
            rejected(&t2, &s);
        } else {
            assert_mention(&t, MentionPhase::Cancelled);
            assert!(cancels(&t));
            assert_mention(&t2, MentionPhase::Cancelled);
            assert!(comment_bodies(&t2)[0].contains("@carol"));
        }
        rejected(
            &run(Permission::Read, &s, &ui("u2", "bob", UiActionKind::Cancel)),
            &s,
        );
        rejected(
            &run(
                Permission::Write,
                &s,
                &ui("u3", "bob", UiActionKind::Approve),
            ),
            &s,
        );
    }
}

#[test]
fn mention_failures_and_retry() {
    let running = mention_in(MentionPhase::Running);
    for t in [
        run(Permission::None, &running, &failed("f", JobKind::AgentTask)),
        run(
            Permission::None,
            &running,
            &agent_ok("b", JobKind::AgentTask, "blocked", "no access"),
        ),
        run(
            Permission::None,
            &running,
            &ok("g", JobKind::AgentTask, json!({ "summary": "x" })),
        ),
        run(Permission::None, &running, &timer("t", Some(1))),
    ] {
        assert_mention(&t, MentionPhase::Failed);
        assert_eq!(progress_values(&t), vec!["Failed"]);
        let r = run(
            Permission::Write,
            &t.next,
            &ui("r", "carol", UiActionKind::Retry),
        );
        assert_mention(&r, MentionPhase::Running);
        assert!(kinds(&r).contains(&JobKind::AgentTask));
        rejected(
            &run(
                Permission::Read,
                &t.next,
                &ui("r", "bob", UiActionKind::Retry),
            ),
            &t.next,
        );
    }
    assert!(cancels(&run(
        Permission::None,
        &running,
        &timer("t", Some(1))
    )));
    for phase in [
        MentionPhase::Running,
        MentionPhase::Done,
        MentionPhase::Cancelled,
    ] {
        let s = mention_in(phase);
        rejected(
            &run(
                Permission::Write,
                &s,
                &ui("r", "carol", UiActionKind::Retry),
            ),
            &s,
        );
    }
}

#[test]
fn mention_stale_results_and_timers() {
    for phase in ALL_MENTION_PHASES {
        let s = mention_in(phase);
        for kind in [JobKind::SetProgress, JobKind::PostComment] {
            assert!(run(Permission::None, &s, &ok("a", kind, json!({}))).accepted);
            assert!(run(Permission::None, &s, &failed("a", kind)).accepted);
        }
        if phase != MentionPhase::Running {
            rejected(
                &run(
                    Permission::None,
                    &s,
                    &agent_ok("x", JobKind::AgentTask, "reply", "b"),
                ),
                &s,
            );
            rejected(
                &run(Permission::None, &s, &failed("y", JobKind::AgentTask)),
                &s,
            );
            rejected(&run(Permission::None, &s, &timer("t", Some(1))), &s);
        }
        rejected(&run(Permission::None, &s, &timer("t", None)), &s);
    }
    let running = mention_in(MentionPhase::Running);
    rejected(
        &run(Permission::None, &running, &failed("y", JobKind::AgentPlan)),
        &running,
    );
    rejected(
        &run(
            Permission::None,
            &running,
            &agent_ok("x", JobKind::AgentPlan, "plan", "b"),
        ),
        &running,
    );
}

// ---------------------------------------------------------------------------
// Cross-cutting
// ---------------------------------------------------------------------------

#[test]
fn rejected_transitions_never_have_effects_for_any_state_event_pair() {
    let states: Vec<WorkflowState> = ALL_ISSUE_PHASES
        .iter()
        .map(|p| issue_in(*p))
        .chain(ALL_CR_PHASES.iter().map(|p| cr_in(*p)))
        .chain(ALL_MENTION_PHASES.iter().map(|p| mention_in(*p)))
        .collect();
    let events = vec![
        opened("1"),
        comment_by("2", "alice", 1, "x"),
        approval_comment("3", "carol"),
        approval_label("4", "carol"),
        closed("5", issue_item(), false),
        pr_opened("6", "s"),
        pr_updated("7", "s"),
        review("8", "carol", "changes_requested", "x"),
        mention_ev("9", "carol", issue_item()),
        ui("10", "carol", UiActionKind::Approve),
        ui("11", "carol", UiActionKind::Cancel),
        ui("12", "carol", UiActionKind::Retry),
        timer("13", None),
        timer("14", Some(1)),
        failed("15", JobKind::AgentPlan),
        agent_ok("16", JobKind::AgentTriage, "ready", "x"),
    ];
    for perm in [Permission::None, Permission::Write] {
        for s in &states {
            for ev in &events {
                let t = run(perm, s, ev);
                if !t.accepted {
                    assert!(t.effects.is_empty(), "{s:?} / {ev:?}");
                    assert_eq!(&t.next, s);
                }
                // State always survives a persistence round trip.
                let back = WorkflowState::from_row(
                    t.next.workflow().as_str(),
                    WORKFLOW_VERSION,
                    &t.next.state_name(),
                    &t.next.state_data(),
                )
                .unwrap();
                assert_eq!(back, t.next);
                // Effects are storable as jsonb.
                serde_json::to_value(&t.effects).unwrap();
            }
        }
    }
}

#[test]
fn idempotency_keys_are_unique_within_a_transition_and_deterministic() {
    use std::collections::HashSet;
    let new = WorkflowState::New(WorkflowKind::Issue);
    let mut state = new;
    for ev in [
        opened("1"),
        agent_ok("2", JobKind::AgentTriage, "ready", "s"),
        agent_ok("3", JobKind::AgentPlan, "plan", "p"),
        approval_comment("4", "carol"),
        agent_ok("5", JobKind::AgentImplement, "done", "b"),
        ok("6", JobKind::OpenChangeRequest, pr_opened_output()),
    ] {
        let t = run(Permission::Write, &state, &ev);
        let keys: Vec<String> = jobs(&t).into_iter().map(|j| j.2).collect();
        let set: HashSet<_> = keys.iter().collect();
        assert_eq!(set.len(), keys.len());
        for k in &keys {
            assert!(
                k.starts_with(&format!("github:enzarb/enzarb-ui#42:{}:", ev.id)),
                "{k}"
            );
        }
        assert_eq!(t, run(Permission::Write, &state, &ev));
        state = t.next;
    }
    assert!(state.is_terminal());
}

#[test]
fn state_serialization_round_trip_and_errors() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    assert_eq!(s.workflow(), WorkflowKind::Issue);
    assert_eq!(s.state_name(), "AwaitingApproval");
    assert_eq!(s.state_data()["plan"], "PLAN");
    let back = WorkflowState::from_row("issue", 1, "AwaitingApproval", &s.state_data()).unwrap();
    assert_eq!(back, s);

    assert_eq!(
        cr_in(CrPhase::AddressingComments).state_name(),
        "AddressingComments"
    );
    assert_eq!(mention_in(MentionPhase::Running).state_name(), "Running");
    let new = WorkflowState::New(WorkflowKind::Mention);
    assert_eq!(
        (new.state_name().as_str(), new.state_data()),
        ("New", json!({}))
    );
    assert_eq!(new.workflow().as_str(), "mention");

    // Older rows missing newer fields still load.
    let old = WorkflowState::from_row("issue", 1, "Triaging", &json!({ "number": 3 })).unwrap();
    match old {
        WorkflowState::Issue(i) => assert_eq!(i.data.number, 3),
        _ => panic!(),
    }
    assert!(WorkflowState::from_row("issue", 1, "Triaging", &json!({})).is_ok());
    assert!(WorkflowState::from_row("change_request", 1, "Reviewing", &json!({})).is_ok());
    assert!(WorkflowState::from_row("mention", 1, "Done", &json!({})).is_ok());

    for (w, v, st, d) in [
        ("issue", 2, "Triaging", json!({})),
        ("issue", 1, "Nope", json!({})),
        ("issue", 1, "Triaging", json!({ "number": "x" })),
        ("nope", 1, "Triaging", json!({})),
    ] {
        let e = WorkflowState::from_row(w, v, st, &d).unwrap_err();
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn terminal_states() {
    for p in ALL_ISSUE_PHASES {
        let want = matches!(
            p,
            IssuePhase::Done | IssuePhase::Duplicate | IssuePhase::Cancelled
        );
        assert_eq!(issue_in(p).is_terminal(), want, "{p:?}");
    }
    for p in ALL_CR_PHASES {
        let want = matches!(p, CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled);
        assert_eq!(cr_in(p).is_terminal(), want, "{p:?}");
    }
    for p in ALL_MENTION_PHASES {
        assert_eq!(mention_in(p).is_terminal(), p == MentionPhase::Cancelled);
    }
    assert!(!WorkflowState::New(WorkflowKind::Issue).is_terminal());
}

#[test]
fn job_kind_names_match_the_documented_kinds() {
    let all = [
        (JobKind::SetProgress, "set_progress"),
        (JobKind::PostComment, "post_comment"),
        (JobKind::OpenChangeRequest, "open_change_request"),
        (JobKind::AgentTriage, "agent_triage"),
        (JobKind::AgentPlan, "agent_plan"),
        (JobKind::AgentImplement, "agent_implement"),
        (JobKind::AgentReview, "agent_review"),
        (JobKind::AgentAddressComments, "agent_address_comments"),
        (JobKind::AgentResolveConflicts, "agent_resolve_conflicts"),
        (JobKind::AgentTask, "agent_task"),
        (JobKind::CheckMergeReadiness, "check_merge_readiness"),
        (JobKind::MergeChangeRequest, "merge_change_request"),
        (JobKind::CreateSubIssues, "create_sub_issues"),
    ];
    for (k, name) in all {
        assert_eq!(k.as_str(), name);
        assert_eq!(serde_json::to_value(k).unwrap(), json!(name));
        assert_eq!(k.is_agent(), name.starts_with("agent_"));
    }
}

// -- parsing events from rows -------------------------------------------------

#[test]
fn from_row_parses_every_event_type() {
    // Connector events, via their real wire format.
    for ty in EventType::ALL {
        let e = Event::new(
            "github",
            "d1",
            ty,
            now(),
            EventData {
                repo: repo(),
                actor: actor("alice"),
                item: issue_item(),
                comment: None,
                merged: None,
                review: None,
                ci: None,
                label: None,
            },
        );
        let payload = serde_json::to_value(&e).unwrap();
        let ev = EngineEvent::from_row("9", ty.as_str(), &payload).unwrap();
        assert_eq!(ev.id, "9");
        assert_eq!(ev.payload, EventPayload::Connector(Box::new(e)));
    }

    let ev = EngineEvent::from_row(
        "1",
        "JobSucceeded",
        &json!({ "job_id": 5, "kind": "agent_plan", "output": { "summary": "x" } }),
    )
    .unwrap();
    assert!(matches!(
        ev.payload,
        EventPayload::JobSucceeded {
            job_id: 5,
            kind: JobKind::AgentPlan,
            ..
        }
    ));
    let ev = EngineEvent::from_row(
        "1",
        "JobFailed",
        &json!({ "job_id": 5, "kind": "post_comment", "error": "e" }),
    )
    .unwrap();
    assert!(matches!(
        ev.payload,
        EventPayload::JobFailed {
            kind: JobKind::PostComment,
            ..
        }
    ));
    let ev = EngineEvent::from_row("1", "TimerFired", &json!({})).unwrap();
    assert_eq!(ev.payload, EventPayload::TimerFired { job_id: None });
    let ev = EngineEvent::from_row("1", "TimerFired", &json!({ "job_id": 3 })).unwrap();
    assert_eq!(ev.payload, EventPayload::TimerFired { job_id: Some(3) });

    // UI actions, as written by workflows_api.submit_action.
    let ui_payload = json!({ "actor": { "login": "carol", "id": 1, "is_bot": false }, "action": "approve", "data": {} });
    for (ty, action) in [
        ("ApprovalGranted", UiActionKind::Approve),
        ("CancelRequested", UiActionKind::Cancel),
        ("RetryRequested", UiActionKind::Retry),
        ("MergeRequested", UiActionKind::Merge),
        ("RepoRemoved", UiActionKind::RepoRemoved),
        ("CommentAdded", UiActionKind::Comment),
    ] {
        let mut p = ui_payload.clone();
        p["action"] = json!(match action {
            UiActionKind::Comment => "comment",
            UiActionKind::Approve => "approve",
            UiActionKind::Cancel => "cancel",
            UiActionKind::Retry => "retry",
            UiActionKind::Merge => "merge",
            UiActionKind::RepoRemoved => "repo-removed",
        });
        match EngineEvent::from_row("1", ty, &p).unwrap().payload {
            EventPayload::Ui(u) => assert_eq!(u.action, action),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn from_row_errors() {
    let e = EngineEvent::from_row("1", "Nope", &json!({})).unwrap_err();
    assert_eq!(e, ParseError::UnknownType("Nope".into()));
    assert!(e.to_string().contains("Nope"));
    for (ty, p) in [
        ("JobSucceeded", json!({})),
        ("TimerFired", json!({ "job_id": "x" })),
        ("RetryRequested", json!({})),
        ("ApprovalGranted", json!({ "action": "approve" })),
        ("CommentAdded", json!({})),
    ] {
        let e = EngineEvent::from_row("1", ty, &p).unwrap_err();
        assert!(matches!(e, ParseError::BadPayload(..)));
        assert!(e.to_string().contains(ty));
    }
}

#[test]
fn ctx_defaults() {
    let c = Config::default();
    assert!(c.nudge_after_secs > 0 && c.stale_after_secs > 0 && c.agent_timeout_secs > 0);
    let t = Triggers::default();
    assert!(t.issues && t.change_requests && t.mentions);
}

#[test]
fn mention_retries_failed_issue_like_ui_retry() {
    let mention = |who: &str| {
        conn(
            "m",
            EventType::Mentioned,
            issue_item(),
            actor(who),
            Some((9, "@enzarb try again")),
        )
    };
    for phase in [IssuePhase::Failed, IssuePhase::Stale] {
        let s = issue_in(phase);
        let t = run(Permission::Write, &s, &mention("carol"));
        assert_issue(&t, IssuePhase::Triaging);
        assert!(kinds(&t).contains(&JobKind::AgentTriage));
        // No write access: still rejected. A plain comment on Failed: still rejected.
        rejected(&run(Permission::Read, &s, &mention("mallory")), &s);
    }
    let failed = issue_in(IssuePhase::Failed);
    rejected(
        &run(
            Permission::Write,
            &failed,
            &comment_by("c", "carol", 1, "hi"),
        ),
        &failed,
    );
    for phase in [
        IssuePhase::Done,
        IssuePhase::Duplicate,
        IssuePhase::Cancelled,
    ] {
        let s = issue_in(phase);
        rejected(&run(Permission::Write, &s, &mention("carol")), &s);
    }
}

#[test]
fn mention_retries_failed_change_request() {
    let s = cr_in(CrPhase::Failed);
    let m = conn(
        "m",
        EventType::Mentioned,
        pr_item("sha1"),
        actor("carol"),
        Some((9, "@enzarb try again")),
    );
    assert_cr(&run_cr(Permission::Write, &s, &m), CrPhase::Reviewing);
    rejected(&run_cr(Permission::Read, &s, &m), &s);
}

#[test]
fn mention_reruns_the_failed_job_of_a_change_request() {
    let m = conn(
        "m",
        EventType::Mentioned,
        pr_item("sha1"),
        actor("carol"),
        Some((9, "@enzarb please retry this issue.")),
    );
    for (failed_phase, phase, kind) in [
        (CrPhase::Reviewing, CrPhase::Reviewing, JobKind::AgentReview),
        (
            CrPhase::AddressingComments,
            CrPhase::AddressingComments,
            JobKind::AgentAddressComments,
        ),
    ] {
        let mut s = CrState {
            phase: CrPhase::Failed,
            data: cr_data(),
        };
        s.data.failed_phase = Some(failed_phase);
        let s = WorkflowState::ChangeRequest(s);
        let t = run_cr(Permission::Write, &s, &m);
        let d = assert_cr(&t, phase);
        assert_eq!(d.failed_phase, None);
        assert!(kinds(&t).contains(&kind), "{:?}", kinds(&t));
        rejected(&run_cr(Permission::Read, &s, &m), &s);
        // Also as a plain comment event carrying the mention.
        let c = conn(
            "c",
            EventType::CommentAdded,
            pr_item("sha1"),
            actor("carol"),
            Some((10, "@enzarb retry")),
        );
        let t = run_cr(Permission::Write, &s, &c);
        if t.accepted {
            assert_cr(&t, phase);
        }
    }
}

#[test]
fn author_mention_on_stale_keeps_the_answer() {
    let s = issue_in(IssuePhase::Stale);
    let author = match &s {
        WorkflowState::Issue(i) => i.data.author.clone(),
        _ => unreachable!(),
    };
    let m = conn(
        "m",
        EventType::Mentioned,
        issue_item(),
        actor(&author),
        Some((9, "@enzarb here is my answer")),
    );
    let d = assert_issue(&run(Permission::Write, &s, &m), IssuePhase::Triaging);
    assert_eq!(d.trigger_text, "@enzarb here is my answer");
    assert_eq!(d.trigger_comment_id, Some(9));
}

#[test]
fn approve_comment_pair_implements_instead_of_replanning() {
    let new = WorkflowState::New(WorkflowKind::Issue);
    let mut state = run(Permission::None, &new, &opened("1")).next;
    state = run(
        Permission::None,
        &state,
        &agent_ok("2", JobKind::AgentTriage, "ready", "SUMMARY"),
    )
    .next;
    let t = run(
        Permission::None,
        &state,
        &agent_ok("3", JobKind::AgentPlan, "plan", "## Plan\nDo it"),
    );
    assert_issue(&t, IssuePhase::AwaitingApproval);
    let state = t.next;

    // The connector emits CommentAdded then ApprovalGranted for one `/approve`.
    let c = conn(
        "4a",
        EventType::CommentAdded,
        issue_item(),
        actor("carol"),
        Some((900, "/approve")),
    );
    let t = run(Permission::Write, &state, &c);
    rejected(&t, &state);
    let m = conn(
        "4m",
        EventType::Mentioned,
        issue_item(),
        actor("carol"),
        Some((900, "@enzarb\n/approve")),
    );
    rejected(&run(Permission::Write, &state, &m), &state);
    let t = run(Permission::Write, &state, &approval_comment("4b", "carol"));
    assert_issue(&t, IssuePhase::Implementing);
    job_of(&t, JobKind::AgentImplement);
}
