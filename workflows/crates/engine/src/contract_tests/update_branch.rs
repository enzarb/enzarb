//! Contract tests for `update_branch` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": the update is guarded by the expected head).
//! `FakeConnector::update_branch` has no idempotency key, so the retry test asserts only that
//! calling again succeeds.

use enzarb_connector::fake::{Call, FakeConnector};
use enzarb_connector::{Error, UpdateBranchOutcome};
use serde_json::json;

use super::common::*;

fn update_calls(c: &FakeConnector) -> Vec<Call> {
    applied(c)
        .into_iter()
        .filter(|c| matches!(c, Call::UpdateBranch { .. }))
        .collect()
}

#[tokio::test]
async fn result_is_updated_true() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    let out = run(&c, "update_branch", json!({}), JOB).await.unwrap();
    assert_eq!(out, json!({"updated": true}));
}

#[tokio::test]
async fn refused_outcome_is_refused_with_the_reason() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    c.queue_update_outcome(UpdateBranchOutcome::Refused {
        reason: "conflicts".into(),
    });
    let e = run(&c, "update_branch", json!({}), JOB).await.unwrap_err();
    assert_eq!(e, "refused: conflicts");
}

#[tokio::test]
async fn expected_head_is_used_when_given() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    run(&c, "update_branch", json!({"expected_head": "zzz"}), JOB)
        .await
        .unwrap();
    assert!(matches!(&update_calls(&c)[..],
        [Call::UpdateBranch { expected_head_sha, .. }] if expected_head_sha == "zzz"));
}

#[tokio::test]
async fn head_comes_from_readiness_without_expected_head() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    run(&c, "update_branch", json!({}), JOB).await.unwrap();
    assert!(matches!(&update_calls(&c)[..],
        [Call::UpdateBranch { expected_head_sha, number, .. }] if expected_head_sha == "abc" && *number == PR));
}

#[tokio::test]
async fn number_selects_the_pull_request() {
    let c = FakeConnector::default();
    c.set_merge_readiness_immediate(item(30), readiness("def"));
    run(&c, "update_branch", json!({"number": 30}), JOB)
        .await
        .unwrap();
    assert!(matches!(
        &update_calls(&c)[..],
        [Call::UpdateBranch { number: 30, .. }]
    ));
}

#[tokio::test]
async fn calling_it_again_succeeds() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    for _ in 0..2 {
        let out = run(&c, "update_branch", json!({}), JOB).await.unwrap();
        assert_eq!(out, json!({"updated": true}));
    }
}

#[tokio::test]
async fn transient_failure_then_retry_succeeds() {
    let c = FakeConnector::default();
    with_readiness(&c, "abc");
    c.fail_next(Error::Transient("boom".into()));
    let e = run(&c, "update_branch", json!({}), JOB).await.unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    run(&c, "update_branch", json!({}), JOB).await.unwrap();
}
