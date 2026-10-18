//! Contract tests for the `ci`, `mergeable`, `reviews` and `dependencies` facts
//! (`docs/workflows/contracts-v2.md`, "Facts" and the interpreter facts list), run against
//! `FakeConnector`. They assert the value `actions::fact` produces, through the helpers it
//! calls. `facts_mutation_tests` proves a fake that reports `mergeable` as known on the first
//! read fails the contract.

use std::time::Duration;

use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{CiIgnore, CiState, Connector, Error, MergeReadiness, RepoKey, WorkItemKey};
use serde_json::{Value, json};

use crate::actions::mutation::{Mutation, activate};
use crate::actions::{ci_fact, dependencies_fact, mergeable, mergeable_fact, reviews_fact};
use crate::engine::repo_key;

const NO_DELAY: [Duration; 2] = [Duration::ZERO; 2];

fn rk() -> RepoKey {
    repo_key("o/r")
}

fn item(n: u64) -> WorkItemKey {
    WorkItemKey::new(rk(), n)
}

fn readiness(mergeable: Option<bool>) -> MergeReadiness {
    MergeReadiness {
        head_sha: "h1".into(),
        base_sha: None,
        mergeable,
        mergeable_state: String::new(),
        ci: CiState::Pending,
    }
}

fn ensure(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what()) }
}

/// "`mergeable` is `null` on the first read, known on a later one" (GitHub computes it lazily).
async fn check_mergeable_unknown_then_known(want: bool) -> Result<(), String> {
    let c = FakeConnector::default();
    c.set_merge_readiness(item(5), readiness(Some(want)));
    let first = mergeable(&c, &item(5), &[]).await?;
    ensure(first.is_none(), || {
        format!("first read is {first:?}, want unknown")
    })?;
    let later = mergeable(&c, &item(5), &NO_DELAY).await?;
    ensure(later == Some(want), || {
        format!("later read is {later:?}, want {want}")
    })
}

#[tokio::test]
async fn mergeable_unknown_on_first_read() {
    check_mergeable_unknown_then_known(true).await.unwrap();
    // The raw connector read, which the fact is built from.
    let c = FakeConnector::default();
    c.set_merge_readiness(item(5), readiness(Some(true)));
    let raw = c.get_merge_readiness(&item(5), 5).await.unwrap();
    assert_eq!(raw.mergeable, None);
}

#[tokio::test]
async fn mergeable_known_on_later_read() {
    check_mergeable_unknown_then_known(true).await.unwrap();
    check_mergeable_unknown_then_known(false).await.unwrap();
}

#[tokio::test]
async fn mergeable_stays_unknown_when_seeded_none() {
    let c = FakeConnector::default();
    c.set_merge_readiness(item(5), readiness(None));
    let v = mergeable_fact(&c, Some(&item(5)), &NO_DELAY).await.unwrap();
    assert_eq!(v, Value::Null);
}

#[tokio::test]
async fn mergeable_null_without_pr() {
    let c = FakeConnector::default();
    assert_eq!(
        mergeable_fact(&c, None, &NO_DELAY).await.unwrap(),
        Value::Null
    );
    assert!(c.calls().is_empty());
}

#[tokio::test]
async fn dependencies_null_until_known() {
    let c = FakeConnector::default();
    c.fail_next(Error::Transient("boom".into()));
    let got = dependencies_fact(&c, &item(1)).await;
    assert!(got.is_err(), "an error is unknown, never []: {got:?}");
}

#[tokio::test]
async fn dependencies_empty_list_once_known() {
    let c = FakeConnector::default();
    assert_eq!(dependencies_fact(&c, &item(1)).await.unwrap(), vec![]);
}

#[tokio::test]
async fn dependencies_reflect_blocked_by_edges() {
    let c = FakeConnector::default();
    c.set_dependencies(item(1), vec![item(2)]);
    c.add_dependency(&item(1), &item(3), "k1").await.unwrap();
    assert_eq!(
        dependencies_fact(&c, &item(1)).await.unwrap(),
        vec![item(2), item(3)]
    );
    assert!(dependencies_fact(&c, &item(2)).await.unwrap().is_empty());
}

#[tokio::test]
async fn dependencies_reflect_edges_recorded_by_create_issues() {
    let c = FakeConnector::default();
    c.set_next_issue_number(10);
    let a = json!({"issues": [
        {"key": "a", "title": "A", "body": "", "depends_on": ["b"]},
        {"key": "b", "title": "B", "body": ""}
    ]});
    crate::actions::create_issues(&c, "o/r", a.as_object().unwrap(), "t:1")
        .await
        .map_err(|_| "create_issues failed")
        .unwrap();
    assert_eq!(
        dependencies_fact(&c, &item(11)).await.unwrap(),
        vec![item(10)]
    );
    assert!(dependencies_fact(&c, &item(10)).await.unwrap().is_empty());
}

fn seeded_ci(c: &FakeConnector) {
    c.set_ci(
        rk(),
        "h1",
        json!({"state": "failure", "failing": [{
            "name": "build", "kind": "check_run", "url": "https://x/1",
            "conclusion": "failure", "log_excerpt": "boom", "actionable": true,
            "job_id": 7, "run_id": 9
        }]}),
    );
}

#[tokio::test]
async fn ci_shape() {
    let c = FakeConnector::default();
    seeded_ci(&c);
    let v = ci_fact(&c, &rk(), Some("h1")).await.unwrap();
    assert_eq!(v["sha"], "h1");
    assert_eq!(v["state"], "failure");
    let f = &v["failing"][0];
    for k in [
        "name",
        "kind",
        "url",
        "conclusion",
        "log_excerpt",
        "actionable",
        "job_id",
        "run_id",
    ] {
        assert!(f.get(k).is_some(), "failing[].{k} missing: {f}");
    }
    // Unseeded: pending with nothing failing; `sha` is still set.
    let v = ci_fact(&c, &rk(), Some("h2")).await.unwrap();
    assert_eq!(v, json!({"state": "pending", "failing": [], "sha": "h2"}));
}

#[tokio::test]
async fn ci_without_pr_is_null() {
    let c = FakeConnector::default();
    assert_eq!(ci_fact(&c, &rk(), None).await.unwrap(), Value::Null);
    assert!(c.calls().is_empty());
}

#[tokio::test]
async fn ci_ignore_regex_drops_matching_checks() {
    let ig = CiIgnore::new("(?i)copilot").unwrap();
    assert!(ig.is_ignored("Copilot code review"));
    assert!(!ig.is_ignored("build"));
    assert!(!CiIgnore::new("").unwrap().is_ignored("Copilot"));
}

#[tokio::test]
async fn reviews_is_the_list() {
    let c = FakeConnector::default();
    let list = json!([
        {"author": "a", "state": "approved", "body": "ok"},
        {"author": "b", "state": "commented", "body": ""}
    ]);
    c.set_reviews(item(7), json!({"sha": "h1", "reviews": list.clone()}));
    assert_eq!(reviews_fact(&c, Some(&item(7))).await.unwrap(), list);
    for r in list.as_array().unwrap() {
        for k in ["author", "state", "body"] {
            assert!(r.get(k).is_some());
        }
    }
}

#[tokio::test]
async fn reviews_empty_without_pr() {
    let c = FakeConnector::default();
    assert_eq!(reviews_fact(&c, None).await.unwrap(), json!([]));
    assert!(c.calls().is_empty());
    // A PR with no reviews gives the same.
    assert_eq!(reviews_fact(&c, Some(&item(7))).await.unwrap(), json!([]));
}

mod facts_mutation_tests {
    use super::*;

    #[tokio::test]
    async fn baseline_passes_every_check() {
        check_mergeable_unknown_then_known(true).await.unwrap();
        check_mergeable_unknown_then_known(false).await.unwrap();
    }

    #[tokio::test]
    async fn removing_unknown_first_read_fails_the_contract() {
        let _g = activate(Mutation {
            mergeable_known_on_first_read: true,
            ..Default::default()
        });
        assert!(check_mergeable_unknown_then_known(true).await.is_err());
    }
}
