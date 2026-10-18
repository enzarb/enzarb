//! Contract tests for `merge_pull_request` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": `merge_change_request` is guarded by the expected head and
//! idempotent per key).

use enzarb_connector::fake::{Call, FakeConnector};
use enzarb_connector::{Error, MergeOutcome};
use serde_json::json;

use super::common::*;
use crate::actions::mutation::{Mutation, activate};

fn merge_calls(c: &FakeConnector) -> Vec<Call> {
    applied(c)
        .into_iter()
        .filter(|c| matches!(c, Call::MergeChangeRequest { .. }))
        .collect()
}

/// Without `expected_head` the merge is guarded by the head `get_merge_readiness` reports.
fn check_head_from_readiness(c: &FakeConnector) -> Result<(), String> {
    let calls = merge_calls(c);
    let ok = matches!(&calls[..], [Call::MergeChangeRequest { expected_head_sha, .. }] if expected_head_sha == "abc");
    ensure(ok, || format!("merge calls {calls:?}"))
}

#[tokio::test]
async fn merged_result_is_merged_and_sha() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    let out = run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    let sha = out["sha"].as_str().unwrap();
    assert!(sha.starts_with("fake-merge-"));
    assert_eq!(out, json!({"merged": true, "sha": sha}));
}

#[tokio::test]
async fn expected_head_is_passed_through() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    run(
        &c,
        "merge_pull_request",
        json!({"expected_head": "zzz"}),
        JOB,
    )
    .await
    .unwrap();
    let calls = merge_calls(&c);
    assert!(
        matches!(&calls[..], [Call::MergeChangeRequest { expected_head_sha, .. }] if expected_head_sha == "zzz")
    );
    // The readiness was not consulted.
    assert!(
        !c.calls()
            .iter()
            .any(|r| matches!(r.call, Call::GetMergeReadiness { .. }))
    );
}

#[tokio::test]
async fn head_comes_from_readiness_without_expected_head() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    check_head_from_readiness(&c).unwrap();
}

#[tokio::test]
async fn title_body_and_number_are_passed_through() {
    let c = FakeConnector::default();
    c.set_merge_readiness_immediate(item(30), readiness("abc"));
    let a = json!({"number": 30, "title": "T", "body": "B"});
    run(&c, "merge_pull_request", a, JOB).await.unwrap();
    match &merge_calls(&c)[..] {
        [
            Call::MergeChangeRequest {
                item: it,
                number,
                commit_title,
                commit_body,
                ..
            },
        ] => {
            assert_eq!((it.number, *number), (30, 30));
            assert_eq!(commit_title, "T");
            assert_eq!(commit_body.as_deref(), Some("B"));
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn already_merged_reports_already() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    c.queue_merge_outcome(MergeOutcome::AlreadyMerged {
        sha: Some("s1".into()),
    });
    let out = run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    assert_eq!(out, json!({"merged": true, "sha": "s1", "already": true}));
}

#[tokio::test]
async fn head_moved_is_refused() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    c.queue_merge_outcome(MergeOutcome::HeadMoved);
    let e = run(&c, "merge_pull_request", json!({}), JOB)
        .await
        .unwrap_err();
    assert_eq!(e, "refused: head moved");
}

#[tokio::test]
async fn not_mergeable_is_refused_with_the_reason() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    c.queue_merge_outcome(MergeOutcome::NotMergeable {
        reason: "checks failing".into(),
    });
    let e = run(&c, "merge_pull_request", json!({}), JOB)
        .await
        .unwrap_err();
    assert_eq!(e, "refused: checks failing");
}

#[tokio::test]
async fn same_key_replays_the_outcome_and_merges_once() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    let a = run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    let b = run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(merge_calls(&c).len(), 1);
    // A refused outcome replays as refused too.
    let r = FakeConnector::default();
    with_readiness(&r, "abc");
    r.queue_merge_outcome(MergeOutcome::HeadMoved);
    for _ in 0..2 {
        assert_eq!(
            run(&r, "merge_pull_request", json!({}), JOB)
                .await
                .unwrap_err(),
            "refused: head moved"
        );
    }
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    c.fail_next(Error::Transient("boom".into()));
    let e = run(&c, "merge_pull_request", json!({}), JOB)
        .await
        .unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    assert!(merge_calls(&c).is_empty());
    let a = run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    let b = run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
    assert_eq!(a, b);
    assert_eq!(merge_calls(&c).len(), 1);
}

mod mutation_tests {
    use super::*;

    #[tokio::test]
    async fn removing_the_readiness_head_fails_the_contract() {
        let _g = activate(Mutation {
            ignore_readiness_head: true,
            ..Default::default()
        });
        let c = FakeConnector::default();
        with_readiness(&c, "abc");
        run(&c, "merge_pull_request", json!({}), JOB).await.unwrap();
        assert!(check_head_from_readiness(&c).is_err());
    }
}
