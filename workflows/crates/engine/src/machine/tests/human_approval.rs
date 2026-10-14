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
