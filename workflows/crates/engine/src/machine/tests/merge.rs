//! ReadyToMerge and the user-triggered merge.

use super::*;
use crate::machine::change_request::{CheckStatus, readiness};

fn ci_event(id: &str, sha: &str, state: enzarb_connector::CiState) -> EngineEvent {
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
            context: Some("build".into()),
            url: None,
        });
    }
    e
}

/// Approved at `sha1`, CI green on it, mergeable.
fn ready_data() -> CrData {
    CrData {
        last_review: "approve".into(),
        ci: CiState::Success,
        ci_sha: "sha1".into(),
        mergeable: Some(true),
        ..cr_data()
    }
}

/// Approved, but nothing known about CI or mergeability yet.
fn unknown_data() -> CrData {
    CrData {
        ci: CiState::Unknown,
        ci_sha: String::new(),
        mergeable: None,
        ..ready_data()
    }
}

fn cr_with(phase: CrPhase, data: CrData) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState { phase, data })
}

fn readiness_out(sha: &str, ci: &str, mergeable: Value) -> Value {
    json!({ "head_sha": sha, "ci": ci, "mergeable": mergeable, "mergeable_state": "clean" })
}

fn checked(id: &str, out: Value) -> EngineEvent {
    ok(id, JobKind::CheckMergeReadiness, out)
}

fn merge_ev(id: &str, who: &str) -> EngineEvent {
    ui(id, who, UiActionKind::Merge)
}

fn merge_result(id: &str, out: Value) -> EngineEvent {
    ok(id, JobKind::MergeChangeRequest, out)
}

fn merging_state() -> WorkflowState {
    cr_with(CrPhase::Merging, ready_data())
}

fn verified() -> Value {
    json!([{ "name": "cargo test", "status": "passed" }])
}

#[test]
fn approved_green_mergeable_review_is_ready_to_merge() {
    let s = cr_with(
        CrPhase::Reviewing,
        CrData {
            last_review: String::new(),
            ..ready_data()
        },
    );
    let t = run_cr(
        Permission::None,
        &s,
        &agent_ok_checks("r", JobKind::AgentReview, "approve", verified()),
    );
    let d = assert_cr(&t, CrPhase::ReadyToMerge);
    assert_eq!(d.last_review, "approve");
    assert!(!kinds(&t).contains(&JobKind::CheckMergeReadiness));
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
}

#[test]
fn each_missing_condition_keeps_awaiting_changes() {
    let base = ready_data();
    let answer = |d: CrData, out: Value| {
        run_cr(
            Permission::None,
            &cr_with(CrPhase::AwaitingChanges, d),
            &checked("c", out),
        )
    };
    let good = readiness_out("sha1", "success", json!(true));
    // Control: everything present.
    let t = answer(unknown_data(), good.clone());
    assert_cr(&t, CrPhase::ReadyToMerge);

    // The review is the missing condition: nothing to check either.
    for (name, d) in [
        (
            "comment review",
            CrData {
                last_review: "comment".into(),
                ..base.clone()
            },
        ),
        (
            "request_changes review",
            CrData {
                last_review: "request_changes".into(),
                ..base.clone()
            },
        ),
        (
            "no review",
            CrData {
                last_review: String::new(),
                ..base.clone()
            },
        ),
        (
            "review of an older commit",
            CrData {
                reviewed_sha: "old".into(),
                ..base.clone()
            },
        ),
        (
            "empty reviewed sha",
            CrData {
                reviewed_sha: String::new(),
                ..base.clone()
            },
        ),
    ] {
        let t = answer(d, good.clone());
        assert_cr(&t, CrPhase::AwaitingChanges);
        assert!(jobs(&t).is_empty(), "{name}");
    }

    // CI or mergeability is the missing condition. The answer never asks again.
    for (name, out) in [
        ("ci failure", readiness_out("sha1", "failure", json!(true))),
        ("ci pending", readiness_out("sha1", "pending", json!(true))),
        ("ci unknown", readiness_out("sha1", "weird", json!(true))),
        ("conflicts", readiness_out("sha1", "success", json!(false))),
        (
            "mergeable unknown",
            readiness_out("sha1", "success", Value::Null),
        ),
    ] {
        let t = answer(unknown_data(), out);
        assert_cr(&t, CrPhase::AwaitingChanges);
        assert!(jobs(&t).is_empty(), "{name}");
    }

    // An answer for another commit is ignored.
    let s = cr_with(CrPhase::AwaitingChanges, unknown_data());
    let t = run_cr(
        Permission::None,
        &s,
        &checked("c", readiness_out("zzz", "success", json!(true))),
    );
    assert!(t.accepted);
    assert_eq!(t.next, s);
}

#[test]
fn ci_of_another_commit_is_not_trusted() {
    // Stored CI belongs to an older commit, so it counts as unknown.
    let stale = CrData {
        ci_sha: "old".into(),
        ..ready_data()
    };
    let r = readiness(&stale);
    assert_eq!(r.ci.status, CheckStatus::Unknown);
    // An event for a commit that is not the head is rejected.
    let s = cr_with(CrPhase::AwaitingChanges, stale);
    let t = run_cr(
        Permission::None,
        &s,
        &ci_event("e", "old", enzarb_connector::CiState::Success),
    );
    rejected(&t, &s);
}

#[test]
fn unknown_readiness_runs_one_check_and_its_result_decides() {
    let s = cr_with(
        CrPhase::Reviewing,
        CrData {
            last_review: String::new(),
            ..unknown_data()
        },
    );
    let t = run_cr(
        Permission::None,
        &s,
        &agent_ok_checks("r", JobKind::AgentReview, "approve", verified()),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(
        kinds(&t)
            .iter()
            .filter(|k| **k == JobKind::CheckMergeReadiness)
            .count(),
        1
    );
    let (_, key, input) = job_of(&t, JobKind::CheckMergeReadiness);
    assert_eq!(key, "github:enzarb/enzarb-ui#43:r:check_merge_readiness");
    assert_eq!(input["number"], 43);
    assert_eq!(input["head_sha"], "sha1");
    assert_eq!(
        input["item"],
        serde_json::to_value(WorkItemKey::new(repo(), 43)).unwrap()
    );

    let t2 = run_cr(
        Permission::None,
        &t.next,
        &checked("c1", readiness_out("sha1", "success", json!(true))),
    );
    assert_cr(&t2, CrPhase::ReadyToMerge);
    assert!(jobs(&t2).is_empty());

    // Still unknown: held, and no second check.
    let t3 = run_cr(
        Permission::None,
        &t.next,
        &checked("c2", readiness_out("sha1", "pending", Value::Null)),
    );
    let d = assert_cr(&t3, CrPhase::AwaitingChanges);
    assert_eq!(d.ci, CiState::Pending);
    assert!(jobs(&t3).is_empty());

    // A failed check job is best effort.
    let t4 = run_cr(
        Permission::None,
        &t.next,
        &failed("c3", JobKind::CheckMergeReadiness),
    );
    assert!(t4.accepted);
    assert_eq!(t4.next, t.next);

    // Only an approving review asks at all.
    let t5 = run_cr_manual(
        Permission::None,
        &s,
        &agent_ok("r", JobKind::AgentReview, "request_changes", "no"),
    );
    assert_cr(&t5, CrPhase::AwaitingChanges);
    assert!(
        jobs(&t5)
            .iter()
            .all(|j| j.0 != JobKind::CheckMergeReadiness)
    );
}

#[test]
fn ci_events_drive_readiness() {
    use enzarb_connector::CiState as Ev;
    let awaiting = cr_with(
        CrPhase::AwaitingChanges,
        CrData {
            ci: CiState::Pending,
            ci_sha: "sha1".into(),
            ..ready_data()
        },
    );
    // A success only says one check passed: ask for the aggregate.
    let t = run_cr(
        Permission::None,
        &awaiting,
        &ci_event("e1", "sha1", Ev::Success),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(d.ci, CiState::Unknown);
    job_of(&t, JobKind::CheckMergeReadiness);

    // A failure is taken as is, with no check.
    let t = run_cr(
        Permission::None,
        &awaiting,
        &ci_event("e2", "sha1", Ev::Failure),
    );
    assert_eq!(assert_cr(&t, CrPhase::AwaitingChanges).ci, CiState::Failure);
    assert!(jobs(&t).is_empty());

    // Leaving ReadyToMerge on failure and on a running check.
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    for (id, st, want) in [
        ("f", Ev::Failure, CiState::Failure),
        ("p", Ev::Pending, CiState::Pending),
    ] {
        let t = run_cr(Permission::None, &ready, &ci_event(id, "sha1", st));
        assert_eq!(assert_cr(&t, CrPhase::AwaitingChanges).ci, want);
        // A failure is final; a running check is re-asked in case its result is lost.
        assert_eq!(jobs(&t).is_empty(), want == CiState::Failure);
    }
    // Another green check keeps it ready.
    let t = run_cr(
        Permission::None,
        &ready,
        &ci_event("s", "sha1", Ev::Success),
    );
    assert_cr(&t, CrPhase::ReadyToMerge);

    // Conflicts reported by a readiness check leave it too.
    let t = run_cr(
        Permission::None,
        &ready,
        &checked("m", readiness_out("sha1", "success", json!(false))),
    );
    assert_eq!(
        assert_cr(&t, CrPhase::AwaitingChanges).mergeable,
        Some(false)
    );

    // While a review runs the result is only recorded.
    let t = run_cr(
        Permission::None,
        &cr_in(CrPhase::Reviewing),
        &ci_event("r", "sha1", Ev::Failure),
    );
    assert_eq!(assert_cr(&t, CrPhase::Reviewing).ci, CiState::Failure);
    assert!(jobs(&t).is_empty());

    // Events for another commit, without data, or after the end are rejected.
    rejected(
        &run_cr(
            Permission::None,
            &ready,
            &ci_event("o", "other", Ev::Failure),
        ),
        &ready,
    );
    let no_data = conn(
        "n",
        EventType::CiStatusChanged,
        pr_item("sha1"),
        actor("ci"),
        None,
    );
    rejected(&run_cr(Permission::None, &ready, &no_data), &ready);
    for p in [
        CrPhase::Merging,
        CrPhase::Merged,
        CrPhase::Closed,
        CrPhase::Failed,
    ] {
        let s = cr_with(p, ready_data());
        rejected(
            &run_cr(Permission::None, &s, &ci_event("x", "sha1", Ev::Failure)),
            &s,
        );
    }
}

#[test]
fn push_or_feedback_leaves_ready_to_merge() {
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    let t = run_cr(Permission::None, &ready, &pr_updated("u", "sha9"));
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.head_sha, "sha9");
    assert!(d.last_review.is_empty());
    assert_eq!((d.ci, d.mergeable), (CiState::Unknown, None));
    job_of(&t, JobKind::AgentReview);
    // The same commit again is a duplicate.
    rejected(
        &run_cr(Permission::None, &ready, &pr_updated("d", "sha1")),
        &ready,
    );

    // A review asking for changes goes to the agent; a plain comment too.
    let t = run_cr(
        Permission::Write,
        &ready,
        &review("rv", "carol", "CHANGES_REQUESTED", "please fix"),
    );
    assert_cr(&t, CrPhase::AddressingComments);
    job_of(&t, JobKind::AgentAddressComments);
    let t = run_cr(
        Permission::Write,
        &ready,
        &pr_comment("c", "carol", 5, "fix it"),
    );
    assert_cr(&t, CrPhase::AddressingComments);
    // An approving review is a no-op here; read-only authors are rejected.
    let t = run_cr(
        Permission::Write,
        &ready,
        &review("a", "carol", "APPROVED", ""),
    );
    assert!(t.accepted && t.effects.is_empty());
    assert_eq!(t.next, ready);
    rejected(
        &run_cr(
            Permission::Read,
            &ready,
            &pr_comment("c2", "mallory", 6, "fix it"),
        ),
        &ready,
    );
    // Closing and cancelling work as everywhere else.
    assert_cr(
        &run_cr(
            Permission::None,
            &ready,
            &closed("cl", pr_item("sha1"), true),
        ),
        CrPhase::Merged,
    );
    assert_cr(
        &run_cr(
            Permission::Write,
            &ready,
            &ui("x", "carol", UiActionKind::Cancel),
        ),
        CrPhase::Cancelled,
    );
    // A push during a merge only records the head; the merge reports head_moved.
    let t = run_cr(
        Permission::None,
        &merging_state(),
        &pr_updated("u2", "sha9"),
    );
    let d = assert_cr(&t, CrPhase::Merging);
    assert_eq!((d.head_sha.as_str(), d.mergeable), ("sha9", None));
    assert!(jobs(&t).is_empty());
}

#[test]
fn merge_action_is_gated() {
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    // Read-only and anonymous actors are refused.
    for perm in [Permission::Read, Permission::None] {
        let t = run_cr(perm, &ready, &merge_ev("m", "mallory"));
        rejected(&t, &ready);
        assert!(t.note.contains("write access"), "{}", t.note);
    }
    // Outside ReadyToMerge.
    for p in ALL_CR_PHASES {
        if p == CrPhase::ReadyToMerge {
            continue;
        }
        let s = cr_with(p, ready_data());
        let t = run_cr(Permission::Write, &s, &merge_ev("m", "carol"));
        rejected(&t, &s);
        assert!(t.note.contains("not ready to merge"), "{p:?}: {}", t.note);
    }
    // ReadyToMerge with data that no longer adds up (defence in depth).
    for d in [
        CrData {
            last_review: "comment".into(),
            ..ready_data()
        },
        CrData {
            ci: CiState::Failure,
            ..ready_data()
        },
        CrData {
            reviewed_sha: "old".into(),
            ..ready_data()
        },
        CrData {
            mergeable: Some(false),
            ..ready_data()
        },
        CrData {
            head_sha: "moved".into(),
            ..ready_data()
        },
    ] {
        let s = cr_with(CrPhase::ReadyToMerge, d);
        rejected(&run_cr(Permission::Write, &s, &merge_ev("m", "carol")), &s);
    }
}

#[test]
fn merge_enqueues_exactly_one_conventional_squash() {
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    let t = run_cr(Permission::Write, &ready, &merge_ev("m1", "carol"));
    let d = assert_cr(&t, CrPhase::Merging);
    assert_eq!(d.head_sha, "sha1");
    assert_eq!(jobs(&t).len(), 1, "{:?}", t.effects);
    let (_, key, input) = job_of(&t, JobKind::MergeChangeRequest);
    assert_eq!(key, "github:enzarb/enzarb-ui#43:m1:merge_change_request");
    assert_eq!(input["number"], 43);
    assert_eq!(input["expected_head_sha"], "sha1");
    assert_eq!(input["actor"], "carol");
    // "Dark mode" on branch feat/dark
    assert_eq!(input["title"], "feat: Dark mode");

    // An already conventional title is kept.
    let s = cr_with(
        CrPhase::ReadyToMerge,
        CrData {
            title: "fix(ui): crash".into(),
            ..ready_data()
        },
    );
    let t = run_cr(Permission::Write, &s, &merge_ev("m2", "carol"));
    assert_eq!(
        job_of(&t, JobKind::MergeChangeRequest).2["title"],
        "fix(ui): crash"
    );
    // A second click is no longer ready: nothing more is queued.
    rejected(
        &run_cr(Permission::Write, &t.next, &merge_ev("m3", "carol")),
        &t.next,
    );
}

#[test]
fn merge_results() {
    let merging = merging_state();
    // merged
    let t = run_cr(
        Permission::None,
        &merging,
        &merge_result(
            "r",
            json!({ "outcome": "merged", "actor": "carol", "sha": "abc" }),
        ),
    );
    assert_cr(&t, CrPhase::Merged);
    assert!(comment_bodies(&t)[0].contains("merged by @carol"));
    assert_eq!(progress_values(&t), ["Done"]);
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    let t = run_cr(
        Permission::None,
        &merging,
        &merge_result(
            "r",
            json!({ "outcome": "already_merged", "actor": "carol" }),
        ),
    );
    assert_cr(&t, CrPhase::Merged);

    // head moved: review the new commits, no retry
    let t = run_cr(
        Permission::None,
        &merging,
        &merge_result("r", json!({ "outcome": "head_moved", "actor": "carol" })),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert!(!d.merge_error.is_empty());
    assert!(d.last_review.is_empty());
    assert_eq!(
        kinds(&t)
            .iter()
            .filter(|k| **k == JobKind::AgentReview)
            .count(),
        1
    );
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));

    // not mergeable: held with the reason, no retry
    let t = run_cr(
        Permission::None,
        &merging,
        &merge_result(
            "r",
            json!({ "outcome": "not_mergeable", "actor": "carol", "reason": "conflicts" }),
        ),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert_eq!(
        (d.merge_error.as_str(), d.mergeable),
        ("conflicts", Some(false))
    );
    assert!(jobs(&t).is_empty());
    let r = readiness(&d);
    assert_eq!(r.mergeable.status, CheckStatus::Failed);
    assert_eq!(r.mergeable.detail, "conflicts");

    // Unusable output and job failure fail the item; retry never merges.
    for ev in [
        merge_result("r", json!({ "nonsense": true })),
        failed("r", JobKind::MergeChangeRequest),
    ] {
        let t = run_cr(Permission::None, &merging, &ev);
        let d = assert_cr(&t, CrPhase::Failed);
        assert_eq!(d.failed_phase, Some(CrPhase::Merging));
        let t = run_cr(
            Permission::Write,
            &t.next,
            &ui("rt", "carol", UiActionKind::Retry),
        );
        assert_cr(&t, CrPhase::AwaitingChanges);
        assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
        job_of(&t, JobKind::CheckMergeReadiness);
    }
    // Results outside Merging are stale; an external merge still lands in Merged.
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    rejected(
        &run_cr(
            Permission::None,
            &ready,
            &merge_result("r", json!({ "outcome": "merged" })),
        ),
        &ready,
    );
    assert_cr(
        &run_cr(
            Permission::None,
            &merging,
            &closed("cl", pr_item("sha1"), true),
        ),
        CrPhase::Merged,
    );
}

#[test]
fn merge_error_is_cleared_when_ready_again() {
    let s = cr_with(
        CrPhase::AwaitingChanges,
        CrData {
            merge_error: "conflicts".into(),
            mergeable: None,
            ..ready_data()
        },
    );
    let t = run_cr(
        Permission::None,
        &s,
        &checked("c", readiness_out("sha1", "success", json!(true))),
    );
    assert_eq!(assert_cr(&t, CrPhase::ReadyToMerge).merge_error, "");
}

#[test]
fn old_state_loads_and_exposes_the_checklist() {
    let old = json!({
        "number": 43, "head_sha": "sha1", "reviewed_sha": "sha1", "last_review": "approve",
        "title": "Dark mode", "head_ref": "feat/dark"
    });
    let s = WorkflowState::from_row("change_request", 1, "AwaitingChanges", &old).unwrap();
    let data = s.state_data();
    assert_eq!(data["ci"], "unknown");
    assert_eq!(data["mergeable"], Value::Null);
    assert_eq!(data["checklist"]["review"]["status"], "ok");
    assert_eq!(data["checklist"]["ci"]["status"], "unknown");
    assert_eq!(data["checklist"]["mergeable"]["status"], "unknown");

    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    let data = ready.state_data();
    for k in ["ci", "review", "mergeable"] {
        assert_eq!(data["checklist"][k]["status"], "ok", "{k}");
        assert!(!data["checklist"][k]["detail"].as_str().unwrap().is_empty());
    }
    assert_eq!(data["checklist"]["ci"]["detail"], "CI passed on sha1");
    // The derived checklist is ignored on load.
    let back = WorkflowState::from_row("change_request", 1, "ReadyToMerge", &data).unwrap();
    assert_eq!(back, ready);
    for p in ["ReadyToMerge", "Merging"] {
        assert!(WorkflowState::from_row("change_request", 1, p, &json!({})).is_ok());
    }
}

#[test]
fn only_a_ui_merge_reaches_merging() {
    use enzarb_connector::CiState as Ev;
    let mut events: Vec<EngineEvent> = Vec::new();
    for ty in EventType::ALL {
        let mut e = conn(
            &format!("ev-{}", ty.as_str()),
            ty,
            pr_item("sha1"),
            actor("carol"),
            Some((9, "@enzarb please")),
        );
        if let EventPayload::Connector(c) = &mut e.payload {
            c.data.merged = Some(true);
            c.data.review = Some(Review {
                id: 1,
                state: "CHANGES_REQUESTED".into(),
                body: "no".into(),
            });
            c.data.ci = Some(enzarb_connector::CiInfo {
                head_sha: "sha1".into(),
                state: Ev::Success,
                context: None,
                url: None,
            });
        }
        events.push(e);
    }
    for (i, st) in [Ev::Success, Ev::Failure, Ev::Pending]
        .into_iter()
        .enumerate()
    {
        events.push(ci_event(&format!("ci{i}"), "sha1", st));
    }
    events.push(pr_updated("upd", "sha1"));
    events.push(pr_updated("upd2", "sha2"));
    events.push(pr_comment("cm", "carol", 5, "fix it"));
    for a in [
        UiActionKind::Approve,
        UiActionKind::Cancel,
        UiActionKind::Retry,
    ] {
        events.push(ui("ui", "carol", a));
    }
    events.push(timer("t1", None));
    events.push(timer("t2", Some(1)));
    let all_kinds = [
        JobKind::SetProgress,
        JobKind::PostComment,
        JobKind::OpenChangeRequest,
        JobKind::AgentTriage,
        JobKind::AgentPlan,
        JobKind::AgentImplement,
        JobKind::AgentReview,
        JobKind::AgentAddressComments,
        JobKind::AgentTask,
        JobKind::CheckMergeReadiness,
        JobKind::MergeChangeRequest,
    ];
    let outputs = [
        json!({}),
        readiness_out("sha1", "success", json!(true)),
        json!({ "outcome": "merged", "actor": "carol" }),
        agent_output("approve", "ok"),
        agent_output("done", "ok"),
    ];
    for kind in all_kinds {
        for (i, out) in outputs.iter().enumerate() {
            events.push(ok(&format!("ok-{kind:?}-{i}"), kind, out.clone()));
        }
        events.push(failed(&format!("fail-{kind:?}"), kind));
    }

    let mut count = 0;
    for phase in ALL_CR_PHASES {
        for data in [ready_data(), cr_data()] {
            let s = cr_with(phase, data);
            for ev in &events {
                for perm in [Permission::Admin, Permission::Read] {
                    let t = run_cr(perm, &s, ev);
                    count += 1;
                    let reached = matches!(
                        &t.next,
                        WorkflowState::ChangeRequest(n) if n.phase == CrPhase::Merging
                    );
                    if phase != CrPhase::Merging {
                        assert!(!reached, "{phase:?} + {ev:?} reached Merging");
                    }
                    assert!(
                        !kinds(&t).contains(&JobKind::MergeChangeRequest),
                        "{phase:?} + {ev:?} queued a merge"
                    );
                }
            }
        }
    }
    assert!(count > 1000);

    // ... and the UI merge does, from ReadyToMerge only, with write access only.
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    for perm in [Permission::Admin, Permission::Write] {
        let t = run_cr(perm, &ready, &merge_ev("m", "carol"));
        assert_cr(&t, CrPhase::Merging);
        assert_eq!(kinds(&t), [JobKind::MergeChangeRequest]);
    }
}

fn merge_ev_delete(id: &str, who: &str, delete_branch: bool) -> EngineEvent {
    let mut e = merge_ev(id, who);
    if let EventPayload::Ui(u) = &mut e.payload {
        u.data = json!({ "delete_branch": delete_branch });
    }
    e
}

#[test]
fn merge_asks_for_branch_deletion_only_when_the_user_did() {
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    let head_ref = ready_data().head_ref;
    for (ev, want) in [
        (merge_ev("a", "carol"), None),
        (merge_ev_delete("b", "carol", false), None),
        (merge_ev_delete("c", "carol", true), Some(head_ref)),
    ] {
        let t = run_cr(Permission::Write, &ready, &ev);
        let (_, _, input) = job_of(&t, JobKind::MergeChangeRequest);
        assert_eq!(
            input.get("delete_branch").and_then(Value::as_str),
            want.as_deref()
        );
    }
}

#[test]
fn branch_deletion_request_survives_the_branch_update() {
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    let t = run_cr(
        Permission::Write,
        &ready,
        &merge_ev_delete("m", "carol", true),
    );
    let t = run_cr(
        Permission::None,
        &t.next,
        &merge_result(
            "u",
            json!({ "outcome": "updated_branch", "actor": "carol" }),
        ),
    );
    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok_checks("r", JobKind::AgentReview, "approve", verified()),
    );
    let t = run_cr(
        Permission::None,
        &t.next,
        &checked("c", readiness_out("sha1", "success", json!(true))),
    );
    let (_, _, input) = job_of(&t, JobKind::MergeChangeRequest);
    assert_eq!(input["delete_branch"], json!(ready_data().head_ref));
}

#[test]
fn merged_note_says_what_became_of_the_branch() {
    let state = merging_state();
    let t = run_cr(
        Permission::None,
        &state,
        &merge_result("1", json!({ "outcome": "merged", "actor": "carol" })),
    );
    assert_eq!(t.note, "merged");
    let t = run_cr(
        Permission::None,
        &state,
        &merge_result(
            "2",
            json!({ "outcome": "merged", "actor": "carol",
                    "branch": "branch enzarb/x not deleted: permission denied" }),
        ),
    );
    assert_cr(&t, CrPhase::Merged);
    assert_eq!(
        t.note,
        "merged; branch enzarb/x not deleted: permission denied"
    );
}

#[test]
fn behind_branch_is_updated_reviewed_and_not_merged() {
    let ready = cr_with(CrPhase::ReadyToMerge, ready_data());
    let t = run_cr(Permission::Write, &ready, &merge_ev("m", "carol"));
    let d = assert_cr(&t, CrPhase::Merging);
    assert!((d.merge_requested, d.merge_actor.as_str()) == (true, "carol"));
    let t = run_cr(
        Permission::None,
        &t.next,
        &merge_result(
            "u",
            json!({ "outcome": "updated_branch", "actor": "carol" }),
        ),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert!(d.last_review.is_empty());
    assert_eq!((d.ci, d.mergeable), (CiState::Unknown, None));
    assert!(d.merge_requested, "the merge request survives the update");
    assert!(kinds(&t).contains(&JobKind::AgentReview));
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    assert_wait(CrPhase::Reviewing, d, "enzarb", "updating_branch");
    // The new head is approved but CI is unknown: nothing merges yet.
    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok_checks("r", JobKind::AgentReview, "approve", verified()),
    );
    assert_cr(&t, CrPhase::AwaitingChanges);
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    // CI green and mergeable: the merge resumes without a second click.
    let t = run_cr(
        Permission::None,
        &t.next,
        &checked("c", readiness_out("sha1", "success", json!(true))),
    );
    let d = assert_cr(&t, CrPhase::Merging);
    assert!(d.merge_requested);
    let (_, _, input) = job_of(&t, JobKind::MergeChangeRequest);
    assert_eq!(input["actor"], "carol");
    assert_eq!(input["expected_head_sha"], "sha1");
}

#[test]
fn merge_request_is_cleared_by_failed_check_or_requested_changes() {
    let pending = CrData {
        merge_requested: true,
        merge_actor: "carol".into(),
        ci: CiState::Unknown,
        ci_sha: String::new(),
        mergeable: None,
        ..ready_data()
    };
    // A failed check: waits like any other, and the intent is gone.
    let t = run_cr(
        Permission::None,
        &cr_with(CrPhase::AwaitingChanges, pending.clone()),
        &checked("c", readiness_out("sha1", "failure", json!(true))),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert!(!d.merge_requested);
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    // CI going red while reviewing also clears it.
    let t = run_cr(
        Permission::None,
        &cr_with(CrPhase::Reviewing, pending.clone()),
        &ci_event("f", "sha1", enzarb_connector::CiState::Failure),
    );
    assert!(!assert_cr(&t, CrPhase::Reviewing).merge_requested);
    // Requested changes from the review.
    let t = run_cr_manual(
        Permission::None,
        &cr_with(CrPhase::Reviewing, pending),
        &agent_ok_checks("r", JobKind::AgentReview, "request_changes", verified()),
    );
    assert!(!assert_cr(&t, CrPhase::AwaitingChanges).merge_requested);
    // A refused merge.
    let mut merging = ready_data();
    merging.merge_requested = true;
    let t = run_cr(
        Permission::None,
        &cr_with(CrPhase::Merging, merging),
        &merge_result("n", json!({ "outcome": "not_mergeable", "reason": "x" })),
    );
    assert!(!assert_cr(&t, CrPhase::AwaitingChanges).merge_requested);
}

fn wait_of(phase: CrPhase, data: CrData) -> (String, String) {
    let v = cr_with(phase, data).state_data();
    (
        v["wait"]["waiting_on"].as_str().unwrap().to_string(),
        v["wait"]["reason"].as_str().unwrap().to_string(),
    )
}

fn assert_wait(phase: CrPhase, data: CrData, on: &str, reason: &str) {
    assert_eq!(
        wait_of(phase, data),
        (on.into(), reason.into()),
        "{phase:?}"
    );
}

#[test]
fn wait_reason_per_phase() {
    for (phase, on, reason) in [
        (CrPhase::Reviewing, "enzarb", "reviewing"),
        (CrPhase::AddressingComments, "enzarb", "addressing_comments"),
        (CrPhase::ResolvingConflicts, "enzarb", "resolving_conflict"),
        (CrPhase::ReadyToMerge, "human", "ready_to_merge"),
        (CrPhase::Merging, "github", "merging"),
        (CrPhase::Failed, "human", "enzarb_failed"),
        (CrPhase::ConflictParked, "human", "conflict_parked"),
    ] {
        assert_wait(phase, ready_data(), on, reason);
    }
    for phase in [CrPhase::Merged, CrPhase::Closed, CrPhase::Cancelled] {
        let v = cr_with(phase, ready_data()).state_data();
        assert_eq!(v["wait"], Value::Null, "{phase:?}");
    }
}

#[test]
fn wait_reason_while_awaiting_changes() {
    let p = CrPhase::AwaitingChanges;
    // Review posted for the head but it does not approve: a human has to review.
    let comment = CrData {
        last_review: "comment".into(),
        ..cr_data()
    };
    assert_wait(p, comment, "human", "awaiting_review");
    // No review, or one of an older commit: enzarb is (about to be) reviewing.
    assert_wait(p, cr_data(), "enzarb", "reviewing");
    let stale = CrData {
        reviewed_sha: "old".into(),
        ..ready_data()
    };
    assert_wait(p, stale, "enzarb", "reviewing");
    let changes = CrData {
        last_review: "request_changes".into(),
        ..cr_data()
    };
    assert_wait(p, changes, "enzarb", "changes_requested_fixing");
    let ci_failed = CrData {
        ci: CiState::Failure,
        ..ready_data()
    };
    assert_wait(p, ci_failed, "enzarb", "ci_failed_fixing");
    let ci_pending = CrData {
        ci: CiState::Pending,
        ..ready_data()
    };
    assert_wait(p, ci_pending, "ci", "ci_pending");
    assert_wait(p, unknown_data(), "ci", "ci_pending");
    let conflicts = CrData {
        mergeable: Some(false),
        ..ready_data()
    };
    assert_wait(p, conflicts, "human", "conflict_parked");
    let v = cr_with(p, unknown_data()).state_data();
    assert_eq!(v["wait"]["text"], "Waiting for CI");
    assert!(v["wait"].get("url").is_none());
}
