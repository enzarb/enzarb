//! Contract tests for `comment` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": `post_comment` is idempotent per key).

use enzarb_connector::Error;
use enzarb_connector::fake::FakeConnector;
use serde_json::json;

use super::common::*;
use crate::actions::mutation::{Mutation, activate};

/// `post_comment` is idempotent per key: a retried job leaves one comment.
fn check_one_comment_per_key(c: &FakeConnector) -> Result<(), String> {
    let n = c.comments(&item(ISSUE)).len();
    ensure(n == 1, || format!("{n} comments, want 1"))
}

#[tokio::test]
async fn body_is_required() {
    let c = FakeConnector::default();
    let e = run(&c, "comment", json!({}), JOB).await.unwrap_err();
    assert!(e.starts_with("refused:"), "{e}");
    assert!(c.comments(&item(ISSUE)).is_empty());
}

#[tokio::test]
async fn defaults_to_the_issue_subject() {
    let c = FakeConnector::default();
    run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap();
    assert_eq!(c.comments(&item(ISSUE))[0].comment.body, "hi");
}

#[tokio::test]
async fn number_and_on_select_the_target() {
    let c = FakeConnector::default();
    run(&c, "comment", json!({"body": "a", "number": 42}), "k1")
        .await
        .unwrap();
    run(
        &c,
        "comment",
        json!({"body": "b", "on": "pull_request"}),
        "k2",
    )
    .await
    .unwrap();
    assert_eq!(c.comments(&item(42)).len(), 1);
    assert_eq!(c.comments(&item(PR))[0].comment.body, "b");
    assert!(c.comments(&item(ISSUE)).is_empty());
}

#[tokio::test]
async fn result_is_id_and_url() {
    let c = FakeConnector::default();
    let out = run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap();
    let posted = &c.comments(&item(ISSUE))[0].comment;
    assert_eq!(out, json!({"id": posted.id, "url": posted.url}));
}

#[tokio::test]
async fn same_key_posts_once() {
    let c = FakeConnector::default();
    let first = run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap();
    let again = run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap();
    assert_eq!(first, again);
    check_one_comment_per_key(&c).unwrap();
}

#[tokio::test]
async fn different_keys_post_twice() {
    let c = FakeConnector::default();
    let a = run(&c, "comment", json!({"body": "hi"}), "k1")
        .await
        .unwrap();
    let b = run(&c, "comment", json!({"body": "hi"}), "k2")
        .await
        .unwrap();
    assert_ne!(a["id"], b["id"]);
    assert_eq!(c.comments(&item(ISSUE)).len(), 2);
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let c = FakeConnector::default();
    c.fail_next(Error::Transient("boom".into()));
    let e = run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    assert!(c.comments(&item(ISSUE)).is_empty());
    run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap();
    run(&c, "comment", json!({"body": "hi"}), JOB)
        .await
        .unwrap();
    check_one_comment_per_key(&c).unwrap();
}

mod mutation_tests {
    use super::*;

    #[tokio::test]
    async fn removing_key_pass_through_fails_the_contract() {
        let _g = activate(Mutation {
            fresh_keys: true,
            ..Default::default()
        });
        let c = FakeConnector::default();
        run(&c, "comment", json!({"body": "hi"}), JOB)
            .await
            .unwrap();
        run(&c, "comment", json!({"body": "hi"}), JOB)
            .await
            .unwrap();
        assert!(check_one_comment_per_key(&c).is_err());
    }
}
