//! Automatic CI-fix rounds and the truthful `ci_failed` wait.

use super::*;
use crate::machine::change_request::{CiFailure, WaitingOn, readiness, wait};
use enzarb_connector::CiState as Ev;

fn check_event(id: &str, sha: &str, name: &str, state: Ev) -> EngineEvent {
    let mut e = conn(
        id,
        EventType::CiStatusChanged,
        pr_item(sha),
        actor("ci"),
        None,
    );
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.ci = Some(enzarb_connector::CiInfo {
            head_sha: sha.into(),
            state,
            context: Some(name.into()),
            url: Some(format!("https://ci.example/{name}")),
        });
    }
    e
}

fn fail_event(id: &str, name: &str) -> EngineEvent {
    check_event(id, "sha1", name, Ev::Failure)
}

/// Number of fix jobs the transition queued.
fn fix_jobs(t: &Transition) -> usize {
    kinds(t)
        .into_iter()
        .filter(|k| *k == JobKind::AgentAddressComments)
        .count()
}

fn awaiting(last_review: &str) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState {
        phase: CrPhase::AwaitingChanges,
        data: CrData {
            last_review: last_review.into(),
            ..cr_data()
        },
    })
}

fn wait_of_state(t: &Transition) -> (String, String, String) {
    let v = t.next.state_data();
    (
        v["wait"]["waiting_on"].as_str().unwrap().into(),
        v["wait"]["reason"].as_str().unwrap().into(),
        v["wait"]["text"].as_str().unwrap().into(),
    )
}

#[test]
fn ci_failure_after_a_comment_review_starts_one_round() {
    // Replay of #139: a `comment` review, then failing checks on the head.
    let t = run_cr(
        Permission::None,
        &awaiting("comment"),
        &fail_event("f0", "actionlint"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(fix_jobs(&t), 1);
    assert_eq!(d.auto_rounds, 1);
    assert!(d.auto_trigger);
    assert_eq!(d.ci_fix_sha, "sha1");
    assert!(d.trigger_text.starts_with("CI failed on sha1"));
    assert!(
        d.trigger_text
            .contains("- actionlint (https://ci.example/actionlint)")
    );
    // Burst debounce: the later failures of the same head only record.
    let mut state = t.next;
    for (i, name) in ["scripts", "ci / test"].iter().enumerate() {
        let t = run_cr(
            Permission::None,
            &state,
            &fail_event(&format!("f{i}"), name),
        );
        let d = assert_cr(&t, CrPhase::AddressingComments);
        assert_eq!(d.auto_rounds, 1);
        assert_eq!(fix_jobs(&t), 0);
        state = t.next;
    }
}

#[test]
fn checks_seen_before_the_review_ends_are_all_listed() {
    let mut state = cr_in(CrPhase::Reviewing);
    for (i, name) in ["actionlint", "scripts", "ci / test"].iter().enumerate() {
        let t = run_cr(
            Permission::None,
            &state,
            &fail_event(&format!("f{i}"), name),
        );
        assert_eq!(fix_jobs(&t), 0, "no round while reviewing");
        state = t.next;
    }
    let t = run_cr(
        Permission::None,
        &state,
        &agent_ok("r", JobKind::AgentReview, "comment", "remark"),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    for name in ["actionlint", "scripts", "ci / test"] {
        assert!(
            d.trigger_text
                .contains(&format!("- {name} (https://ci.example/{name})")),
            "{}",
            d.trigger_text
        );
    }
    assert_eq!(fix_jobs(&t), 1);
    assert_eq!(wait_of_state(&t).1, "enzarb_fixing");
}

#[test]
fn ci_failure_after_an_approve_review_starts_a_round() {
    let ready = WorkflowState::ChangeRequest(CrState {
        phase: CrPhase::ReadyToMerge,
        data: CrData {
            last_review: "approve".into(),
            ci: CiState::Success,
            ci_sha: "sha1".into(),
            mergeable: Some(true),
            merge_requested: true,
            ..cr_data()
        },
    });
    let t = run_cr(Permission::None, &ready, &fail_event("f", "build"));
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert!(!d.merge_requested);
    assert_eq!(fix_jobs(&t), 1);
    let t = run_cr(
        Permission::None,
        &awaiting("approve"),
        &fail_event("g", "build"),
    );
    assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(fix_jobs(&t), 1);
}

#[test]
fn rounds_used_up_wait_for_a_human_with_the_failing_checks() {
    let capped = WorkflowState::ChangeRequest(CrState {
        phase: CrPhase::AwaitingChanges,
        data: CrData {
            last_review: "comment".into(),
            auto_rounds: 3,
            ..cr_data()
        },
    });
    let t = run_cr(Permission::None, &capped, &fail_event("f", "scripts"));
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(fix_jobs(&t), 0);
    let (on, reason, text) = wait_of_state(&t);
    assert_eq!((on.as_str(), reason.as_str()), ("human", "ci_failed"));
    assert_eq!(text, "CI failed: scripts");
    assert_eq!(
        t.next.state_data()["wait"]["url"],
        json!("https://ci.example/scripts")
    );
    // Automatic rounds switched off.
    let t = run_cr_manual(
        Permission::None,
        &awaiting("comment"),
        &fail_event("g", "scripts"),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(fix_jobs(&t), 0);
    assert_eq!(wait_of_state(&t).1, "ci_failed");
}

#[test]
fn retry_starts_a_round_after_the_cap() {
    let t = run_cr_manual(
        Permission::None,
        &awaiting("comment"),
        &fail_event("g", "scripts"),
    );
    let t = run_cr(
        Permission::Write,
        &t.next,
        &ui("r", "carol", UiActionKind::Retry),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!(d.auto_rounds, 1);
    assert_eq!(fix_jobs(&t), 1);
    // No failed CI, nothing to retry.
    let t = run_cr(
        Permission::Write,
        &awaiting("comment"),
        &ui("r2", "carol", UiActionKind::Retry),
    );
    assert!(!t.accepted);
}

#[test]
fn failure_during_a_run_is_recorded_and_acted_on_when_it_ends() {
    for (phase, kind) in [
        (CrPhase::AddressingComments, JobKind::AgentAddressComments),
        (CrPhase::ResolvingConflicts, JobKind::AgentResolveConflicts),
    ] {
        let state = WorkflowState::ChangeRequest(CrState {
            phase,
            data: CrData {
                last_review: "comment".into(),
                ..cr_data()
            },
        });
        let t = run_cr(Permission::None, &state, &fail_event("f", "scripts"));
        assert_eq!(fix_jobs(&t), 0, "{phase:?}");
        // Nothing was pushed: the head is still the reviewed one.
        let mut out = agent_output("done", "ok");
        out["head_sha"] = json!("sha1");
        let t = run_cr(Permission::None, &t.next, &ok("d", kind, out));
        let d = assert_cr(&t, CrPhase::AddressingComments);
        assert!(d.trigger_text.contains("scripts"), "{phase:?}");
        assert_eq!(fix_jobs(&t), 1);
    }
}

fn fix_input(t: &Transition) -> AgentJobInput {
    serde_json::from_value(job_of(t, JobKind::AgentAddressComments).2).unwrap()
}

#[test]
fn a_ci_fix_round_carries_the_failing_check_and_the_run_id() {
    let mut e = fail_event("f0", "ci / test");
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.ci.as_mut().unwrap().url =
            Some("https://github.com/enzarb/enzarb/actions/runs/777/job/888".into());
    }
    let t = run_cr(Permission::None, &awaiting("comment"), &e);
    assert_cr(&t, CrPhase::AddressingComments);
    let input = fix_input(&t);
    assert!(input.context.ci_fix);
    for needle in [
        "check: ci / test",
        "conclusion: failure",
        "url: https://github.com/enzarb/enzarb/actions/runs/777/job/888",
        "run id: 777",
        "job id: 888",
    ] {
        assert!(input.context.ci_failure.contains(needle), "{needle}");
        assert!(input.prompt.contains(needle), "{needle}");
    }
    assert!(input.prompt.contains("gh run view <run id> --log-failed"));
}

#[test]
fn a_non_actions_check_has_no_run_id() {
    let t = run_cr(
        Permission::None,
        &awaiting("comment"),
        &fail_event("f0", "actionlint"),
    );
    let input = fix_input(&t);
    assert!(input.context.ci_fix);
    assert!(input.context.ci_failure.contains("check: actionlint"));
    assert!(!input.context.ci_failure.contains("run id"));
    assert!(
        input
            .context
            .ci_failure
            .contains("not a GitHub Actions run")
    );
}

#[test]
fn only_ci_fix_rounds_are_flagged() {
    // A round started by a person's comment has no CI failure and no flag.
    let t = run_cr(
        Permission::Admin,
        &awaiting("comment"),
        &conn(
            "c1",
            EventType::CommentAdded,
            pr_item("sha1"),
            actor("alice"),
            Some((9, "please rename this")),
        ),
    );
    assert_cr(&t, CrPhase::AddressingComments);
    let input = fix_input(&t);
    assert!(!input.context.ci_fix && input.context.ci_failure.is_empty());
    assert!(input.prompt.contains("<untrusted>\n(none)\n</untrusted>"));
}

#[test]
fn actions_urls_are_parsed() {
    use crate::machine::change_request::actions_ids;
    let id = |u: &str| actions_ids(u);
    assert_eq!(
        id("https://github.com/o/r/actions/runs/12/job/34"),
        Some(("12".into(), Some("34".into())))
    );
    assert_eq!(
        id("https://github.com/o/r/actions/runs/12"),
        Some(("12".into(), None))
    );
    assert_eq!(id("https://ci.example/x"), None);
    assert_eq!(id("https://github.com/o/r/actions/runs/abc"), None);
}

#[test]
fn a_new_head_clears_the_ci_fix_bookkeeping() {
    let t = run_cr(
        Permission::None,
        &awaiting("comment"),
        &fail_event("f", "scripts"),
    );
    let mut out = agent_output("done", "fixed");
    out["head_sha"] = json!("sha2");
    let t = run_cr(
        Permission::None,
        &t.next,
        &ok("d", JobKind::AgentAddressComments, out),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.head_sha, "sha2");
    assert!(d.ci_fix_sha.is_empty() && d.ci_failures.is_empty());
    // The fix is reviewed, then CI fails on it: a second round.
    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok("r", JobKind::AgentReview, "comment", "fine"),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    let t = run_cr(
        Permission::None,
        &t.next,
        &check_event("f2", "sha2", "scripts", Ev::Failure),
    );
    let d = assert_cr(&t, CrPhase::AddressingComments);
    assert_eq!((d.auto_rounds, d.ci_fix_sha.as_str()), (2, "sha2"));
}

#[test]
fn old_state_without_the_new_fields_loads() {
    let d: CrData =
        serde_json::from_value(json!({ "number": 7, "head_sha": "a", "ci": "failure" })).unwrap();
    assert!(d.ci_fix_sha.is_empty() && d.ci_failures.is_empty());
}

/// Wherever `wait()` says enzarb is fixing something in `AwaitingChanges`, a job must be
/// pending: a failed CI is never reported as "enzarb is fixing it", and after a CI event
/// the item either started a fix round or waits on a human.
#[test]
fn enzarb_never_waits_on_ci_without_a_job() {
    let reviews = ["", "comment", "approve", "request_changes"];
    let cis = [
        CiState::Unknown,
        CiState::Pending,
        CiState::Success,
        CiState::Failure,
    ];
    for review in reviews {
        for ci in cis {
            for capped in [false, true] {
                for (rounds, max) in [(0, 3), (3, 3), (0, 0), (1, 3)] {
                    let d = CrData {
                        last_review: review.into(),
                        ci,
                        ci_sha: "sha1".into(),
                        auto_capped: capped,
                        auto_rounds: rounds,
                        ci_failures: vec![CiFailure {
                            name: "a".into(),
                            url: String::new(),
                        }],
                        ci_failures_sha: "sha1".into(),
                        ..cr_data()
                    };
                    let r = readiness(&d);
                    let w = wait(CrPhase::AwaitingChanges, &d, &r).unwrap();
                    let ctxt = format!("{review} {ci:?} capped={capped} {rounds}/{max}");
                    assert_ne!(w.reason, "ci_failed_fixing", "{ctxt}");
                    if ci == CiState::Failure && !review.is_empty() {
                        assert_eq!(
                            (w.waiting_on, w.reason),
                            (WaitingOn::Human, "ci_failed"),
                            "{ctxt}"
                        );
                    }
                    // Through the real transition: enzarb waits on CI only with a job queued.
                    let mut c = ctx_for(43, Permission::None);
                    c.config.max_auto_rounds = max;
                    let state = WorkflowState::ChangeRequest(CrState {
                        phase: CrPhase::AwaitingChanges,
                        data: d,
                    });
                    let t = transition(&c, &state, &fail_event("f", "a"));
                    assert!(t.accepted, "{ctxt}");
                    let (on, reason, _) = wait_of_state(&t);
                    if on == "enzarb" && reason != "reviewing" {
                        assert!(fix_jobs(&t) > 0, "{ctxt}: {reason}");
                    }
                    if reason == "ci_failed" {
                        assert_eq!(on, "human", "{ctxt}");
                    }
                }
            }
        }
    }
}
