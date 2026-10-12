//! Automatic conflict resolution in the change_request machine.

use serde_json::json;

use super::*;

fn unmergeable_ev(id: &str, base_sha: &str) -> EngineEvent {
    EngineEvent::from_row(
        id,
        "MergeConflictDetected",
        &json!({ "base_sha": base_sha }),
    )
    .unwrap()
}

fn cr_bot_in(phase: CrPhase) -> WorkflowState {
    WorkflowState::ChangeRequest(CrState {
        phase,
        data: CrData {
            author: "enzarb[bot]".into(),
            ..cr_data()
        },
    })
}

#[test]
fn unmergeable_starts_the_resolve_job() {
    let s = cr_bot_in(CrPhase::AwaitingChanges);
    let t = run_cr(Permission::None, &s, &unmergeable_ev("m1", "base1"));
    let d = assert_cr(&t, CrPhase::ResolvingConflicts);
    assert_eq!(d.base_sha, "base1");
    assert_eq!(d.resolve_attempted_base_sha, "base1");
    let input: AgentJobInput =
        serde_json::from_value(job_of(&t, JobKind::AgentResolveConflicts).2).unwrap();
    assert!(input.push);
    assert_eq!(input.branch, "feat/dark");
    assert!(input.prompt.contains("git merge origin/main"));

    // Done: the merge commit is reviewed; `reviewed_sha` is not pre-set.
    let mut out = agent_output("done", "merged main");
    out["head_sha"] = json!("merge1");
    let t = run_cr(
        Permission::None,
        &t.next,
        &ok("d", JobKind::AgentResolveConflicts, out),
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert_eq!(
        (d.head_sha.as_str(), d.reviewed_sha.as_str()),
        ("merge1", "sha1")
    );
    assert!(kinds(&t).contains(&JobKind::AgentReview));
    assert_eq!(comment_bodies(&t), vec!["merged main".to_string()]);

    // After the review, the same base commit does not start another attempt.
    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok("r", JobKind::AgentReview, "comment", "ok"),
    );
    let awaiting = t.next.clone();
    let t = run_cr(Permission::None, &awaiting, &unmergeable_ev("m2", "base1"));
    rejected(&t, &awaiting);
    assert!(kinds(&t).is_empty());
}

#[test]
fn unresolvable_conflict_parks_and_never_loops() {
    let s = cr_bot_in(CrPhase::AwaitingChanges);
    let t = run_cr(Permission::None, &s, &unmergeable_ev("m1", "base1"));
    let t = run_cr(
        Permission::None,
        &t.next,
        &agent_ok(
            "b",
            JobKind::AgentResolveConflicts,
            "blocked",
            "both changed parse()",
        ),
    );
    assert_cr(&t, CrPhase::ConflictParked);
    assert!(!kinds(&t).contains(&JobKind::AgentResolveConflicts));
    assert!(comment_bodies(&t)[0].contains("both changed parse()"));
    let parked = t.next.clone();

    // Same base commit, even as a different event: nothing starts.
    let t = run_cr(Permission::None, &parked, &unmergeable_ev("m2", "base1"));
    rejected(&t, &parked);
    assert!(kinds(&t).is_empty());

    // Plain comments do not resume it.
    rejected(
        &run_cr(
            Permission::Write,
            &parked,
            &pr_comment("c", "carol", 5, "x"),
        ),
        &parked,
    );

    // A new base commit allows exactly one new attempt.
    let t = run_cr(Permission::None, &parked, &unmergeable_ev("m3", "base2"));
    let d = assert_cr(&t, CrPhase::ResolvingConflicts);
    assert_eq!(d.resolve_attempted_base_sha, "base2");
    assert_eq!(
        kinds(&t)
            .iter()
            .filter(|k| **k == JobKind::AgentResolveConflicts)
            .count(),
        1
    );
    rejected(
        &run_cr(Permission::None, &t.next, &unmergeable_ev("m4", "base2")),
        &t.next,
    );
    // Further base commits during the run are only recorded.
    let t2 = run_cr(Permission::None, &t.next, &unmergeable_ev("m5", "base3"));
    let d = assert_cr(&t2, CrPhase::ResolvingConflicts);
    assert_eq!(d.base_sha, "base3");
    assert!(kinds(&t2).is_empty());

    // A writer mention or UI retry resumes a parked item.
    for ev in [
        ui("r", "carol", UiActionKind::Retry),
        conn(
            "mm",
            EventType::Mentioned,
            pr_item("sha1"),
            actor("carol"),
            Some((9, "@enzarb try again")),
        ),
    ] {
        let t = run_cr(Permission::Write, &parked, &ev);
        assert_cr(&t, CrPhase::ResolvingConflicts);
        assert!(kinds(&t).contains(&JobKind::AgentResolveConflicts));
    }
    // A human push goes back to review.
    let t = run_cr(Permission::None, &parked, &pr_updated("u", "sha9"));
    assert_cr(&t, CrPhase::Reviewing);
}

#[test]
fn unmergeable_ignores_foreign_authors_and_inactive_states() {
    let s = cr_in(CrPhase::AwaitingChanges);
    rejected(&run_cr(Permission::None, &s, &unmergeable_ev("m", "b")), &s);
    for phase in [
        CrPhase::Merged,
        CrPhase::Closed,
        CrPhase::Failed,
        CrPhase::Cancelled,
    ] {
        let s = cr_bot_in(phase);
        rejected(&run_cr(Permission::None, &s, &unmergeable_ev("m", "b")), &s);
    }
    let s = cr_bot_in(CrPhase::AwaitingChanges);
    rejected(&run_cr(Permission::None, &s, &unmergeable_ev("m", "")), &s);
}

#[test]
fn resolve_failure_and_deadline_are_retryable() {
    let s = cr_bot_in(CrPhase::ResolvingConflicts);
    for ev in [
        failed("f", JobKind::AgentResolveConflicts),
        timer("t", Some(1)),
    ] {
        let t = run_cr(Permission::None, &s, &ev);
        let d = assert_cr(&t, CrPhase::Failed);
        assert_eq!(d.failed_phase, Some(CrPhase::ResolvingConflicts));
        let r = run_cr(
            Permission::Write,
            &t.next,
            &ui("r", "carol", UiActionKind::Retry),
        );
        assert_cr(&r, CrPhase::ResolvingConflicts);
        assert!(kinds(&r).contains(&JobKind::AgentResolveConflicts));
    }
}

#[test]
fn feedback_and_pushes_during_a_resolve_are_kept() {
    let s = cr_bot_in(CrPhase::ResolvingConflicts);
    let t = run_cr(
        Permission::Write,
        &s,
        &pr_comment("c", "carol", 21, "and this"),
    );
    let d = assert_cr(&t, CrPhase::ResolvingConflicts);
    assert_eq!(d.pending.as_ref().unwrap().text, "and this");
    let t = run_cr(Permission::None, &s, &pr_updated("u", "sha7"));
    assert_eq!(assert_cr(&t, CrPhase::ResolvingConflicts).head_sha, "sha7");
    assert!(kinds(&t).is_empty());
}

#[test]
fn prompt_forbids_rewriting_history() {
    let out = prompt::render_prompt(
        JobKind::AgentResolveConflicts,
        &prompt::PromptContext {
            base_ref: "main".into(),
            head_ref: "feat/x".into(),
            ..Default::default()
        },
    );
    assert!(out.contains("git merge origin/main"));
    assert!(out.contains("Never rebase, never force-push"));
    assert!(out.contains("`blocked`"));
}
