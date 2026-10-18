//! Contract tests for `create_issues` (`docs/workflows/contracts-v2.md`, "Kernel actions";
//! `docs/contracts.md`; ADR-0021, ADR-0022; the doc comment on `actions::create_issues`).
//!
//! Each promise is a `check_*` helper over the fake's final state, wrapped by a
//! `#[tokio::test]`. `mutation_tests` runs the same helpers with one behaviour switched off
//! and requires them to fail.

use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{Connector, WorkItemKey};
use serde_json::{Map, Value, json};

use crate::actions::create_issues;
use crate::actions::mutation::{Mutation, activate};
use crate::engine::repo_key;

const JOB: &str = "t1:call-4";

fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

async fn run(c: &FakeConnector, a: &Map<String, Value>, key: &str) -> Result<Value, String> {
    match create_issues(c, "o/r", a, key).await {
        Ok(v) => Ok(v),
        Err(_) => Err("create_issues failed".to_string()),
    }
}

fn item(n: u64) -> WorkItemKey {
    WorkItemKey::new(repo_key("o/r"), n)
}

/// `b` is created first (`a` depends on it), then `a`, then `c`; `b` refers forward to `a`.
fn scenario() -> Map<String, Value> {
    args(json!({
        "parent": "#7",
        "footer": "Part of #7",
        "issues": [
            {"key": "a", "title": "A", "body": "needs {{issue:b}}", "depends_on": ["b", "zz", "a"]},
            {"key": "b", "title": "B", "body": "see {{issue:a}} and {{issue:nope}}"},
            {"key": "c", "title": "C", "body": "plain", "labels": ["x"]}
        ]
    }))
}

async fn run_scenario() -> (FakeConnector, Value) {
    let c = FakeConnector::default();
    c.set_next_issue_number(10);
    let out = run(&c, &scenario(), JOB).await.unwrap();
    (c, out)
}

fn ensure(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what()) }
}

/// "links it as a sub-issue of `parent`": every created issue is a sub-issue of it.
fn check_sub_issue_links(c: &FakeConnector) -> Result<(), String> {
    let mut got = c.sub_issues(&item(7));
    got.sort_by_key(|k| k.number);
    let want = vec![item(10), item(11), item(12)];
    ensure(got == want, || format!("sub-issues {got:?}, want {want:?}"))
}

/// "records its `depends_on` keys as blocked-by relations"; unknown keys and self-references
/// are skipped.
async fn check_dependencies(c: &FakeConnector) -> Result<(), String> {
    let a = c.list_dependencies(&item(11)).await.unwrap();
    ensure(a == vec![item(10)], || format!("a blocked by {a:?}"))?;
    for n in [10, 12] {
        let d = c.list_dependencies(&item(n)).await.unwrap();
        ensure(d.is_empty(), || format!("#{n} blocked by {d:?}"))?;
    }
    Ok(())
}

/// "replaces `{{issue:<key>}}` in its body with `#<number> (<title>)`, appends `footer`".
fn check_placeholders(c: &FakeConnector) -> Result<(), String> {
    let bodies: Vec<String> = c.created_issues().into_iter().map(|i| i.2).collect();
    let want = [
        "see #11 (A) and {{issue:nope}}\n\nPart of #7",
        "needs #10 (B)\n\nPart of #7",
        "plain\n\nPart of #7",
    ];
    ensure(bodies == want, || format!("bodies {bodies:?}"))
}

#[tokio::test]
async fn sub_issue_links_created() {
    let (c, _) = run_scenario().await;
    check_sub_issue_links(&c).unwrap();
}

#[tokio::test]
async fn sub_issue_parent_as_number() {
    let c = FakeConnector::default();
    let a = args(json!({"parent": 7, "issues": [{"key": "a", "title": "A", "body": ""}]}));
    run(&c, &a, JOB).await.unwrap();
    assert_eq!(c.sub_issues(&item(7)).len(), 1);
}

#[tokio::test]
async fn no_parent_creates_no_sub_issue_link() {
    let c = FakeConnector::default();
    let a = args(json!({"issues": [{"key": "a", "title": "A", "body": ""}]}));
    run(&c, &a, JOB).await.unwrap();
    assert!(
        !c.calls()
            .iter()
            .any(|r| format!("{:?}", r.call).contains("LinkSubIssue"))
    );
}

#[tokio::test]
async fn depends_on_recorded_as_blocked_by() {
    let (c, _) = run_scenario().await;
    check_dependencies(&c).await.unwrap();
}

#[tokio::test]
async fn dependencies_are_created_first() {
    let (c, out) = run_scenario().await;
    // Input order is a, b, c; `b` is created first and gets the lowest number.
    assert_eq!(out["issues"][0]["number"], 11);
    assert_eq!(out["issues"][1]["number"], 10);
    let titles: Vec<String> = c.created_issues().into_iter().map(|i| i.1).collect();
    assert_eq!(titles, ["B", "A", "C"]);
}

#[tokio::test]
async fn placeholders_resolve_to_number_and_title() {
    let (c, _) = run_scenario().await;
    check_placeholders(&c).unwrap();
}

#[tokio::test]
async fn labels_are_passed_to_the_issue() {
    let (c, _) = run_scenario().await;
    // The fake records labels on the call, not on the issue.
    let calls: Vec<String> = c.calls().iter().map(|r| format!("{:?}", r.call)).collect();
    let with = |t: &str| {
        calls
            .iter()
            .find(|c| c.contains("CreateIssue") && c.contains(t))
    };
    assert!(with("title: \"C\"").unwrap().contains("labels: [\"x\"]"));
    assert!(with("title: \"A\"").unwrap().contains("labels: []"));
}

#[tokio::test]
async fn result_shape_and_order() {
    let (_, out) = run_scenario().await;
    let want = json!({"issues": [
        {"kind": "issue", "repo": "o/r", "number": 11, "title": "A",
         "url": "https://github.com/o/r/issues/11", "key": "github:o/r#11"},
        {"kind": "issue", "repo": "o/r", "number": 10, "title": "B",
         "url": "https://github.com/o/r/issues/10", "key": "github:o/r#10"},
        {"kind": "issue", "repo": "o/r", "number": 12, "title": "C",
         "url": "https://github.com/o/r/issues/12", "key": "github:o/r#12"}
    ]});
    assert_eq!(out, want);
}

#[tokio::test]
async fn retry_creates_no_duplicates() {
    let (c, out) = run_scenario().await;
    let created = c.created_issues();
    let again = run(&c, &scenario(), JOB).await.unwrap();
    assert_eq!(again, out);
    assert_eq!(c.created_issues(), created);
    assert_eq!(c.sub_issues(&item(7)).len(), 3);
    check_sub_issue_links(&c).unwrap();
    check_dependencies(&c).await.unwrap();
    check_placeholders(&c).unwrap();
}

#[tokio::test]
async fn different_job_key_is_not_a_replay() {
    let (c, _) = run_scenario().await;
    run(&c, &scenario(), "t2:call-9").await.unwrap();
    assert_eq!(c.created_issues().len(), 6);
}

#[tokio::test]
async fn duplicate_edge_422_is_absorbed() {
    let (c, _) = run_scenario().await;
    assert!(c.rejected_duplicates().is_empty());
    // The retry re-sends the edge and the link; the fake answers 422 for both.
    run(&c, &scenario(), JOB).await.unwrap();
    let ops: Vec<&str> = c.rejected_duplicates().iter().map(|r| r.op).collect();
    assert!(ops.contains(&"add_dependency"), "{ops:?}");
    assert!(ops.contains(&"link_sub_issue"), "{ops:?}");
    assert!(c.rejected_duplicates().iter().all(|r| r.status == 422));
    assert_eq!(c.list_dependencies(&item(11)).await.unwrap(), [item(10)]);
}

#[tokio::test]
async fn transient_failure_then_retry_converges() {
    let a = args(json!({
        "parent": 7,
        "issues": [
            {"key": "x", "title": "X", "body": "first"},
            {"key": "y", "title": "Y", "body": "after {{issue:x}}", "depends_on": ["x"]}
        ]
    }));
    let clean = FakeConnector::default();
    run(&clean, &a, JOB).await.unwrap();

    let c = FakeConnector::default();
    // The first attempt got as far as the first issue, then the job died.
    let mut first = a.clone();
    first.insert("issues".into(), json!([a["issues"][0].clone()]));
    run(&c, &first, JOB).await.unwrap();
    c.fail_next(enzarb_connector::Error::Transient("boom".into()));
    assert!(run(&c, &a, JOB).await.is_err());
    run(&c, &a, JOB).await.unwrap();

    assert_eq!(c.created_issues(), clean.created_issues());
    assert_eq!(c.sub_issues(&item(7)), clean.sub_issues(&item(7)));
    let y = c.created_issues()[1].0.clone();
    assert_eq!(
        c.list_dependencies(&y).await.unwrap(),
        clean.list_dependencies(&y).await.unwrap()
    );
}

/// Removing a documented behaviour must fail the contract assertion that covers it.
mod mutation_tests {
    use super::*;

    #[tokio::test]
    async fn baseline_passes_every_check() {
        let (c, _) = run_scenario().await;
        check_sub_issue_links(&c).unwrap();
        check_dependencies(&c).await.unwrap();
        check_placeholders(&c).unwrap();
    }

    #[tokio::test]
    async fn removing_sub_issue_linking_fails_the_contract() {
        let _g = activate(Mutation {
            skip_sub_issue_link: true,
            ..Default::default()
        });
        let (c, _) = run_scenario().await;
        assert!(check_sub_issue_links(&c).is_err());
    }

    #[tokio::test]
    async fn removing_dependency_recording_fails_the_contract() {
        let _g = activate(Mutation {
            skip_dependencies: true,
            ..Default::default()
        });
        let (c, _) = run_scenario().await;
        assert!(check_dependencies(&c).await.is_err());
    }

    #[tokio::test]
    async fn removing_placeholder_resolution_fails_the_contract() {
        let _g = activate(Mutation {
            skip_placeholders: true,
            ..Default::default()
        });
        let (c, _) = run_scenario().await;
        assert!(check_placeholders(&c).is_err());
    }
}
