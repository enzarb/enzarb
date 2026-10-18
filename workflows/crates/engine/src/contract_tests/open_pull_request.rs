//! Contract tests for `open_pull_request` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": `open_change_request` is idempotent per key).

use enzarb_connector::Error;
use enzarb_connector::fake::FakeConnector;
use serde_json::json;

use super::common::*;
use crate::actions::mutation::{Mutation, activate};

/// A retried job (same key) opens one pull request.
fn check_one_pull_request_per_key(c: &FakeConnector) -> Result<(), String> {
    let n = c.change_requests().len();
    ensure(n == 1, || format!("{n} pull requests, want 1"))
}

#[tokio::test]
async fn head_and_base_default_to_task_branch_and_default_branch() {
    let c = FakeConnector::default();
    run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    let pr = &c.change_requests()[0];
    assert_eq!(pr.head_ref.as_deref(), Some("enzarb/task-7"));
    assert_eq!(pr.base_ref.as_deref(), Some("main"));
}

#[tokio::test]
async fn title_body_head_base_are_passed_through() {
    let c = FakeConnector::default();
    let a = json!({"title": "T", "body": "B", "head": "feat", "base": "dev"});
    run(&c, "open_pull_request", a, JOB).await.unwrap();
    let pr = &c.change_requests()[0];
    assert_eq!((pr.title.as_str(), pr.body.as_str()), ("T", "B"));
    assert_eq!(pr.head_ref.as_deref(), Some("feat"));
    assert_eq!(pr.base_ref.as_deref(), Some("dev"));
}

#[tokio::test]
async fn draft_and_repo_are_passed_through() {
    let c = FakeConnector::default();
    let a = json!({"title": "T", "draft": true, "repo": "Other/Repo"});
    run(&c, "open_pull_request", a, JOB).await.unwrap();
    let calls = c.applied_calls();
    let s = format!("{:?}", calls[0]);
    assert!(s.contains("draft: true"), "{s}");
    assert!(s.contains("other") && s.contains("repo"), "{s}");
    let plain = FakeConnector::default();
    run(&plain, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    assert!(format!("{:?}", plain.applied_calls()[0]).contains("draft: false"));
}

#[tokio::test]
async fn result_shape_links_the_subject() {
    let c = FakeConnector::default();
    let out = run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    let pr = &c.change_requests()[0];
    let (n, url) = (pr.number, pr.url.clone());
    let want = json!({"kind": "pull_request", "repo": "o/r", "number": n, "title": "T",
        "url": url,
        "subject": {"kind": "pull_request", "repo": "o/r", "number": n, "url": url}});
    assert_eq!(out, want);
}

#[tokio::test]
async fn same_key_opens_one_pull_request() {
    let c = FakeConnector::default();
    let a = run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    let b = run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    assert_eq!(a["number"], b["number"]);
    check_one_pull_request_per_key(&c).unwrap();
}

#[tokio::test]
async fn different_keys_open_distinct_pull_requests() {
    let c = FakeConnector::default();
    let a = run(&c, "open_pull_request", json!({"title": "T"}), "k1")
        .await
        .unwrap();
    let b = run(&c, "open_pull_request", json!({"title": "T"}), "k2")
        .await
        .unwrap();
    assert_ne!(a["number"], b["number"]);
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let c = FakeConnector::default();
    c.fail_next(Error::Transient("boom".into()));
    let e = run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    assert!(c.change_requests().is_empty());
    run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    run(&c, "open_pull_request", json!({"title": "T"}), JOB)
        .await
        .unwrap();
    check_one_pull_request_per_key(&c).unwrap();
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
        for _ in 0..2 {
            run(&c, "open_pull_request", json!({"title": "T"}), JOB)
                .await
                .unwrap();
        }
        assert!(check_one_pull_request_per_key(&c).is_err());
    }
}
