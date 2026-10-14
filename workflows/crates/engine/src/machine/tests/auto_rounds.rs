//! Automatic fix rounds on enzarb's own review findings, and feedback queued during a run.

use super::*;

fn cr_state_of(t: &Transition) -> &change_request::CrState {
    match &t.next {
        WorkflowState::ChangeRequest(s) => s,
        other => panic!("not a change request: {other:?}"),
    }
}

fn reason(t: &Transition) -> &'static str {
    change_request::waiting_reason(cr_state_of(t))
}

#[test]
fn review_findings_start_an_automatic_round() {
    for decision in ["request_changes", "comment"] {
        let t = run_cr(
            Permission::None,
            &cr_in(CrPhase::Reviewing),
            &agent_ok("r", JobKind::AgentReview, decision, "fix the nits"),
        );
        let d = assert_cr(&t, CrPhase::AddressingComments);
        assert_eq!(d.auto_rounds, 1);
        assert!(d.auto_trigger);
        assert_eq!(d.trigger_text, "fix the nits");
        assert_eq!(d.trigger_comment_id, None);
        assert_eq!(d.last_review, decision);
        assert!(kinds(&t).contains(&JobKind::AgentAddressComments));
        assert_eq!(reason(&t), "enzarb_fixing");
    }
    // An unverified approve is downgraded and its note is part of the findings.
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok("r", JobKind::AgentReview, "approve", "LGTM"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert!(d.trigger_text.contains("Downgraded"));
}

#[test]
fn verified_approve_starts_no_automatic_round() {
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok_checks(
            "r",
            JobKind::AgentReview,
            "approve",
            json!([{ "name": "cargo test", "status": "passed" }]),
        ),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.auto_rounds, 0);
    assert!(!kinds(&t).contains(&JobKind::AgentAddressComments));
}

#[test]
fn automatic_rounds_are_capped_and_reset_by_a_human() {
    let mut state = cr_in(CrPhase::Reviewing);
    for round in 1..=3u32 {
        let t = run_cr(
            Permission::None,
            &state,
            &agent_ok("r", JobKind::AgentReview, "request_changes", "again"),
        );
        assert_eq!(
            assert_cr(&t, CrPhase::AddressingComments).auto_rounds,
            round
        );
        let mut out = agent_output("done", "fixed");
        out["head_sha"] = json!(format!("sha{}", round + 1));
        let t = run_cr(
            Permission::None,
            &t.next,
            &ok("d", JobKind::AgentAddressComments, out),
        );
        assert_cr(&t, CrPhase::Reviewing);
        state = t.next;
    }
    let t = run_cr(
        Permission::None,
        &state,
        &agent_ok("r", JobKind::AgentReview, "request_changes", "again"),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.auto_rounds, 3);
    assert!(!kinds(&t).contains(&JobKind::AgentAddressComments));
    assert_eq!(reason(&t), "round_limit");

    let t = run_cr(
        Permission::Write,
        &t.next,
        &pr_comment("c", "carol", 5, "please"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(
        (d.auto_rounds, d.auto_trigger, d.auto_capped),
        (0, false, false)
    );
}

#[test]
fn automatic_rounds_can_be_disabled() {
    let t = run_cr_manual(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok("r", JobKind::AgentReview, "request_changes", "no"),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(reason(&t), "human_review");
}

#[test]
fn automatic_round_without_a_push_does_not_loop() {
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok("r", JobKind::AgentReview, "request_changes", "no"),
    );
    let t = run_cr(
        Permission::None,
        &t.next,
        &ok(
            "d",
            JobKind::AgentAddressComments,
            json!({ "summary": "Done.\n\n```json\n{\"decision\":\"done\",\"body\":\"nothing to change\"}\n```\n" }),
        ),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.auto_rounds, 1);
    assert!(!kinds(&t).contains(&JobKind::AgentAddressComments));
    assert!(!kinds(&t).contains(&JobKind::AgentReview));
    assert_eq!(reason(&t), "human_review");
}

#[test]
fn feedback_during_review_is_queued_and_goes_before_the_findings() {
    let busy = cr_in(CrPhase::Reviewing);
    let t = run_cr(
        Permission::Write,
        &busy,
        &pr_comment("c", "carol", 21, "also this"),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.pending.as_ref().unwrap().text, "also this");
    assert_eq!(progress_values(&t), vec!["Received"]);
    // Strangers still cannot queue work.
    rejected(
        &run_cr(Permission::Read, &busy, &pr_comment("c3", "bob", 22, "x")),
        &busy,
    );
    // A new head restarts the review but keeps the queued feedback.
    let t2 = run_cr(Permission::None, &t.next, &pr_updated("u", "sha2"));
    assert_eq!(
        assert_cr(&t2, CrPhase::Reviewing).pending.unwrap().text,
        "also this"
    );

    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok("r", JobKind::AgentReview, "request_changes", "findings"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(d.trigger_text, "also this");
    assert_eq!(d.trigger_comment_id, Some(21));
    assert_eq!(d.pending, None);
    assert_eq!(d.auto_rounds, 0);
    assert_eq!(d.last_review, "request_changes");
    assert!(comment_bodies(&t)[0].contains("findings"));
}

#[test]
fn feedback_during_a_merge_is_queued_then_run_or_dropped() {
    let merging = WorkflowState::ChangeRequest(CrState {
        phase: CrPhase::Merging,
        data: CrData {
            last_review: "approve".into(),
            ..cr_data()
        },
    });
    let t = run_cr(
        Permission::Write,
        &merging,
        &pr_comment("c", "carol", 21, "wait"),
    );
    let d = assert_cr(&t, CrPhase::Merging);
    assert_eq!(d.pending.as_ref().unwrap().text, "wait");
    let queued = t.next;

    for out in [
        json!({ "outcome": "head_moved" }),
        json!({ "outcome": "not_mergeable", "reason": "conflicts" }),
    ] {
        let t = run_cr(
            Permission::None,
            &queued,
            &ok("m", JobKind::MergeChangeRequest, out),
        );
        let d = assert_cr(&t, CrPhase::AddressingComments);
        assert_eq!(d.trigger_text, "wait");
        assert_eq!(d.pending, None);
        assert!(kinds(&t).contains(&JobKind::AgentAddressComments));
    }

    let t = run_cr(
        Permission::None,
        &queued,
        &ok(
            "m",
            JobKind::MergeChangeRequest,
            json!({ "outcome": "merged", "actor": "carol" }),
        ),
    );
    assert_eq!(assert_cr(&t, CrPhase::Merged).pending, None);
}

#[test]
fn waiting_reason_per_state() {
    let with = |phase, data: CrData| change_request::waiting_reason(&CrState { phase, data });
    let reviewed = CrData {
        last_review: "request_changes".into(),
        ..cr_data()
    };
    assert_eq!(
        with(CrPhase::AwaitingChanges, reviewed.clone()),
        "human_review"
    );
    assert_eq!(with(CrPhase::AwaitingChanges, cr_data()), "");
    assert_eq!(
        with(
            CrPhase::AwaitingChanges,
            CrData {
                auto_capped: true,
                ..reviewed.clone()
            }
        ),
        "round_limit"
    );
    assert_eq!(
        with(
            CrPhase::AddressingComments,
            CrData {
                auto_trigger: true,
                ..cr_data()
            }
        ),
        "enzarb_fixing"
    );
    assert_eq!(with(CrPhase::AddressingComments, cr_data()), "");
    assert_eq!(with(CrPhase::ReadyToMerge, reviewed), "");
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &agent_ok("r", JobKind::AgentReview, "request_changes", "x"),
    );
    assert_eq!(t.next.state_data()["waiting_reason"], "enzarb_fixing");
}

#[test]
fn state_without_auto_round_fields_still_loads() {
    let d: CrData =
        serde_json::from_value(json!({ "number": 7, "last_review": "comment" })).unwrap();
    assert_eq!(
        (d.auto_rounds, d.auto_trigger, d.auto_capped),
        (0, false, false)
    );
}
