//! A human `approved` review on a change request.

use super::*;
use crate::machine::change_request::{WaitingOn, readiness, wait};

fn waiting(d: &CrData, phase: CrPhase) -> (WaitingOn, &'static str) {
    let w = wait(phase, d, &readiness(d)).expect("wait");
    (w.waiting_on, w.reason)
}

fn unreviewed_ci_pending() -> CrData {
    CrData {
        last_review: String::new(),
        reviewed_sha: String::new(),
        ci: CiState::Pending,
        ci_sha: "sha1".into(),
        mergeable: Some(true),
        ..cr_data()
    }
}

fn state(phase: CrPhase, d: CrData) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState { phase, data: d })
}

fn approval(id: &str) -> EngineEvent {
    review(id, "carol", "approved", "")
}

#[test]
fn approval_with_ci_pending_waits_for_ci() {
    let s = state(CrPhase::AwaitingChanges, unreviewed_ci_pending());
    let t = run_cr(Permission::Write, &s, &approval("1"));
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.last_review, "approve");
    assert_eq!(d.reviewed_sha, d.head_sha);
    assert_eq!(
        waiting(&d, CrPhase::AwaitingChanges),
        (WaitingOn::Ci, "ci_pending")
    );
}

#[test]
fn approval_then_green_ci_is_ready_to_merge() {
    let s = state(CrPhase::AwaitingChanges, unreviewed_ci_pending());
    let t = run_cr(Permission::Write, &s, &approval("1"));
    assert_cr(&t, CrPhase::AwaitingChanges);
    let t = run_cr(
        Permission::None,
        &t.next,
        &ok(
            "c",
            JobKind::CheckMergeReadiness,
            json!({ "head_sha": "sha1", "ci": "success", "mergeable": true, "mergeable_state": "clean" }),
        ),
    );
    assert_cr(&t, CrPhase::ReadyToMerge);
}

#[test]
fn approval_with_green_ci_is_ready_to_merge() {
    let d = CrData {
        ci: CiState::Success,
        ..unreviewed_ci_pending()
    };
    let t = run_cr(
        Permission::Write,
        &state(CrPhase::AwaitingChanges, d),
        &approval("1"),
    );
    assert_cr(&t, CrPhase::ReadyToMerge);
}

#[test]
fn approval_by_bot_or_reader_is_rejected() {
    let s = state(CrPhase::AwaitingChanges, unreviewed_ci_pending());
    rejected(
        &run_cr(
            Permission::Write,
            &s,
            &review("1", "enzarb[bot]", "approved", ""),
        ),
        &s,
    );
    rejected(&run_cr(Permission::Read, &s, &approval("2")), &s);
}

#[test]
fn approval_during_a_round_or_after_ready_is_ignored() {
    for phase in [
        CrPhase::Reviewing,
        CrPhase::AddressingComments,
        CrPhase::ResolvingConflicts,
        CrPhase::ReadyToMerge,
    ] {
        let d = CrData {
            trigger_text: "fix it".into(),
            ..unreviewed_ci_pending()
        };
        let s = state(phase, d);
        let t = run_cr(Permission::Write, &s, &approval("1"));
        assert!(t.accepted && t.effects.is_empty(), "{phase:?}");
        assert_eq!(t.next, s, "{phase:?}");
    }
}

#[test]
fn dismissed_review_is_still_rejected() {
    let s = state(CrPhase::AwaitingChanges, unreviewed_ci_pending());
    rejected(
        &run_cr(
            Permission::Write,
            &s,
            &review("1", "carol", "dismissed", ""),
        ),
        &s,
    );
}

#[test]
fn approval_is_invalidated_by_a_push() {
    let s = state(CrPhase::AwaitingChanges, unreviewed_ci_pending());
    let t = run_cr(Permission::Write, &s, &approval("1"));
    let t = run_cr(Permission::None, &t.next, &pr_updated("2", "sha2"));
    let WorkflowState::ChangeRequest(cs) = &t.next else {
        panic!("not a change request");
    };
    assert_eq!(waiting(&cs.data, cs.phase).1, "reviewing");
}

fn green_awaiting_review() -> CrData {
    CrData {
        last_review: String::new(),
        reviewed_sha: String::new(),
        ci: CiState::Success,
        ci_sha: "sha1".into(),
        mergeable: Some(true),
        ..cr_data()
    }
}

fn poll_event() -> EngineEvent {
    EngineEvent {
        id: "p".into(),
        payload: EventPayload::ReadinessPoll,
    }
}

#[test]
fn approve_command_by_a_writer_moves_to_ready_to_merge() {
    let s = state(CrPhase::AwaitingChanges, green_awaiting_review());
    let t = run_cr(Permission::Write, &s, &approval_comment("1", "carol"));
    assert_eq!(assert_cr(&t, CrPhase::ReadyToMerge).last_review, "approve");
    let t = run_cr(
        Permission::Write,
        &s,
        &ui("2", "carol", UiActionKind::Approve),
    );
    assert_cr(&t, CrPhase::ReadyToMerge);
    rejected(
        &run_cr(Permission::Read, &s, &approval_comment("3", "mallory")),
        &s,
    );
}

#[test]
fn approve_command_still_needs_green_ci() {
    let s = state(CrPhase::AwaitingChanges, unreviewed_ci_pending());
    let t = run_cr(Permission::Write, &s, &approval_comment("1", "carol"));
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(
        waiting(&d, CrPhase::AwaitingChanges),
        (WaitingOn::Ci, "ci_pending")
    );
}

#[test]
fn comment_verdict_lists_the_unverified_checks_and_a_human_approval_clears_them() {
    let s = state(CrPhase::Reviewing, green_awaiting_review());
    let checks = json!([
        { "name": "cargo test", "status": "passed" },
        { "name": "connectors tests", "status": "not_run", "reason": "missing_tool" },
        { "name": "cargo fmt --check", "status": "not_run" },
    ]);
    let t = run_cr(
        Permission::None,
        &s,
        &agent_ok_checks("r", JobKind::AgentReview, "comment", checks),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.unverified_checks.len(), 2);
    let w = wait(CrPhase::AwaitingChanges, &d, &readiness(&d)).unwrap();
    assert_eq!(
        (w.waiting_on, w.reason),
        (WaitingOn::Human, "checks_unverified")
    );
    assert!(
        w.text.contains("connectors tests (missing_tool)"),
        "{}",
        w.text
    );
    assert!(w.text.contains("cargo fmt --check (not_run)"), "{}", w.text);
    assert_eq!(t.next.state_data()["wait"]["reason"], "checks_unverified");

    let t = run_cr(Permission::Write, &t.next, &approval("a"));
    let d = assert_cr(&t, CrPhase::ReadyToMerge);
    assert!(d.unverified_checks.is_empty());
}

#[test]
fn covered_by_ci_counts_only_while_ci_is_green() {
    let checks = json!([
        { "name": "cargo test", "status": "passed" },
        { "name": "connectors tests", "status": "not_run", "reason": "covered_by_ci" },
    ]);
    let review = |d: CrData| {
        run_cr(
            Permission::None,
            &state(CrPhase::Reviewing, d),
            &agent_ok_checks("r", JobKind::AgentReview, "approve", checks.clone()),
        )
    };
    let green = review(green_awaiting_review());
    assert_eq!(
        assert_cr(&green, CrPhase::ReadyToMerge).last_review,
        "approve"
    );

    // CI not green: the claim does not count, the approve is downgraded. With no must-fix
    // items there is nothing to fix: no round, the item waits on the unverified check.
    let pending = review(unreviewed_ci_pending());
    let d = assert_cr(&pending, CrPhase::AwaitingChanges);
    assert!(!d.auto_trigger);
    assert_eq!(d.last_review, "comment");
    assert_eq!(d.unverified_checks.len(), 1);
    assert!(!kinds(&pending).contains(&JobKind::AgentAddressComments));
}

#[test]
fn readiness_poll_asks_again_whatever_the_review_said() {
    let d = CrData {
        last_review: "comment".into(),
        ..cr_data()
    };
    let s = state(CrPhase::AwaitingChanges, d);
    let t = run_cr(Permission::None, &s, &poll_event());
    assert_cr(&t, CrPhase::AwaitingChanges);
    job_of(&t, JobKind::CheckMergeReadiness);

    // Nothing to ask once CI is decided.
    let done = state(
        CrPhase::AwaitingChanges,
        CrData {
            ci: CiState::Failure,
            ci_sha: "sha1".into(),
            ..cr_data()
        },
    );
    rejected(&run_cr(Permission::None, &done, &poll_event()), &done);
}

#[test]
fn ci_pending_for_long_becomes_stale() {
    let d = CrData {
        last_review: "approve".into(),
        ci: CiState::Pending,
        ci_sha: "sha1".into(),
        mergeable: Some(true),
        ci_pending_sha: "sha1".into(),
        ci_pending_since: Some(now() - chrono::Duration::minutes(31)),
        ..cr_data()
    };
    let s = state(CrPhase::AwaitingChanges, d.clone());
    let d2 = assert_cr(
        &run_cr(Permission::None, &s, &poll_event()),
        CrPhase::AwaitingChanges,
    );
    assert!(d2.ci_stale);
    assert_eq!(
        waiting(&d2, CrPhase::AwaitingChanges),
        (WaitingOn::Human, "ci_stale")
    );

    // A recent start is not stale.
    let fresh = CrData {
        ci_pending_since: Some(now() - chrono::Duration::minutes(5)),
        ..d
    };
    let s = state(CrPhase::AwaitingChanges, fresh);
    let d3 = assert_cr(
        &run_cr(Permission::None, &s, &poll_event()),
        CrPhase::AwaitingChanges,
    );
    assert!(!d3.ci_stale);
    assert_eq!(
        waiting(&d3, CrPhase::AwaitingChanges),
        (WaitingOn::Ci, "ci_pending")
    );
}

#[test]
fn state_data_without_the_new_fields_still_loads() {
    let d: CrData = serde_json::from_value(json!({
        "number": 7, "head_sha": "s", "last_review": "comment", "ci": "pending"
    }))
    .unwrap();
    assert!(d.unverified_checks.is_empty() && d.ci_pending_since.is_none() && !d.ci_stale);
}
