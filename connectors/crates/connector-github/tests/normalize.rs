//! Fixture -> golden tests for webhook normalization. Regenerate goldens with
//! `UPDATE_GOLDEN=1 cargo test -p enzarb-connector-github --test normalize`
//! and review the diff.

use std::path::PathBuf;

use enzarb_connector::CiState;
use enzarb_connector::EventType::{self, *};
use enzarb_connector_github::{Normalized, normalize};

const HANDLE: &str = "enzarb";
const LOGIN: &str = "enzarb[bot]";

fn dir(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(sub)
}

fn run(event: &str, fixture: &str) -> Normalized {
    let body = std::fs::read(dir("fixtures").join(format!("{fixture}.json"))).unwrap();
    normalize(event, &format!("d-{fixture}"), &body, HANDLE, LOGIN).unwrap()
}

fn types(n: &Normalized) -> Vec<EventType> {
    match n {
        Normalized::Events { events } => events.iter().map(|e| e.event_type).collect(),
        Normalized::Installation { .. } => panic!("installation change"),
        Normalized::CommitStatus { .. } => panic!("commit status"),
    }
}

/// CI fixtures are asserted individually below instead of via goldens.
const CI_FIXTURES: &[&str] = &[
    "check_suite_completed_success",
    "check_suite_completed_failure",
    "check_suite_completed_no_pr",
    "check_run_completed",
    "check_run_by_bot_app",
    "check_run_by_bot_sender",
    "status_success",
    "status_pending",
    "status_by_bot",
];

fn ci_events_of(event: &str, fixture: &str) -> Vec<enzarb_connector::Event> {
    match run(event, fixture) {
        Normalized::Events { events } => events,
        other => panic!("{fixture}: unexpected {other:?}"),
    }
}

#[test]
fn check_suite_completed_maps_to_pr() {
    let evs = ci_events_of("check_suite", "check_suite_completed_success");
    assert_eq!(evs.len(), 1);
    let e = &evs[0];
    assert_eq!(e.event_type, CiStatusChanged);
    assert_eq!(e.id, "d-check_suite_completed_success");
    assert_eq!(e.subject.to_string(), "github:enzarb/enzarb-ui#57");
    let ci = e.data.ci.as_ref().unwrap();
    assert_eq!(ci.state, CiState::Success);
    assert_eq!(ci.head_sha, "9fceb02d0ae598e95dc970b74767f19372d61af8");
    assert_eq!(e.data.item.head_ref.as_deref(), Some("enzarb/issue-12"));
    assert_eq!(e.data.item.base_ref.as_deref(), Some("main"));
    assert_eq!(
        e.data.item.url,
        "https://github.com/enzarb/enzarb-ui/pull/57"
    );

    let evs = ci_events_of("check_suite", "check_suite_completed_failure");
    assert_eq!(evs[0].data.ci.as_ref().unwrap().state, CiState::Failure);
}

#[test]
fn check_run_completed_emits_one_event_per_pr() {
    let evs = ci_events_of("check_run", "check_run_completed");
    assert_eq!(evs.len(), 2);
    assert_eq!(evs[0].id, "d-check_run_completed");
    assert_eq!(evs[1].id, "d-check_run_completed#pr58");
    assert_ne!(evs[0].dedupe_key(), evs[1].dedupe_key());
    let ci = evs[0].data.ci.as_ref().unwrap();
    assert_eq!(ci.state, CiState::Failure); // timed_out
    assert_eq!(ci.context.as_deref(), Some("build"));
    assert_eq!(
        ci.url.as_deref(),
        Some("https://github.com/enzarb/enzarb-ui/runs/8001")
    );
    assert_eq!(evs[1].subject.number, 58);
}

#[test]
fn ci_without_a_pr_or_from_the_bot_is_dropped() {
    assert!(ci_events_of("check_suite", "check_suite_completed_no_pr").is_empty());
    // Owned by the bot's App although a human is the sender.
    assert!(ci_events_of("check_run", "check_run_by_bot_app").is_empty());
    // Sent by the bot although the run belongs to another App.
    assert!(ci_events_of("check_run", "check_run_by_bot_sender").is_empty());
    assert!(run("status", "status_by_bot").into_events().is_empty());
    assert!(matches!(
        run("status", "status_by_bot"),
        Normalized::Events { .. }
    ));
}

#[test]
fn only_completed_check_actions_are_mapped() {
    let body = std::fs::read(dir("fixtures").join("check_run_completed.json")).unwrap();
    let text = String::from_utf8(body).unwrap().replacen(
        "\"action\": \"completed\"",
        "\"action\": \"created\"",
        1,
    );
    let n = normalize("check_run", "d", text.as_bytes(), HANDLE, LOGIN).unwrap();
    assert_eq!(types(&n), []);
}

#[test]
fn status_becomes_commit_status() {
    let Normalized::CommitStatus { status } = run("status", "status_success") else {
        panic!("expected commit status");
    };
    assert_eq!(status.delivery, "d-status_success");
    assert_eq!(status.repo.to_string(), "github:enzarb/enzarb-ui");
    assert_eq!(status.sha, "9fceb02d0ae598e95dc970b74767f19372d61af8");
    assert_eq!(status.state, CiState::Success);
    assert_eq!(status.context.as_deref(), Some("ci/jenkins"));
    assert_eq!(status.url.as_deref(), Some("https://ci.example.com/job/1"));
    assert_eq!(status.actor.login, "ci-user");

    let Normalized::CommitStatus { status } = run("status", "status_pending") else {
        panic!("expected commit status");
    };
    assert_eq!(status.state, CiState::Pending);
    assert_eq!(status.url, None);
}

/// (X-GitHub-Event, fixture name, expected event types in order)
const CASES: &[(&str, &str, &[EventType])] = &[
    ("issues", "issues_opened", &[WorkItemOpened, Mentioned]),
    ("issues", "issues_opened_plain", &[WorkItemOpened]),
    ("issues", "issues_closed", &[WorkItemClosed]),
    ("issues", "issues_labeled_approved", &[ApprovalGranted]),
    ("issues", "issues_labeled_other", &[]),
    ("issues", "issues_edited", &[]),
    ("issue_comment", "issue_comment_created", &[CommentAdded]),
    (
        "issue_comment",
        "issue_comment_mention",
        &[CommentAdded, Mentioned],
    ),
    (
        "issue_comment",
        "issue_comment_approve",
        &[CommentAdded, ApprovalGranted],
    ),
    (
        "issue_comment",
        "issue_comment_mention_approve_on_pr",
        &[CommentAdded, Mentioned, ApprovalGranted],
    ),
    (
        "issue_comment",
        "issue_comment_quoted_approve",
        &[CommentAdded],
    ),
    ("issue_comment", "issue_comment_by_bot", &[]),
    ("issue_comment", "issue_comment_edited", &[]),
    (
        "pull_request",
        "pull_request_opened",
        &[ChangeRequestOpened, Mentioned],
    ),
    ("pull_request", "pull_request_opened_by_bot", &[]),
    (
        "pull_request",
        "pull_request_reopened",
        &[ChangeRequestOpened, Mentioned],
    ),
    (
        "pull_request",
        "pull_request_synchronize",
        &[ChangeRequestUpdated],
    ),
    (
        "pull_request",
        "pull_request_closed_merged",
        &[WorkItemClosed],
    ),
    (
        "pull_request",
        "pull_request_closed_unmerged",
        &[WorkItemClosed],
    ),
    (
        "pull_request",
        "pull_request_labeled_approved",
        &[ApprovalGranted],
    ),
    (
        "pull_request_review_comment",
        "pull_request_review_comment_created",
        &[CommentAdded, Mentioned],
    ),
    (
        "pull_request_review",
        "pull_request_review_submitted",
        &[ReviewSubmitted],
    ),
    (
        "pull_request_review",
        "pull_request_review_submitted_changes",
        &[ReviewSubmitted],
    ),
    ("ping", "ping", &[]),
];

const INSTALLATION_CASES: &[(&str, &str)] = &[
    ("installation", "installation_created"),
    ("installation", "installation_deleted"),
    (
        "installation_repositories",
        "installation_repositories_added",
    ),
    (
        "installation_repositories",
        "installation_repositories_removed",
    ),
];

fn check_golden(fixture: &str, n: &Normalized) {
    let path = dir("golden").join(format!("{fixture}.json"));
    let mut actual = serde_json::to_string_pretty(n).unwrap();
    actual.push('\n');
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {path:?}; run with UPDATE_GOLDEN=1"));
    assert_eq!(actual, want, "golden mismatch for {fixture}");
}

#[test]
fn event_types_per_fixture() {
    for (event, fixture, want) in CASES {
        let n = run(event, fixture);
        assert_eq!(&types(&n), want, "{fixture}");
    }
}

#[test]
fn golden_output() {
    for (event, fixture, _) in CASES {
        check_golden(fixture, &run(event, fixture));
    }
    for (event, fixture) in INSTALLATION_CASES {
        check_golden(fixture, &run(event, fixture));
    }
}

#[test]
fn every_fixture_is_covered() {
    let covered: Vec<&str> = CASES
        .iter()
        .map(|c| c.1)
        .chain(INSTALLATION_CASES.iter().map(|c| c.1))
        .chain(CI_FIXTURES.iter().copied())
        .collect();
    for f in std::fs::read_dir(dir("fixtures")).unwrap() {
        let name = f.unwrap().file_name().into_string().unwrap();
        if let Some(stem) = name.strip_suffix(".json") {
            assert!(covered.contains(&stem), "fixture {stem} has no test case");
        }
    }
}

#[test]
fn derived_events_get_distinct_dedupe_keys() {
    let Normalized::Events { events } = run("issue_comment", "issue_comment_mention_approve_on_pr")
    else {
        panic!()
    };
    let keys: Vec<_> = events.iter().map(|e| e.dedupe_key()).collect();
    assert_eq!(
        keys,
        [
            "github:d-issue_comment_mention_approve_on_pr",
            "github:d-issue_comment_mention_approve_on_pr#mentioned",
            "github:d-issue_comment_mention_approve_on_pr#approved",
        ]
    );
    // Same subject and comment on all three.
    assert!(
        events
            .iter()
            .all(|e| e.subject.to_string() == "github:enzarb/enzarb-ui#57")
    );
    assert!(
        events
            .iter()
            .all(|e| e.data.comment.as_ref().unwrap().id == 990)
    );
}

#[test]
fn pr_comment_is_a_change_request_item() {
    let Normalized::Events { events } = run("issue_comment", "issue_comment_mention_approve_on_pr")
    else {
        panic!()
    };
    assert_eq!(
        events[0].data.item.kind,
        enzarb_connector::ItemKind::ChangeRequest
    );
}

#[test]
fn merged_flag() {
    let merged = |f: &str| match run("pull_request", f) {
        Normalized::Events { events } => events[0].data.merged,
        _ => panic!(),
    };
    assert_eq!(merged("pull_request_closed_merged"), Some(true));
    assert_eq!(merged("pull_request_closed_unmerged"), Some(false));
}

#[test]
fn custom_bot_identity() {
    let body = std::fs::read(dir("fixtures").join("issue_comment_mention.json")).unwrap();
    // Different handle: the @enzarb mention no longer counts.
    let n = normalize("issue_comment", "d", &body, "other-bot", LOGIN).unwrap();
    assert_eq!(types(&n), [CommentAdded]);
    // The sender being the configured bot drops the event.
    let body = std::fs::read(dir("fixtures").join("issue_comment_created.json")).unwrap();
    let n = normalize("issue_comment", "d", &body, HANDLE, "ruckc").unwrap();
    assert_eq!(types(&n), []);
}

#[test]
fn malformed_and_incomplete_payloads_error() {
    assert!(normalize("issues", "d", b"not json", HANDLE, LOGIN).is_err());
    // Missing issue on a mapped action.
    let e = normalize(
        "issues",
        "d",
        br#"{"action":"opened","sender":{"login":"a","id":1},"repository":{"name":"r","owner":{"login":"o","id":2}}}"#,
        HANDLE,
        LOGIN,
    );
    assert!(e.is_err());
    // Unknown events are ignored, not errors.
    let n = normalize("star", "d", br#"{"action":"created"}"#, HANDLE, LOGIN).unwrap();
    assert!(n.into_events().is_empty());
}
