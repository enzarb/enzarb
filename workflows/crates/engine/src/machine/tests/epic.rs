//! Epics: a big issue is broken into sub-issues, never implemented as one change request.

use super::*;

const EPIC_COMMENT: u64 = 77;

/// A valid epic breakdown: `ui` waits for `api`.
fn breakdown() -> String {
    "## Summary\nSplit in two.\n\n## Sub-issues\n- api\n- ui (after api)\n\n```enzarb-subissues\n[\
     {\"key\":\"ui\",\"title\":\"Show it\",\"body\":\"UI part\",\"depends_on\":[\"api\"]},\
     {\"key\":\"api\",\"title\":\"Add the API\",\"body\":\"API part\",\"depends_on\":[]}]\n```\n\n\
     Reply with feedback to revise this breakdown, or comment /approve to create the sub-issues."
        .into()
}

fn epic_plan_ok(id: &str) -> EngineEvent {
    agent_ok(id, JobKind::AgentPlan, "plan", &breakdown())
}

fn epic_data() -> IssueData {
    IssueData {
        epic: true,
        sub_issues: parse_epic_plan(&breakdown(), 8).unwrap(),
        plan: breakdown(),
        ..issue_data()
    }
}

fn epic_in(phase: IssuePhase) -> WorkflowState {
    WorkflowState::Issue(IssueState {
        phase,
        data: epic_data(),
    })
}

fn child(n: u64, key: &str, title: &str) -> Value {
    json!({
        "key": key,
        "item": format!("github:enzarb/enzarb-ui#{n}"),
        "title": title,
        "url": format!("https://github.com/enzarb/enzarb-ui/issues/{n}"),
    })
}

fn created() -> EngineEvent {
    ok(
        "created",
        JobKind::CreateSubIssues,
        json!({ "children": [child(50, "api", "Add the API"), child(51, "ui", "Show it")] }),
    )
}

fn dep(n: u64, problem: &str) -> OpenDependency {
    OpenDependency {
        key: format!("github:enzarb/enzarb-ui#{n}"),
        label: format!("#{n}"),
        url: String::new(),
        problem: problem.into(),
    }
}

fn deps_changed(id: &str) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::DependenciesChanged,
    }
}

fn run_with_deps(state: &WorkflowState, deps: Vec<OpenDependency>, ev: &EngineEvent) -> Transition {
    let c = Ctx {
        open_dependencies: deps,
        ..ctx(Permission::Write)
    };
    transition(&c, state, ev)
}

fn slash_epic(id: &str, who: &str) -> EngineEvent {
    comment_by(id, who, EPIC_COMMENT, "This is too big.\n/epic")
}

fn epic_label(id: &str, who: &str) -> EngineEvent {
    conn(
        id,
        EventType::WorkItemLabeled,
        issue_item(),
        actor(who),
        None,
    )
    .with_label("epic")
}

trait WithLabel {
    fn with_label(self, label: &str) -> Self;
}

impl WithLabel for EngineEvent {
    fn with_label(mut self, label: &str) -> Self {
        if let EventPayload::Connector(c) = &mut self.payload {
            c.data.label = Some(label.into());
        }
        self
    }
}

fn plan_prompt(t: &Transition) -> String {
    let (_, _, input) = job_of(t, JobKind::AgentPlan);
    input["prompt"].as_str().unwrap().to_string()
}

#[test]
fn triage_epic_plans_a_breakdown_then_creates_sub_issues_and_waits_for_them() {
    let s = issue_in(IssuePhase::Triaging);
    let t = run(
        Permission::None,
        &s,
        &agent_ok("1", JobKind::AgentTriage, "epic", "too big"),
    );
    let d = assert_issue(&t, IssuePhase::Planning);
    assert!(d.epic);
    assert_eq!(d.triage_summary, "too big");
    assert!(plan_prompt(&t).contains("enzarb-subissues"));

    let t = run(
        Permission::None,
        &WorkflowState::Issue(IssueState {
            phase: IssuePhase::Planning,
            data: d,
        }),
        &epic_plan_ok("2"),
    );
    let d = assert_issue(&t, IssuePhase::AwaitingApproval);
    let keys: Vec<_> = d.sub_issues.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(keys, ["api", "ui"], "stored in creation order");
    assert!(comment_bodies(&t)[0].contains("enzarb-subissues"));
    let awaiting = t.next.clone();

    // Approval creates the sub-issues; it does not implement anything.
    let t = run(
        Permission::Write,
        &awaiting,
        &approval_comment("3", "carol"),
    );
    assert_issue(&t, IssuePhase::Decomposing);
    assert_eq!(kinds(&t).iter().filter(|k| k.is_agent()).count(), 0);
    let (_, _, input) = job_of(&t, JobKind::CreateSubIssues);
    assert_eq!(input["depth"], 1);
    assert_eq!(input["item"], "github:enzarb/enzarb-ui#42");
    assert_eq!(input["sub_issues"][0]["key"], "api");
    assert_eq!(wake(&t), Some(None));
    let decomposing = t.next.clone();

    // The job result moves it to waiting; the children are remembered.
    let c = Ctx {
        open_dependencies: vec![dep(50, ""), dep(51, "")],
        ..ctx(Permission::None)
    };
    let t = transition(&c, &decomposing, &created());
    let d = assert_issue(&t, IssuePhase::AwaitingChildren);
    assert_eq!(d.children.len(), 2);
    assert_eq!(d.open_dependencies.len(), 2);
    assert!(comment_bodies(&t)[0].contains("#50") && comment_bodies(&t)[0].contains("#51"));
    let waiting = t.next.clone();

    // One child done: still waiting, nothing posted.
    let t = run_with_deps(&waiting, vec![dep(51, "")], &deps_changed("5"));
    let d = assert_issue(&t, IssuePhase::AwaitingChildren);
    assert_eq!(d.open_dependencies.len(), 1);
    assert!(t.effects.is_empty());

    // All done: the epic is done, with no change request.
    let t = run_with_deps(&waiting, vec![], &deps_changed("6"));
    let d = assert_issue(&t, IssuePhase::Done);
    assert_eq!(d.change_request, None);
    assert!(comment_bodies(&t)[0].contains("All 2 sub-issues are done"));
    assert!(!has(&t, |e| matches!(e, Effect::StartWorkflow { .. })));
    assert!(!kinds(&t).contains(&JobKind::OpenChangeRequest));
    assert_eq!(wake(&t), Some(None));
    // The issue is closed as completed, after the summary comment.
    let (close_p, _, input) = job_of(&t, JobKind::CloseIssue);
    assert_eq!(input["reason"], "completed");
    assert_eq!(input["item"], json!(ctx(Permission::None).work_item));
    let (comment_p, _, _) = job_of(&t, JobKind::PostComment);
    assert!(close_p < comment_p, "the comment runs first");
    let done = t.next.clone();

    // The bot's own close coming back, and the close job's result, change nothing.
    rejected(
        &run(Permission::None, &done, &closed("7", issue_item(), false)),
        &done,
    );
    for ev in [
        ok("8", JobKind::CloseIssue, json!({})),
        failed("9", JobKind::CloseIssue),
    ] {
        let t = run(Permission::None, &done, &ev);
        assert_eq!(t.next, done);
        assert!(t.effects.is_empty());
    }
}

#[test]
fn ordinary_issues_never_queue_close_issue() {
    for phase in ALL_ISSUE_PHASES {
        let s = issue_in(phase);
        let t = run(Permission::Write, &s, &approval_comment("1", "carol"));
        assert!(!kinds(&t).contains(&JobKind::CloseIssue), "{phase:?}");
    }
}

#[test]
fn an_epic_never_implements_or_opens_a_change_request() {
    // Walk every epic phase with every kind of event that could start implementing.
    for phase in ALL_ISSUE_PHASES {
        let s = epic_in(phase);
        for ev in [
            approval_comment("a", "carol"),
            ui("b", "carol", UiActionKind::Approve),
            ui("c", "carol", UiActionKind::Retry),
            deps_changed("d"),
            created(),
        ] {
            let t = run(Permission::Write, &s, &ev);
            assert!(!kinds(&t).contains(&JobKind::AgentImplement), "{phase:?}");
            assert!(
                !kinds(&t).contains(&JobKind::OpenChangeRequest),
                "{phase:?}"
            );
            assert!(
                !has(&t, |e| matches!(e, Effect::StartWorkflow { .. })),
                "{phase:?}"
            );
        }
    }
}

#[test]
fn slash_epic_from_a_writer_is_remembered_while_triage_runs() {
    let s = issue_in(IssuePhase::Triaging);
    let t = run(Permission::Write, &s, &slash_epic("1", "carol"));
    let d = assert_issue(&t, IssuePhase::Triaging);
    assert!(d.epic);
    assert_eq!(progress_values(&t), ["Received"]);
    assert!(kinds(&t).iter().all(|k| *k == JobKind::SetProgress));
    // Triage then says `ready`: the plan is still a breakdown.
    let t = run(
        Permission::None,
        &t.next,
        &agent_ok("2", JobKind::AgentTriage, "ready", "fine"),
    );
    let d = assert_issue(&t, IssuePhase::Planning);
    assert!(d.epic);
    assert!(plan_prompt(&t).contains("enzarb-subissues"));
}

#[test]
fn slash_epic_in_awaiting_answers_and_stale_sets_the_flag_only() {
    for phase in [IssuePhase::AwaitingAnswers, IssuePhase::Stale] {
        let s = issue_in(phase);
        let t = run(Permission::Write, &s, &slash_epic("1", "carol"));
        let d = assert_issue(&t, phase);
        assert!(d.epic, "{phase:?}");
        assert!(!kinds(&t).iter().any(|k| k.is_agent()), "{phase:?}");
    }
}

#[test]
fn slash_epic_in_awaiting_approval_replans_as_an_epic() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    let t = run(Permission::Write, &s, &slash_epic("1", "carol"));
    let d = assert_issue(&t, IssuePhase::Planning);
    assert!(d.epic);
    assert_eq!(d.trigger_comment_id, Some(EPIC_COMMENT));
    let prompt = plan_prompt(&t);
    assert!(prompt.contains("enzarb-subissues") && prompt.contains("This is too big."));
    assert!(prompt.contains("PLAN"), "the previous plan is passed on");
}

#[test]
fn the_epic_label_from_a_writer_asks_for_an_epic() {
    let s = issue_in(IssuePhase::AwaitingApproval);
    let t = run(Permission::Write, &s, &epic_label("1", "carol"));
    let d = assert_issue(&t, IssuePhase::Planning);
    assert!(d.epic);
    assert!(plan_prompt(&t).contains("labeled this issue `epic`"));
    let s = issue_in(IssuePhase::Triaging);
    let d = assert_issue(
        &run(Permission::Maintain, &s, &epic_label("2", "carol")),
        IssuePhase::Triaging,
    );
    assert!(d.epic);
}

#[test]
fn epic_requests_from_non_writers_do_nothing() {
    for phase in [
        IssuePhase::Triaging,
        IssuePhase::AwaitingAnswers,
        IssuePhase::AwaitingApproval,
        IssuePhase::Stale,
    ] {
        let s = issue_in(phase);
        for perm in [Permission::None, Permission::Read, Permission::Triage] {
            // The label is rejected outright.
            rejected(&run(perm, &s, &epic_label("1", "mallory")), &s);
            // `/epic` from a stranger is not even feedback.
            let t = run(perm, &s, &slash_epic("2", "mallory"));
            assert!(
                !t.accepted || !matches!(&t.next, WorkflowState::Issue(i) if i.data.epic),
                "{phase:?}/{perm:?}"
            );
        }
    }
    // The author without write access: the comment is plan feedback, not an epic request.
    let s = issue_in(IssuePhase::AwaitingApproval);
    let t = run(Permission::Read, &s, &slash_epic("3", "alice"));
    let d = assert_issue(&t, IssuePhase::Planning);
    assert!(!d.epic);
    assert!(!plan_prompt(&t).contains("enzarb-subissues"));
}

#[test]
fn only_the_epic_label_counts_and_the_bot_cannot_apply_it() {
    let s = issue_in(IssuePhase::Triaging);
    rejected(
        &run(
            Permission::Write,
            &s,
            &epic_label("1", "carol").with_label("bug"),
        ),
        &s,
    );
    let mut bot = epic_label("2", "enzarb[bot]");
    if let EventPayload::Connector(c) = &mut bot.payload {
        c.data.actor.is_bot = true;
    }
    rejected(&run(Permission::Write, &s, &bot), &s);
    // Case does not matter.
    let d = assert_issue(
        &run(
            Permission::Write,
            &s,
            &epic_label("3", "carol").with_label("Epic"),
        ),
        IssuePhase::Triaging,
    );
    assert!(d.epic);
}

#[test]
fn an_epic_request_is_accepted_once_and_not_while_busy_or_finished() {
    let s = epic_in(IssuePhase::AwaitingApproval);
    rejected(&run(Permission::Write, &s, &slash_epic("1", "carol")), &s);
    rejected(&run(Permission::Write, &s, &epic_label("2", "carol")), &s);
    for phase in [
        IssuePhase::Planning,
        IssuePhase::Implementing,
        IssuePhase::Decomposing,
        IssuePhase::AwaitingChildren,
        IssuePhase::Duplicate,
        IssuePhase::Done,
        IssuePhase::Failed,
        IssuePhase::Cancelled,
    ] {
        let s = issue_in(phase);
        rejected(&run(Permission::Write, &s, &slash_epic("3", "carol")), &s);
        rejected(&run(Permission::Write, &s, &epic_label("4", "carol")), &s);
    }
}

#[test]
fn the_depth_limit_keeps_a_sub_issue_from_becoming_an_epic() {
    let data = IssueData {
        epic_depth: 1,
        epic_parent: Some("github:enzarb/enzarb-ui#10".into()),
        ..issue_data()
    };
    let s = WorkflowState::Issue(IssueState {
        phase: IssuePhase::Triaging,
        data,
    });
    // Triage says `epic`: it is planned as one change request.
    let t = run(
        Permission::None,
        &s,
        &agent_ok("1", JobKind::AgentTriage, "epic", "large"),
    );
    let d = assert_issue(&t, IssuePhase::Planning);
    assert!(!d.epic);
    assert!(d.triage_summary.contains("cannot be split further"));
    assert!(!plan_prompt(&t).contains("enzarb-subissues"));
    // The triage prompt tells the agent not to answer `epic` here.
    let t = run(Permission::Write, &s, &slash_epic("2", "carol"));
    rejected(&t, &s);
    rejected(&run(Permission::Write, &s, &epic_label("3", "carol")), &s);
    // A top-level issue is told it may.
    let mut c = ctx(Permission::None);
    let top = issue_in(IssuePhase::AwaitingAnswers);
    let t = transition(&c, &top, &comment_by("4", "alice", 1, "answer"));
    let (_, _, input) = job_of(&t, JobKind::AgentTriage);
    assert!(
        input["prompt"]
            .as_str()
            .unwrap()
            .contains("Epics allowed for this item: `yes`")
    );
    let t = transition(
        &c,
        &WorkflowState::Issue(IssueState {
            phase: IssuePhase::AwaitingAnswers,
            data: IssueData {
                epic_depth: 1,
                ..issue_data()
            },
        }),
        &comment_by("5", "alice", 1, "answer"),
    );
    let (_, _, input) = job_of(&t, JobKind::AgentTriage);
    assert!(
        input["prompt"]
            .as_str()
            .unwrap()
            .contains("Epics allowed for this item: `no`")
    );
    // The limit is configurable: depth 0 with a limit of 0 is not allowed either.
    c.config.max_epic_depth = 0;
    rejected(
        &transition(
            &c,
            &issue_in(IssuePhase::Triaging),
            &slash_epic("6", "carol"),
        ),
        &issue_in(IssuePhase::Triaging),
    );
}

#[test]
fn a_breakdown_over_the_cap_or_with_a_cycle_fails_the_plan() {
    let s = WorkflowState::Issue(IssueState {
        phase: IssuePhase::Planning,
        data: IssueData {
            epic: true,
            ..issue_data()
        },
    });
    let mut small = ctx(Permission::None);
    small.config.max_epic_children = 1;
    let t = transition(&small, &s, &epic_plan_ok("1"));
    let d = assert_issue(&t, IssuePhase::Failed);
    assert_eq!(d.failed_phase, Some(IssuePhase::Planning));
    assert!(d.error.contains("more than 1"), "{}", d.error);

    let cyc = "```enzarb-subissues\n[{\"key\":\"a\",\"title\":\"A\",\"depends_on\":[\"b\"]},\
               {\"key\":\"b\",\"title\":\"B\",\"depends_on\":[\"a\"]}]\n```";
    let t = run(
        Permission::None,
        &s,
        &agent_ok("2", JobKind::AgentPlan, "plan", cyc),
    );
    let d = assert_issue(&t, IssuePhase::Failed);
    assert!(d.error.contains("cycle"), "{}", d.error);
    assert!(has(&t, |e| matches!(e, Effect::Job { .. })));

    let none = run(
        Permission::None,
        &s,
        &agent_ok("3", JobKind::AgentPlan, "plan", "no list"),
    );
    assert_issue(&none, IssuePhase::Failed);
    // A plain (non-epic) plan needs no list.
    let plain = issue_in(IssuePhase::Planning);
    assert_issue(
        &run(
            Permission::None,
            &plain,
            &agent_ok("4", JobKind::AgentPlan, "plan", "plain"),
        ),
        IssuePhase::AwaitingApproval,
    );
}

#[test]
fn cancel_close_and_retry_in_the_epic_phases() {
    for phase in [IssuePhase::Decomposing, IssuePhase::AwaitingChildren] {
        let s = epic_in(phase);
        let t = run(
            Permission::Write,
            &s,
            &ui("1", "carol", UiActionKind::Cancel),
        );
        assert_issue(&t, IssuePhase::Cancelled);
        assert!(cancels(&t));
        let t = run(Permission::None, &s, &closed("2", issue_item(), false));
        assert_issue(&t, IssuePhase::Cancelled);
        assert!(cancels(&t));
        // Nothing to retry while it is healthy.
        rejected(
            &run(
                Permission::Write,
                &s,
                &ui("3", "carol", UiActionKind::Retry),
            ),
            &s,
        );
    }
    // A failed job lands in Failed and remembers where.
    let s = epic_in(IssuePhase::Decomposing);
    let t = run(Permission::None, &s, &failed("4", JobKind::CreateSubIssues));
    let d = assert_issue(&t, IssuePhase::Failed);
    assert_eq!(d.failed_phase, Some(IssuePhase::Decomposing));
    assert!(d.error.contains("create_sub_issues"));
    let failed_state = t.next.clone();
    // Retry runs the job again (the connector finds the issues made so far by their markers).
    let t = run(
        Permission::Write,
        &failed_state,
        &ui("5", "carol", UiActionKind::Retry),
    );
    let d = assert_issue(&t, IssuePhase::Decomposing);
    assert!(d.error.is_empty());
    let (_, key, input) = job_of(&t, JobKind::CreateSubIssues);
    assert_eq!(input["sub_issues"].as_array().unwrap().len(), 2);
    assert!(key.ends_with(":create_sub_issues"));
    rejected(
        &run(
            Permission::Read,
            &failed_state,
            &ui("6", "bob", UiActionKind::Retry),
        ),
        &failed_state,
    );
    // A deadline for a running job fails it and cancels what is left.
    let t = run(Permission::None, &s, &timer("7", Some(1)));
    assert_issue(&t, IssuePhase::Failed);
    assert!(cancels(&t));
}

#[test]
fn stale_or_unreadable_sub_issue_results_are_not_applied() {
    // Not in Decomposing: stale.
    for phase in [
        IssuePhase::AwaitingApproval,
        IssuePhase::AwaitingChildren,
        IssuePhase::Done,
    ] {
        let s = epic_in(phase);
        rejected(&run(Permission::None, &s, &created()), &s);
    }
    let s = epic_in(IssuePhase::Decomposing);
    let t = run(
        Permission::None,
        &s,
        &ok("1", JobKind::CreateSubIssues, json!({ "children": [] })),
    );
    assert_issue(&t, IssuePhase::Failed);
    let t = run(
        Permission::None,
        &s,
        &ok("2", JobKind::CreateSubIssues, json!("junk")),
    );
    assert_issue(&t, IssuePhase::Failed);
}

#[test]
fn comments_while_creating_or_waiting_are_not_feedback() {
    for phase in [IssuePhase::Decomposing, IssuePhase::AwaitingChildren] {
        let s = epic_in(phase);
        rejected(
            &run(Permission::Write, &s, &comment_by("1", "alice", 1, "hello")),
            &s,
        );
    }
}

#[test]
fn closed_sub_issues_park_the_epic_until_a_maintainer_decides() {
    let waiting = epic_in(IssuePhase::AwaitingChildren);
    // One child is still in progress and one was closed: keep waiting, say nothing.
    let t = run_with_deps(
        &waiting,
        vec![
            dep(50, "dependency #50 closed without completing"),
            dep(51, ""),
        ],
        &deps_changed("1"),
    );
    let d = assert_issue(&t, IssuePhase::AwaitingChildren);
    assert!(!d.children_notified && t.effects.is_empty());
    let busy = t.next.clone();
    rejected(
        &run(Permission::Write, &busy, &approval_comment("2", "carol")),
        &busy,
    );

    // Only the closed one is left: comment once.
    let stuck = vec![dep(50, "dependency #50 closed without completing")];
    let t = run_with_deps(&waiting, stuck.clone(), &deps_changed("3"));
    let d = assert_issue(&t, IssuePhase::AwaitingChildren);
    assert!(d.children_notified);
    let body = &comment_bodies(&t)[0];
    assert!(body.contains("closed without completing") && body.contains("/approve"));
    let parked = t.next.clone();
    let again = run_with_deps(&parked, stuck, &deps_changed("4"));
    assert!(again.effects.is_empty(), "no second comment");

    // A maintainer closes the epic with /approve (or the UI button); others cannot.
    let t = run(Permission::Write, &parked, &approval_comment("5", "carol"));
    assert_issue(&t, IssuePhase::Done);
    assert!(comment_bodies(&t)[0].contains("closed"));
    assert_eq!(job_of(&t, JobKind::CloseIssue).2["reason"], "not_planned");
    let t = run(
        Permission::Write,
        &parked,
        &ui("6", "carol", UiActionKind::Approve),
    );
    assert_issue(&t, IssuePhase::Done);
    rejected(
        &run(Permission::Read, &parked, &approval_comment("7", "bob")),
        &parked,
    );
}

#[test]
fn dependency_changes_only_matter_while_waiting_for_sub_issues() {
    for phase in ALL_ISSUE_PHASES {
        if phase == IssuePhase::AwaitingChildren {
            continue;
        }
        let s = epic_in(phase);
        rejected(&run_with_deps(&s, vec![], &deps_changed("1")), &s);
    }
}

// -- sub-issues opened by the bot -----------------------------------------------------

fn parent() -> EpicParent {
    EpicParent {
        key: WorkItemKey::new(repo(), 10),
        depth: 0,
        sub_keys: vec!["api".into(), "ui".into()],
    }
}

fn child_item(marker: &str) -> ItemRef {
    ItemRef {
        number: 50,
        title: "Add the API".into(),
        body: format!("API part\n\n{marker}\n<!-- enzarb:epic-depth=1 -->"),
        author: "enzarb[bot]".into(),
        url: "https://github.com/enzarb/enzarb-ui/issues/50".into(),
        ..issue_item()
    }
}

fn bot_opened(marker: &str) -> EngineEvent {
    let who = Actor {
        login: "enzarb[bot]".into(),
        id: 9000,
        is_bot: true,
    };
    conn(
        "b",
        EventType::WorkItemOpened,
        child_item(marker),
        who,
        None,
    )
}

fn child_ctx(parent: Option<EpicParent>) -> Ctx {
    Ctx {
        epic_parent: parent,
        ..ctx_for(50, Permission::None)
    }
}

const GOOD_MARKER: &str = "<!-- enzarb:epic=github:enzarb/enzarb-ui#10:api -->";

#[test]
fn a_bot_authored_sub_issue_of_a_known_epic_starts_the_normal_flow() {
    let new = WorkflowState::New(WorkflowKind::Issue);
    let t = transition(&child_ctx(Some(parent())), &new, &bot_opened(GOOD_MARKER));
    let d = assert_issue(&t, IssuePhase::Triaging);
    assert_eq!(d.epic_depth, 1, "depth comes from the stored parent");
    assert_eq!(d.epic_parent.as_deref(), Some("github:enzarb/enzarb-ui#10"));
    assert!(!d.epic);
    // It is triaged like any issue: nothing is implemented before a human approves.
    assert_eq!(kinds(&t).iter().filter(|k| k.is_agent()).count(), 1);
    assert!(kinds(&t).contains(&JobKind::AgentTriage));
    let (_, _, input) = job_of(&t, JobKind::AgentTriage);
    assert!(
        input["prompt"]
            .as_str()
            .unwrap()
            .contains("Epics allowed for this item: `no`")
    );
}

#[test]
fn a_bot_authored_issue_is_ignored_unless_every_check_passes() {
    let new = WorkflowState::New(WorkflowKind::Issue);
    let cases: Vec<(&str, Ctx, EngineEvent)> = vec![
        (
            "no marker",
            child_ctx(Some(parent())),
            bot_opened("nothing"),
        ),
        ("no stored parent", child_ctx(None), bot_opened(GOOD_MARKER)),
        (
            "key not in the plan",
            child_ctx(Some(parent())),
            bot_opened("<!-- enzarb:epic=github:enzarb/enzarb-ui#10:other -->"),
        ),
        (
            "another parent",
            child_ctx(Some(parent())),
            bot_opened("<!-- enzarb:epic=github:enzarb/enzarb-ui#11:api -->"),
        ),
        (
            "parent in another repo",
            child_ctx(Some(parent())),
            bot_opened("<!-- enzarb:epic=github:enzarb/other#10:api -->"),
        ),
    ];
    for (name, c, ev) in cases {
        rejected(&transition(&c, &new, &ev), &new);
        let _ = name;
    }
    // A different bot account, even with a perfect marker.
    let mut other = bot_opened(GOOD_MARKER);
    if let EventPayload::Connector(c) = &mut other.payload {
        c.data.actor.login = "dependabot[bot]".into();
    }
    rejected(&transition(&child_ctx(Some(parent())), &new, &other), &new);
    // A human who copies the marker is just a human: normal flow, not a sub-issue.
    let mut human = bot_opened(GOOD_MARKER);
    if let EventPayload::Connector(c) = &mut human.payload {
        c.data.actor = actor("mallory");
    }
    let t = transition(&child_ctx(None), &new, &human);
    let d = assert_issue(&t, IssuePhase::Triaging);
    assert_eq!((d.epic_depth, d.epic_parent), (0, None));
}

#[test]
fn state_saved_before_epics_still_loads() {
    let old = json!({ "number": 42, "title": "t", "plan": "p", "author": "alice" });
    let s = WorkflowState::from_row("issue", WORKFLOW_VERSION, "AwaitingApproval", &old).unwrap();
    let WorkflowState::Issue(i) = &s else {
        panic!()
    };
    assert!(!i.data.epic && i.data.sub_issues.is_empty() && i.data.children.is_empty());
    assert_eq!(i.data.epic_depth, 0);
    // And the new phases round-trip by name.
    for p in [IssuePhase::Decomposing, IssuePhase::AwaitingChildren] {
        let st = epic_in(p);
        let back = WorkflowState::from_row(
            "issue",
            WORKFLOW_VERSION,
            &st.state_name(),
            &st.state_data(),
        )
        .unwrap();
        assert_eq!(back, st);
    }
    assert_eq!(epic_in(IssuePhase::Decomposing).state_name(), "Decomposing");
    assert_eq!(
        epic_in(IssuePhase::AwaitingChildren).state_name(),
        "AwaitingChildren"
    );
}
