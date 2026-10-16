//! Cheaper review rounds: delta-mode re-reviews, the mechanical-delta shortcut, flagged
//! unchecked pushes and the per-change-request counters.

use super::*;
use crate::machine::change_request::{
    CrStats, PriorReview, WaitingOn, delta_base, readiness, wait,
};

fn waiting(d: &CrData, phase: CrPhase) -> (WaitingOn, &'static str) {
    let w = wait(phase, d, &readiness(d)).expect("wait");
    (w.waiting_on, w.reason)
}

/// A change request that was reviewed at `sha1` against base `b1` with `verdict`, and whose
/// head is now `sha2` (an address round just pushed it, or a person did).
fn reviewed_earlier(verdict: &str) -> CrData {
    CrData {
        head_sha: "sha2".into(),
        reviewed_sha: "sha1".into(),
        last_review: verdict.into(),
        base_sha: "b1".into(),
        prior_review: PriorReview {
            sha: "sha1".into(),
            base_sha: "b1".into(),
            verdict: verdict.into(),
        },
        ..cr_data()
    }
}

fn reviewing(d: CrData) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState {
        phase: CrPhase::Reviewing,
        data: d,
    })
}

/// The result of an address round that pushed `head`, with the given `checks`.
fn pushed_round(head: &str, checks: Value) -> EngineEvent {
    let block = json!({ "decision": "done", "body": "Fixed it", "checks": checks });
    ok(
        "d",
        JobKind::AgentAddressComments,
        json!({
            "summary": format!("Done.\n\n```json\n{block}\n```\n"),
            "head_sha": head,
        }),
    )
}

fn after_address_round(d: CrData, checks: Value) -> Transition {
    let s = WorkflowState::ChangeRequest(CrState {
        phase: CrPhase::AddressingComments,
        data: CrData {
            head_sha: "sha1".into(),
            reviewed_sha: "sha1".into(),
            ..d
        },
    });
    run_cr(Permission::None, &s, &pushed_round("sha2", checks))
}

fn job_input(t: &Transition, kind: JobKind) -> AgentJobInput {
    serde_json::from_value(job_of(t, kind).2).unwrap()
}

fn mechanical_result(decision: &str, checks: Value) -> EngineEvent {
    let block = json!({ "decision": decision, "body": "mechanical", "checks": checks });
    ok(
        "m",
        JobKind::MechanicalCheck,
        json!({
            "summary": format!("Done.\n\n```json\n{block}\n```\n"),
            "elapsed_secs": 12,
        }),
    )
}

fn passed() -> Value {
    json!([{ "name": "cargo fmt --check", "status": "passed" }])
}

#[test]
fn the_second_review_gets_the_previous_sha_and_a_delta_prompt() {
    let t = after_address_round(reviewed_earlier("comment"), passed());
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert!(!d.mechanical_check);
    let input = job_input(&t, JobKind::AgentReview);
    assert_eq!(input.context.previous_reviewed_sha, "sha1");
    assert!(input.prompt.contains("git diff sha1..HEAD"));
    assert!(input.prompt.contains("scripts/monorepo/affected.sh sha1"));
    assert!(!kinds(&t).contains(&JobKind::MechanicalCheck));
}

#[test]
fn the_first_review_is_a_full_review() {
    let t = run_cr(
        Permission::None,
        &WorkflowState::ChangeRequest(CrState {
            phase: CrPhase::AddressingComments,
            data: cr_data(),
        }),
        &pushed_round("sha2", passed()),
    );
    assert_cr(&t, CrPhase::Reviewing);
    let input = job_input(&t, JobKind::AgentReview);
    assert_eq!(input.context.previous_reviewed_sha, "");
}

#[test]
fn a_moved_or_unknown_base_forces_a_full_review() {
    for (base, prior_base) in [("b2", "b1"), ("b1", ""), ("", "b1")] {
        let mut d = reviewed_earlier("comment");
        d.base_sha = base.into();
        d.prior_review.base_sha = prior_base.into();
        let t = after_address_round(d, passed());
        assert_cr(&t, CrPhase::Reviewing);
        let input = job_input(&t, JobKind::AgentReview);
        assert_eq!(
            input.context.previous_reviewed_sha, "",
            "base {base:?}, reviewed against {prior_base:?}"
        );
        assert!(!kinds(&t).contains(&JobKind::MechanicalCheck));
    }
}

#[test]
fn a_repeated_head_is_not_a_delta() {
    let mut d = reviewed_earlier("comment");
    d.head_sha = "sha1".into();
    assert_eq!(delta_base(&d), None);
}

#[test]
fn a_delta_after_an_approval_starts_the_mechanical_check_not_the_agent_review() {
    let t = after_address_round(reviewed_earlier("approve"), passed());
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert!(d.mechanical_check);
    assert!(!kinds(&t).contains(&JobKind::AgentReview));
    let input = job_input(&t, JobKind::MechanicalCheck);
    assert_eq!(input.context.previous_reviewed_sha, "sha1");
    assert!(
        input
            .prompt
            .contains("scripts/monorepo/mechanical.sh sha1 HEAD 300")
    );
}

#[test]
fn a_mechanical_delta_with_green_ci_skips_the_agent_review() {
    let d = CrData {
        mechanical_check: true,
        ci: CiState::Success,
        ci_sha: "sha2".into(),
        mergeable: Some(true),
        ..reviewed_earlier("approve")
    };
    let t = run_cr(
        Permission::None,
        &reviewing(d),
        &mechanical_result("approve", passed()),
    );
    let d = assert_cr(&t, CrPhase::ReadyToMerge);
    assert!(!d.mechanical_check);
    assert_eq!(d.reviewed_sha, "sha2");
    assert_eq!(d.prior_review.sha, "sha2");
    assert_eq!(d.last_review, "approve");
    // The skipped review is not a review round; the job's time still counts.
    assert_eq!(d.stats.review_rounds, 0);
    assert_eq!(d.stats.agent_secs, 12);
    assert!(!kinds(&t).contains(&JobKind::AgentReview));
    assert!(
        comment_bodies(&t)
            .iter()
            .any(|b| b.starts_with("### enzarb review: skipped"))
    );
}

#[test]
fn a_mechanical_delta_without_green_ci_waits_and_does_not_merge() {
    let d = CrData {
        mechanical_check: true,
        ci: CiState::Pending,
        ci_sha: "sha2".into(),
        mergeable: Some(true),
        ..reviewed_earlier("approve")
    };
    let t = run_cr(
        Permission::None,
        &reviewing(d),
        &mechanical_result("approve", passed()),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.reviewed_sha, "sha2");
    assert!(!kinds(&t).contains(&JobKind::AgentReview));
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    assert_eq!(
        waiting(&d, CrPhase::AwaitingChanges),
        (WaitingOn::Ci, "ci_pending")
    );
}

#[test]
fn red_ci_on_the_head_gets_the_agent_review_instead() {
    let d = CrData {
        mechanical_check: true,
        ci: CiState::Failure,
        ci_sha: "sha2".into(),
        ..reviewed_earlier("approve")
    };
    let t = run_cr(
        Permission::None,
        &reviewing(d),
        &mechanical_result("approve", passed()),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert!(!d.mechanical_check);
    assert!(kinds(&t).contains(&JobKind::AgentReview));
}

#[test]
fn a_non_mechanical_or_failed_check_starts_a_full_agent_review() {
    let failed_check = json!([{ "name": "npm run lint", "status": "failed" }]);
    for ev in [
        mechanical_result("comment", passed()),
        mechanical_result("approve", failed_check),
        mechanical_result("approve", json!([])),
        failed("f", JobKind::MechanicalCheck),
    ] {
        let d = CrData {
            mechanical_check: true,
            ..reviewed_earlier("approve")
        };
        let t = run_cr(Permission::None, &reviewing(d), &ev);
        let d = assert_cr(&t, CrPhase::Reviewing);
        assert!(!d.mechanical_check);
        assert_eq!(d.reviewed_sha, "sha1", "the head is still unreviewed");
        let input = job_input(&t, JobKind::AgentReview);
        assert_eq!(
            input.context.previous_reviewed_sha, "sha1",
            "the fallback is still a delta review"
        );
    }
}

#[test]
fn a_failed_mechanical_check_adds_its_time() {
    let d = CrData {
        mechanical_check: true,
        ..reviewed_earlier("approve")
    };
    let ev = EngineEvent {
        id: "f".into(),
        payload: EventPayload::JobFailed {
            job_id: 1,
            kind: JobKind::MechanicalCheck,
            error: "boom".into(),
            elapsed_secs: 17,
        },
    };
    let t = run_cr(Permission::None, &reviewing(d), &ev);
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.stats.agent_secs, 17);
}

#[test]
fn a_mechanical_result_nobody_waits_for_is_stale() {
    let s = reviewing(reviewed_earlier("approve"));
    rejected(
        &run_cr(
            Permission::None,
            &s,
            &mechanical_result("approve", passed()),
        ),
        &s,
    );
}

#[test]
fn counters_follow_the_rounds_and_the_job_time() {
    // A review: one review round, its time counted.
    let block = json!({ "decision": "request_changes", "body": "no", "must_fix": ["x"] });
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &ok(
            "r",
            JobKind::AgentReview,
            json!({ "summary": format!("x\n\n```json\n{block}\n```\n"), "elapsed_secs": 90 }),
        ),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(d.stats.review_rounds, 1);
    assert_eq!(d.stats.fix_rounds, 1);
    assert_eq!(d.stats.agent_secs, 90);
    // The fix round: its time is added.
    let mut out = agent_output("done", "Fixed it");
    out["elapsed_secs"] = json!(30);
    let t = run_cr(
        Permission::None,
        &t.next,
        &ok("d", JobKind::AgentAddressComments, out),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        (
            d.stats.review_rounds,
            d.stats.fix_rounds,
            d.stats.agent_secs
        ),
        (1, 1, 120)
    );
    // A new commit by a person restarts the review and keeps the counters and the prior review.
    let t = run_cr(Permission::None, &t.next, &pr_updated("u", "sha77"));
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.stats.agent_secs, 120);
    assert_eq!(d.prior_review.verdict, "request_changes");
    assert_eq!(d.prior_review.sha, "sha1");
}

#[test]
fn counters_are_in_the_state_data_for_the_work_item_page() {
    let mut d = cr_data();
    d.stats = CrStats {
        review_rounds: 2,
        fix_rounds: 1,
        agent_secs: 300,
    };
    let v = serde_json::to_value(&d).unwrap();
    assert_eq!(v["stats"]["review_rounds"], 2);
    assert_eq!(v["stats"]["fix_rounds"], 1);
    assert_eq!(v["stats"]["agent_secs"], 300);
    // State written before the counters existed still reads.
    let old: CrData = serde_json::from_value(json!({ "number": 4 })).unwrap();
    assert_eq!(old.stats, CrStats::default());
    assert_eq!(old.prior_review, PriorReview::default());
}

#[test]
fn a_push_with_unverified_checks_is_flagged_in_the_comment() {
    let checks = json!([
        { "name": "cargo test", "status": "passed" },
        { "name": "npm run lint", "status": "failed" },
        { "name": "helm lint", "status": "not_run", "reason": "missing_tool" },
    ]);
    let t = after_address_round(reviewed_earlier("comment"), checks);
    let bodies = comment_bodies(&t);
    assert_eq!(
        bodies,
        vec![
            "Fixed it\n\n> Pushed with unverified checks: `npm run lint` (failed), `helm lint` (not_run: missing_tool)."
                .to_string()
        ]
    );
}

#[test]
fn a_push_with_all_checks_passed_has_no_flag() {
    let t = after_address_round(reviewed_earlier("comment"), passed());
    assert_eq!(comment_bodies(&t), vec!["Fixed it".to_string()]);
}
