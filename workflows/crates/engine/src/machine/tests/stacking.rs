//! Stacked change requests: base selection, the retarget after the parent merged, and the
//! rule that only `RetargetChangeRequest` may force-push (and only from a merged parent).

use super::*;
use crate::machine::change_request::{CheckStatus, readiness};
use crate::machine::stack::{StackParent, stack_marker};

const PARENT: &str = "github:enzarb/enzarb-ui#40";

fn parent(merged: bool, closed: bool) -> StackParent {
    StackParent {
        key: PARENT.into(),
        head_ref: "enzarb/issue-7".into(),
        head_sha: "ptip".into(),
        merged,
        closed,
    }
}

fn with_parent(p: Option<StackParent>, number: u64) -> Ctx {
    Ctx {
        stack_parent: p,
        ..ctx_for(number, Permission::Write)
    }
}

/// A stacked change request of enzarb, approved with green CI.
fn stacked() -> CrData {
    CrData {
        author: "enzarb[bot]".into(),
        head_ref: "enzarb/issue-8".into(),
        base_ref: "enzarb/issue-7".into(),
        last_review: "approve".into(),
        ci: CiState::Success,
        ci_sha: "sha1".into(),
        mergeable: Some(true),
        stacked_on: PARENT.into(),
        stack_parent_ref: "enzarb/issue-7".into(),
        stack_parent_tip: "ptip".into(),
        default_base: "main".into(),
        ..cr_data()
    }
}

fn cr_with(phase: CrPhase, data: CrData) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState { phase, data })
}

fn changed(id: &str) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::DependenciesChanged,
    }
}

fn retargeting() -> WorkflowState {
    cr_with(
        CrPhase::AwaitingChanges,
        CrData {
            stack_parent_merged: true,
            retargeting: true,
            retarget_attempts: 1,
            ..stacked()
        },
    )
}

fn retarget_done(id: &str, out: Value) -> EngineEvent {
    ok(id, JobKind::RetargetChangeRequest, out)
}

// -- base selection ----------------------------------------------------------------------

fn approve_issue(ctx: &Ctx) -> Transition {
    transition(
        ctx,
        &issue_in(IssuePhase::AwaitingApproval),
        &approval_comment("a1", "carol"),
    )
}

#[test]
fn an_issue_with_an_open_parent_starts_from_the_parents_branch() {
    let ctx = with_parent(Some(parent(false, false)), 42);
    let t = approve_issue(&ctx);
    let d = assert_issue(&t, IssuePhase::Implementing);
    assert_eq!(
        d.stack.as_ref().map(|p| p.head_ref.as_str()),
        Some("enzarb/issue-7")
    );
    let input: AgentJobInput =
        serde_json::from_value(job_of(&t, JobKind::AgentImplement).2).unwrap();
    assert_eq!(input.base_ref, "enzarb/issue-7");
    assert_eq!(input.branch, "enzarb/issue-42");
    assert!(input.push);
}

#[test]
fn without_a_parent_or_with_a_merged_one_the_base_is_the_default_branch() {
    for p in [None, Some(parent(true, false))] {
        let t = approve_issue(&with_parent(p, 42));
        let d = assert_issue(&t, IssuePhase::Implementing);
        assert!(d.stack.is_none());
        let input: AgentJobInput =
            serde_json::from_value(job_of(&t, JobKind::AgentImplement).2).unwrap();
        assert_eq!(input.base_ref, "main");
    }
}

#[test]
fn the_change_request_opens_on_the_parents_branch() {
    let mut state = issue_in(IssuePhase::Implementing);
    if let WorkflowState::Issue(s) = &mut state {
        s.data.stack = Some(parent(false, false));
    }
    // The parent merged meanwhile: nothing changes for this branch.
    let mut out = agent_output("done", "PR text");
    out["pushed"] = json!(true);
    out["branch"] = json!("enzarb/issue-42");
    let t = transition(
        &with_parent(None, 42),
        &state,
        &ok("a3", JobKind::AgentImplement, out),
    );
    assert_issue(&t, IssuePhase::Implementing);
    let spec = &job_of(&t, JobKind::OpenChangeRequest).2["spec"];
    assert_eq!(spec["base_ref"], "enzarb/issue-7");
    let body = spec["body"].as_str().unwrap();
    assert!(body.contains("Closes #42"));
    assert!(body.contains(&stack_marker(&parent(false, false))));
}

// -- recording the stack when the change request opens -----------------------------------

fn handoff_item(author: &str, base: &str, body: &str) -> ItemRef {
    ItemRef {
        author: author.into(),
        head_ref: Some("enzarb/issue-8".into()),
        base_ref: Some(base.into()),
        body: body.into(),
        ..pr_item("sha1")
    }
}

fn opened_with(item: ItemRef) -> EngineEvent {
    conn(
        "o1",
        EventType::ChangeRequestOpened,
        item,
        actor("enzarb[bot]"),
        None,
    )
}

#[test]
fn an_enzarb_change_request_with_the_marker_records_the_stack() {
    let body = format!("Closes #8\n\n{}", stack_marker(&parent(false, false)));
    let new = WorkflowState::New(WorkflowKind::ChangeRequest);
    let t = transition(
        &with_parent(None, 43),
        &new,
        &opened_with(handoff_item("enzarb[bot]", "enzarb/issue-7", &body)),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.stacked_on, PARENT);
    assert_eq!(d.stack_parent_ref, "enzarb/issue-7");
    assert_eq!(d.stack_parent_tip, "ptip");
    assert_eq!(d.default_base, "main");
    assert_eq!(d.base_ref, "enzarb/issue-7");
}

#[test]
fn a_marker_on_someone_elses_change_request_or_base_stacks_nothing() {
    let body = format!("x\n{}", stack_marker(&parent(false, false)));
    let new = WorkflowState::New(WorkflowKind::ChangeRequest);
    for item in [
        handoff_item("mallory", "enzarb/issue-7", &body),
        handoff_item("enzarb[bot]", "main", &body),
        handoff_item("enzarb[bot]", "enzarb/issue-7", "no marker"),
    ] {
        let t = transition(&with_parent(None, 43), &new, &opened_with(item));
        let d = assert_cr(&t, CrPhase::Reviewing);
        assert!(d.stacked_on.is_empty());
    }
}

#[test]
fn new_commits_keep_the_stack() {
    let s = cr_with(CrPhase::AwaitingChanges, stacked());
    let t = transition(
        &with_parent(Some(parent(false, false)), 43),
        &s,
        &pr_updated("u1", "sha9"),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.stacked_on, PARENT);
    assert_eq!(d.stack_parent_tip, "ptip");
}

// -- the retarget -------------------------------------------------------------------------

#[test]
fn a_stacked_change_request_cannot_be_ready_while_its_parent_is_open() {
    let d = stacked();
    assert_eq!(readiness(&d).dependencies.status, CheckStatus::Pending);
    let s = cr_with(CrPhase::AwaitingChanges, d);
    // Reviewed + green + mergeable, yet a CI event does not make it ready.
    let t = transition(
        &with_parent(Some(parent(false, false)), 43),
        &s,
        &changed("c1"),
    );
    rejected(&t, &s);
}

#[test]
fn the_parent_merging_queues_one_retarget_job() {
    let s = cr_with(CrPhase::AwaitingChanges, stacked());
    let t = transition(
        &with_parent(Some(parent(true, false)), 43),
        &s,
        &changed("c2"),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert!(d.retargeting);
    assert_eq!(d.retarget_attempts, 1);
    assert_eq!(kinds(&t), vec![JobKind::RetargetChangeRequest]);
    let (_, key, input) = job_of(&t, JobKind::RetargetChangeRequest);
    assert!(key.ends_with(":c2:retarget_change_request"), "{key}");
    assert_eq!(input["head_ref"], "enzarb/issue-8");
    assert_eq!(input["expected_sha"], "sha1");
    assert_eq!(input["old_base"], "ptip");
    assert_eq!(input["onto"], "main");
    assert_eq!(input["number"], 43);
    // Not ready, and nothing merges, while it is moved.
    assert_ne!(readiness(&d).dependencies.status, CheckStatus::Ok);
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));

    // A second delivery while the job runs queues nothing.
    let t2 = transition(
        &with_parent(Some(parent(true, false)), 43),
        &t.next,
        &changed("c3"),
    );
    assert!(!kinds(&t2).contains(&JobKind::RetargetChangeRequest));
}

#[test]
fn a_closed_parent_keeps_the_child_blocked() {
    let s = cr_with(CrPhase::AwaitingChanges, stacked());
    let t = transition(
        &with_parent(Some(parent(false, true)), 43),
        &s,
        &changed("c4"),
    );
    let d = assert_cr(&t, CrPhase::AwaitingChanges);
    assert!(d.stack_parent_closed);
    assert!(kinds(&t).is_empty(), "{:?}", kinds(&t));
    assert_eq!(readiness(&d).dependencies.status, CheckStatus::Failed);
    assert_eq!(d.base_ref, "enzarb/issue-7");
}

#[test]
fn a_successful_retarget_moves_the_base_and_reviews_the_new_head() {
    let t = transition(
        &with_parent(Some(parent(true, false)), 43),
        &retargeting(),
        &retarget_done(
            "r1",
            json!({"outcome": "retargeted", "head_sha": "rebased"}),
        ),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(d.base_ref, "main");
    assert_eq!(d.head_sha, "rebased");
    assert!(d.stacked_on.is_empty() && !d.retargeting);
    assert!(kinds(&t).contains(&JobKind::AgentReview));
    assert!(!kinds(&t).contains(&JobKind::MergeChangeRequest));
    assert!(
        comment_bodies(&t)
            .iter()
            .any(|c| c.contains("retargeted onto `main`"))
    );
}

#[test]
fn a_conflict_parks_the_item_for_a_person_and_pushes_nothing() {
    let t = transition(
        &with_parent(Some(parent(true, false)), 43),
        &retargeting(),
        &retarget_done("r2", json!({"outcome": "conflict", "reason": "src/a.rs"})),
    );
    let d = assert_cr(&t, CrPhase::ConflictParked);
    assert!(d.stacked_on.is_empty() && !d.retargeting);
    assert_eq!(d.base_ref, "enzarb/issue-7");
    assert_eq!(kinds(&t), vec![JobKind::PostComment]);
    let c = comment_bodies(&t).join("\n");
    assert!(
        c.contains("conflicts") && c.contains("nothing was force-pushed"),
        "{c}"
    );
    // A parked item is not retargeted again by itself.
    let again = transition(
        &with_parent(Some(parent(true, false)), 43),
        &t.next,
        &changed("r2b"),
    );
    assert!(!kinds(&again).contains(&JobKind::RetargetChangeRequest));
}

#[test]
fn a_moved_branch_is_reviewed_and_retried_up_to_the_cap() {
    let lease = json!({"outcome": "lease_failed", "reason": "moved"});
    let t = transition(
        &with_parent(Some(parent(true, false)), 43),
        &retargeting(),
        &retarget_done("r3", lease.clone()),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        d.stacked_on, PARENT,
        "the stack is kept for the next attempt"
    );
    assert!(!d.retargeting);

    // At the cap the item is parked instead of looping.
    let mut at_cap = retargeting();
    if let WorkflowState::ChangeRequest(s) = &mut at_cap {
        s.data.retarget_attempts = crate::machine::change_request::MAX_RETARGET_ATTEMPTS;
    }
    let t = transition(
        &with_parent(Some(parent(true, false)), 43),
        &at_cap,
        &retarget_done("r4", lease),
    );
    assert_cr(&t, CrPhase::ConflictParked);
}

#[test]
fn refusals_failures_and_timeouts_flag_needs_you() {
    let ctx = with_parent(Some(parent(true, false)), 43);
    for out in [
        json!({"outcome": "foreign_commits", "reason": "commit by mallory"}),
        json!({"outcome": "not_ancestor", "reason": "parent tip not in history"}),
        json!({"outcome": "rejected", "reason": "not an enzarb branch"}),
        json!({"outcome": "retargeted", "head_sha": "n", "base_set": false, "reason": "422"}),
        // an old sandbox that ran an agent run instead
        json!({"outcome": "succeeded", "summary": "I did nothing"}),
    ] {
        let t = transition(&ctx, &retargeting(), &retarget_done("r5", out.clone()));
        let d = assert_cr(&t, CrPhase::ConflictParked);
        assert!(d.stacked_on.is_empty(), "{out}");
        assert_eq!(kinds(&t), vec![JobKind::PostComment], "{out}");
    }
    let t = transition(
        &ctx,
        &retargeting(),
        &failed("r6", JobKind::RetargetChangeRequest),
    );
    assert_cr(&t, CrPhase::ConflictParked);
    let t = transition(&ctx, &retargeting(), &timer("r7", Some(5)));
    assert_cr(&t, CrPhase::ConflictParked);
    assert!(cancels(&t), "a timed-out run is cancelled");
    let w = crate::machine::change_request::wait(
        CrPhase::ConflictParked,
        &cr_data(),
        &readiness(&cr_data()),
    )
    .unwrap();
    assert_eq!(
        w.waiting_on,
        crate::machine::change_request::WaitingOn::Human
    );
}

#[test]
fn a_retarget_result_for_an_item_that_is_not_retargeting_is_stale() {
    let s = cr_with(CrPhase::AwaitingChanges, stacked());
    let t = transition(
        &with_parent(Some(parent(false, false)), 43),
        &s,
        &retarget_done("r8", json!({"outcome": "retargeted", "head_sha": "x"})),
    );
    rejected(&t, &s);
}

// -- invariants ---------------------------------------------------------------------------

/// A retarget job (the only force-push) is queued only for a stacked change request whose
/// parent was merged, never by a merge, and at most one per event.
#[test]
fn only_a_merged_parent_queues_a_retarget() {
    let mut events: Vec<EngineEvent> = vec![changed("d1")];
    for ty in EventType::ALL {
        events.push(conn(
            &format!("ev-{}", ty.as_str()),
            ty,
            pr_item("sha1"),
            actor("carol"),
            Some((9, "@enzarb please")),
        ));
    }
    events.push(pr_updated("upd", "sha9"));
    events.push(timer("t1", None));
    events.push(timer("t2", Some(1)));
    for a in [
        UiActionKind::Approve,
        UiActionKind::Cancel,
        UiActionKind::Retry,
        UiActionKind::Merge,
    ] {
        events.push(ui("ui", "carol", a));
    }
    for kind in [
        JobKind::AgentReview,
        JobKind::AgentAddressComments,
        JobKind::AgentResolveConflicts,
        JobKind::CheckMergeReadiness,
        JobKind::MergeChangeRequest,
        JobKind::RetargetChangeRequest,
    ] {
        for (i, out) in [
            json!({}),
            json!({"outcome": "merged"}),
            json!({"outcome": "lease_failed"}),
            json!({"outcome": "retargeted", "head_sha": "n"}),
            agent_output("approve", "ok"),
        ]
        .into_iter()
        .enumerate()
        {
            events.push(ok(&format!("ok-{kind:?}-{i}"), kind, out));
        }
        events.push(failed(&format!("f-{kind:?}"), kind));
    }

    let datas = [
        cr_data(),
        stacked(),
        CrData {
            stack_parent_merged: true,
            ..stacked()
        },
    ];
    let parents = [None, Some(parent(false, false)), Some(parent(true, false))];
    let mut queued = 0;
    for phase in ALL_CR_PHASES {
        for data in &datas {
            let s = cr_with(phase, data.clone());
            for p in &parents {
                let ctx = with_parent(p.clone(), 43);
                for ev in &events {
                    let t = transition(&ctx, &s, ev);
                    let n = kinds(&t)
                        .iter()
                        .filter(|k| **k == JobKind::RetargetChangeRequest)
                        .count();
                    assert!(n <= 1, "{phase:?} + {ev:?}");
                    if n == 1 {
                        queued += 1;
                        let merged =
                            data.stack_parent_merged || p.as_ref().is_some_and(|p| p.merged);
                        assert!(
                            !data.stacked_on.is_empty() && merged,
                            "{phase:?} + {ev:?} queued a retarget for a non-stacked or unmerged item"
                        );
                        assert!(
                            !kinds(&t).contains(&JobKind::MergeChangeRequest),
                            "a retarget never merges"
                        );
                        let (_, _, input) = job_of(&t, JobKind::RetargetChangeRequest);
                        assert!(input["head_ref"].as_str().unwrap().starts_with("enzarb/"));
                        assert_ne!(input["head_ref"], input["onto"]);
                    }
                }
            }
        }
    }
    assert!(queued > 0, "the sweep never reached a retarget");
}

/// Nothing outside the sandbox's retarget module can force-push: the engine never spells a
/// forced push (the sandbox builds the lease itself, see `sandbox-container/src/retarget.rs`).
#[test]
fn the_engine_never_spells_a_force_push() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let needles = [
        concat!("--", "force"),
        concat!("force", "-with-lease"),
        concat!("force", "_with_lease"),
    ];
    let mut files = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(files.len() > 10);
    for f in files {
        if f.file_name().is_some_and(|n| n == "stacking.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for n in needles {
            assert!(!text.contains(n), "{} mentions {n}", f.display());
        }
    }
}
