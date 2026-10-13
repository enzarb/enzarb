//! Work items whose repository left every Project (`Ctx.orphaned`), and `RepoRemoved`.

use super::*;

const REMOVED_NOTE: &str = "enzarb: repository removed from the Project.";

fn orphan_ctx(number: u64, perm: Permission, owner: bool) -> Ctx {
    Ctx {
        orphaned: true,
        orphan_owner: owner,
        ..ctx_for(number, perm)
    }
}

#[test]
fn can_cancel_needs_write_or_orphan_owner() {
    assert!(ctx(Permission::Write).can_cancel());
    assert!(!ctx(Permission::Read).can_cancel());
    assert!(orphan_ctx(42, Permission::None, true).can_cancel());
    assert!(!orphan_ctx(42, Permission::None, false).can_cancel());
}

#[test]
fn orphan_owner_cancels_but_cannot_approve_retry_or_merge() {
    let c = orphan_ctx(42, Permission::None, true);
    let s = issue_in(IssuePhase::AwaitingApproval);
    let t = transition(&c, &s, &ui("c", "ruckc", UiActionKind::Cancel));
    assert_issue(&t, IssuePhase::Cancelled);
    assert!(cancels(&t));
    for action in [
        UiActionKind::Approve,
        UiActionKind::Retry,
        UiActionKind::Merge,
    ] {
        let t = transition(&c, &s, &ui("a", "ruckc", action));
        rejected(&t, &s);
        assert!(t.note.contains("no longer in a Project"), "{}", t.note);
    }
    let cr = cr_in(CrPhase::ReadyToMerge);
    let t = transition(
        &orphan_ctx(43, Permission::None, true),
        &cr,
        &ui("m", "ruckc", UiActionKind::Merge),
    );
    rejected(&t, &cr);
}

#[test]
fn non_owner_cannot_cancel_an_orphaned_item() {
    let c = orphan_ctx(42, Permission::None, false);
    let s = issue_in(IssuePhase::AwaitingApproval);
    rejected(
        &transition(&c, &s, &ui("c", "mallory", UiActionKind::Cancel)),
        &s,
    );
    let m = mention_in(MentionPhase::Running);
    rejected(
        &transition(&c, &m, &ui("c", "mallory", UiActionKind::Cancel)),
        &m,
    );
    let cr = cr_in(CrPhase::ReadyToMerge);
    let c = orphan_ctx(43, Permission::None, false);
    rejected(
        &transition(&c, &cr, &ui("c", "mallory", UiActionKind::Cancel)),
        &cr,
    );
}

#[test]
fn orphan_owner_cancels_change_requests_and_mentions() {
    let cr = cr_in(CrPhase::ReadyToMerge);
    let t = transition(
        &orphan_ctx(43, Permission::None, true),
        &cr,
        &ui("c", "ruckc", UiActionKind::Cancel),
    );
    assert_cr(&t, CrPhase::Cancelled);
    let m = mention_in(MentionPhase::Running);
    let t = transition(
        &orphan_ctx(42, Permission::None, true),
        &m,
        &ui("c", "ruckc", UiActionKind::Cancel),
    );
    assert_mention(&t, MentionPhase::Cancelled);
}

#[test]
fn orphaned_items_cancel_on_close_and_merge_but_reject_other_events() {
    let c = orphan_ctx(42, Permission::None, false);
    let s = issue_in(IssuePhase::AwaitingApproval);
    let t = transition(&c, &s, &closed("x", issue_item(), false));
    assert_issue(&t, IssuePhase::Cancelled);
    assert!(cancels(&t));

    let cr = cr_in(CrPhase::ReadyToMerge);
    let t = transition(
        &orphan_ctx(43, Permission::None, false),
        &cr,
        &closed("x", pr_item("sha1"), true),
    );
    assert_cr(&t, CrPhase::Merged);

    let comment = conn(
        "k",
        EventType::CommentAdded,
        issue_item(),
        actor("alice"),
        None,
    );
    let t = transition(&c, &s, &comment);
    rejected(&t, &s);
    assert!(t.note.contains("no longer in a Project"), "{}", t.note);
    let timer = EngineEvent {
        id: "t".into(),
        payload: EventPayload::TimerFired { job_id: None },
    };
    rejected(&transition(&c, &s, &timer), &s);
}

#[test]
fn repo_removed_cancels_every_active_state_without_permission() {
    let ev = ui("r", "ruckc", UiActionKind::RepoRemoved);
    for phase in ALL_ISSUE_PHASES {
        let s = issue_in(phase);
        let t = run(Permission::None, &s, &ev);
        match phase {
            IssuePhase::Done | IssuePhase::Duplicate | IssuePhase::Cancelled => rejected(&t, &s),
            _ => {
                assert_issue(&t, IssuePhase::Cancelled);
                assert!(cancels(&t), "{phase:?}");
                assert_eq!(t.note, REMOVED_NOTE);
            }
        }
    }
    for phase in ALL_CR_PHASES {
        let s = cr_in(phase);
        let t = run_cr(Permission::None, &s, &ev);
        match phase {
            CrPhase::Merged | CrPhase::Closed | CrPhase::Cancelled => rejected(&t, &s),
            _ => {
                assert_cr(&t, CrPhase::Cancelled);
                assert!(cancels(&t), "{phase:?}");
                assert_eq!(t.note, REMOVED_NOTE);
            }
        }
    }
    for phase in ALL_MENTION_PHASES {
        let s = mention_in(phase);
        let t = run(Permission::None, &s, &ev);
        if phase == MentionPhase::Cancelled {
            rejected(&t, &s);
        } else {
            assert_mention(&t, MentionPhase::Cancelled);
            assert!(cancels(&t), "{phase:?}");
            assert_eq!(t.note, REMOVED_NOTE);
        }
    }
}
