//! Contract tests for `close_issue` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": closing a closed issue is a no-op that succeeds).

use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{CloseReason, Error};
use serde_json::json;

use super::common::*;
use crate::actions::mutation::{Mutation, activate};

/// `reason: not_planned` closes as not planned.
fn check_not_planned_reason(c: &FakeConnector) -> Result<(), String> {
    let got = c.closed_issue(&item(ISSUE));
    ensure(got == Some(CloseReason::NotPlanned), || {
        format!("closed as {got:?}")
    })
}

#[tokio::test]
async fn reason_not_planned_closes_as_not_planned() {
    let c = FakeConnector::default();
    run(&c, "close_issue", json!({"reason": "not_planned"}), JOB)
        .await
        .unwrap();
    check_not_planned_reason(&c).unwrap();
}

#[tokio::test]
async fn any_other_reason_closes_as_completed() {
    for a in [
        json!({}),
        json!({"reason": "completed"}),
        json!({"reason": "dup"}),
    ] {
        let c = FakeConnector::default();
        run(&c, "close_issue", a, JOB).await.unwrap();
        assert_eq!(c.closed_issue(&item(ISSUE)), Some(CloseReason::Completed));
    }
}

#[tokio::test]
async fn number_selects_the_issue_and_result_is_empty() {
    let c = FakeConnector::default();
    let out = run(&c, "close_issue", json!({"number": 33}), JOB)
        .await
        .unwrap();
    assert_eq!(out, json!({}));
    assert!(c.closed_issue(&item(33)).is_some());
    assert!(c.closed_issue(&item(ISSUE)).is_none());
}

#[tokio::test]
async fn closing_a_closed_issue_succeeds_and_keeps_the_first_reason() {
    let c = FakeConnector::default();
    run(&c, "close_issue", json!({"reason": "not_planned"}), "k1")
        .await
        .unwrap();
    let again = run(&c, "close_issue", json!({"reason": "completed"}), "k2")
        .await
        .unwrap();
    assert_eq!(again, json!({}));
    check_not_planned_reason(&c).unwrap();
    assert_eq!(c.applied_calls().len(), 1);
}

#[tokio::test]
async fn replay_with_the_same_key_is_a_no_op() {
    let c = FakeConnector::default();
    for _ in 0..2 {
        run(&c, "close_issue", json!({}), JOB).await.unwrap();
    }
    assert_eq!(c.applied_calls().len(), 1);
    assert_eq!(c.closed_issue(&item(ISSUE)), Some(CloseReason::Completed));
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let c = FakeConnector::default();
    c.fail_next(Error::Transient("boom".into()));
    let e = run(&c, "close_issue", json!({"reason": "not_planned"}), JOB)
        .await
        .unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    assert!(c.closed_issue(&item(ISSUE)).is_none());
    run(&c, "close_issue", json!({"reason": "not_planned"}), JOB)
        .await
        .unwrap();
    check_not_planned_reason(&c).unwrap();
}

mod mutation_tests {
    use super::*;

    #[tokio::test]
    async fn removing_not_planned_mapping_fails_the_contract() {
        let _g = activate(Mutation {
            ignore_close_reason: true,
            ..Default::default()
        });
        let c = FakeConnector::default();
        run(&c, "close_issue", json!({"reason": "not_planned"}), JOB)
            .await
            .unwrap();
        assert!(check_not_planned_reason(&c).is_err());
    }
}
