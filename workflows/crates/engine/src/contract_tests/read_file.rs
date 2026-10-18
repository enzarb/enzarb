//! Contract tests for `read_file` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": `get_file` returns `None` for a missing file). A read has
//! no effect, so retrying it is always safe.

use enzarb_connector::Error;
use enzarb_connector::fake::{Call, FakeConnector};
use serde_json::json;

use super::common::*;
use crate::engine::repo_key;

fn with_file(c: &FakeConnector) {
    c.set_file(repo_key("o/r"), "main", "README.md", "hello");
}

#[tokio::test]
async fn path_is_required() {
    let c = FakeConnector::default();
    let e = run(&c, "read_file", json!({}), JOB).await.unwrap_err();
    assert!(e.starts_with("refused:"), "{e}");
    assert!(c.calls().is_empty());
}

#[tokio::test]
async fn result_is_the_content() {
    let c = FakeConnector::default();
    with_file(&c);
    let out = run(&c, "read_file", json!({"path": "README.md"}), JOB)
        .await
        .unwrap();
    assert_eq!(out, json!({"content": "hello"}));
}

#[tokio::test]
async fn ref_defaults_to_the_task_default_branch() {
    let c = FakeConnector::default();
    with_file(&c);
    c.set_file(repo_key("o/r"), "dev", "README.md", "other");
    let out = run(
        &c,
        "read_file",
        json!({"path": "README.md", "ref": "dev"}),
        JOB,
    )
    .await
    .unwrap();
    assert_eq!(out["content"], "other");
    let plain = run(&c, "read_file", json!({"path": "README.md"}), JOB)
        .await
        .unwrap();
    assert_eq!(plain["content"], "hello");
}

#[tokio::test]
async fn repo_is_passed_through() {
    let c = FakeConnector::default();
    c.set_file(repo_key("x/y"), "main", "a.txt", "from x/y");
    let out = run(
        &c,
        "read_file",
        json!({"path": "a.txt", "repo": "X/Y"}),
        JOB,
    )
    .await
    .unwrap();
    assert_eq!(out["content"], "from x/y");
}

#[tokio::test]
async fn a_missing_file_has_null_content() {
    let c = FakeConnector::default();
    let out = run(&c, "read_file", json!({"path": "nope"}), JOB)
        .await
        .unwrap();
    assert_eq!(out, json!({"content": null}));
}

#[tokio::test]
async fn reads_are_idempotent_and_cause_no_mutation() {
    let c = FakeConnector::default();
    with_file(&c);
    let a = run(&c, "read_file", json!({"path": "README.md"}), JOB)
        .await
        .unwrap();
    let b = run(&c, "read_file", json!({"path": "README.md"}), JOB)
        .await
        .unwrap();
    assert_eq!(a, b);
    assert!(
        c.calls()
            .iter()
            .all(|r| matches!(r.call, Call::GetFile { .. }))
    );
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let c = FakeConnector::default();
    with_file(&c);
    c.fail_next(Error::Transient("boom".into()));
    let e = run(&c, "read_file", json!({"path": "README.md"}), JOB)
        .await
        .unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    let out = run(&c, "read_file", json!({"path": "README.md"}), JOB)
        .await
        .unwrap();
    assert_eq!(out["content"], "hello");
}
