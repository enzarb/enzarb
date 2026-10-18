//! Contract tests for `delete_branch` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`, "Connector": deleting a branch that is already gone succeeds).

use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{Error, RepoKey};
use serde_json::json;

use super::common::*;
use crate::actions::mutation::{Mutation, activate};
use crate::engine::repo_key;

/// With no `branch` the task's own branch `enzarb/task-<id>` is deleted.
fn check_task_branch_default(c: &FakeConnector) -> Result<(), String> {
    let got = c.delete_branch_calls();
    let want = vec![(repo_key("o/r"), "enzarb/task-7".to_string())];
    ensure(got == want, || format!("deleted {got:?}, want {want:?}"))
}

#[tokio::test]
async fn branch_defaults_to_the_task_branch() {
    let c = FakeConnector::default();
    let out = run(&c, "delete_branch", json!({}), JOB).await.unwrap();
    assert_eq!(out, json!({}));
    check_task_branch_default(&c).unwrap();
}

#[tokio::test]
async fn branch_and_repo_are_passed_through() {
    let c = FakeConnector::default();
    run(
        &c,
        "delete_branch",
        json!({"branch": "feat", "repo": "x/y"}),
        JOB,
    )
    .await
    .unwrap();
    let want: Vec<(RepoKey, String)> = vec![(repo_key("x/y"), "feat".into())];
    assert_eq!(c.delete_branch_calls(), want);
}

#[tokio::test]
async fn deleting_a_missing_branch_succeeds() {
    let c = FakeConnector::default();
    // `feat` was never there, and a present branch goes away.
    run(&c, "delete_branch", json!({"branch": "feat"}), "k1")
        .await
        .unwrap();
    c.set_branch_head(repo_key("o/r"), "feat2", "abc");
    run(&c, "delete_branch", json!({"branch": "feat2"}), "k2")
        .await
        .unwrap();
    run(&c, "delete_branch", json!({"branch": "feat2"}), "k3")
        .await
        .unwrap();
}

#[tokio::test]
async fn replay_with_the_same_key_is_recorded_as_replayed() {
    let c = FakeConnector::default();
    run(&c, "delete_branch", json!({}), JOB).await.unwrap();
    run(&c, "delete_branch", json!({}), JOB).await.unwrap();
    let calls = c.calls();
    assert_eq!(calls.len(), 2);
    assert!(!calls[0].replayed && calls[1].replayed);
    assert_eq!(c.applied_calls().len(), 1);
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let c = FakeConnector::default();
    c.fail_delete_branch(Error::Transient("boom".into()));
    let e = run(&c, "delete_branch", json!({}), JOB).await.unwrap_err();
    assert!(e.starts_with("transient:"), "{e}");
    run(&c, "delete_branch", json!({}), JOB).await.unwrap();
    run(&c, "delete_branch", json!({}), JOB).await.unwrap();
    // The failed attempt, the effective retry and the replay.
    let calls = c.calls();
    assert_eq!(calls.len(), 3);
    assert!(!calls[1].replayed && calls[2].replayed);
}

mod mutation_tests {
    use super::*;

    #[tokio::test]
    async fn removing_the_task_branch_default_fails_the_contract() {
        let _g = activate(Mutation {
            wrong_branch_default: true,
            ..Default::default()
        });
        let c = FakeConnector::default();
        run(&c, "delete_branch", json!({}), JOB).await.unwrap();
        assert!(check_task_branch_default(&c).is_err());
    }
}
