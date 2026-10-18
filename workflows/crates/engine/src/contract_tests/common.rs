//! Shared setup for the contract tests that drive `actions::dispatch` against `FakeConnector`.

use enzarb_connector::fake::{Call, FakeConnector};
use enzarb_connector::{CiState, MergeReadiness, WorkItemKey};
use enzarb_flow::document::{Subject, TaskDocument, TaskInfo};
use enzarb_flow::interp::TaskState;
use serde_json::{Map, Value};

use crate::actions::{ActionError, dispatch};
use crate::engine::repo_key;

pub const JOB: &str = "t7:call-2";
pub const ISSUE: u64 = 5;
pub const PR: u64 = 8;

pub fn args(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

/// Task 7 on `o/r`, default branch `main`, with issue #5 and open pull request #8.
pub fn state() -> TaskState {
    let subject = |number| Subject {
        repo: "o/r".into(),
        number,
        state: "open".into(),
        ..Default::default()
    };
    let mut doc = TaskDocument {
        task: TaskInfo {
            id: "7".into(),
            repo: "o/r".into(),
            default_branch: "main".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    doc.subjects.issue = Some(subject(ISSUE));
    doc.subjects.pull_requests = vec![subject(PR)];
    TaskState::new(doc)
}

pub fn item(n: u64) -> WorkItemKey {
    WorkItemKey::new(repo_key("o/r"), n)
}

/// Runs `name`; errors read `refused: ..` or `transient: ..`.
pub async fn run(c: &FakeConnector, name: &str, a: Value, key: &str) -> Result<Value, String> {
    match dispatch(c, "o/r", &state(), name, &args(a), key).await {
        Ok(v) => Ok(v),
        Err(ActionError::Refused(m)) => Err(format!("refused: {m}")),
        Err(ActionError::Transient(m)) => Err(format!("transient: {m}")),
    }
}

/// Runs `name` through the executor's outcome mapping: `(ok, result)` as the workflow sees it.
pub async fn run_outcome(
    c: &FakeConnector,
    name: &str,
    a: Value,
    key: &str,
) -> Result<(bool, Value), String> {
    crate::actions::finish(dispatch(c, "o/r", &state(), name, &args(a), key).await)
}

pub fn readiness(head: &str) -> MergeReadiness {
    MergeReadiness {
        head_sha: head.into(),
        base_sha: None,
        mergeable: Some(true),
        mergeable_state: "clean".into(),
        ci: CiState::Pending,
    }
}

pub fn with_readiness(c: &FakeConnector, head: &str) {
    c.set_merge_readiness_immediate(item(PR), readiness(head));
}

pub fn ensure(ok: bool, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok { Ok(()) } else { Err(what()) }
}

/// Calls that had an effect, in order.
pub fn applied(c: &FakeConnector) -> Vec<Call> {
    c.applied_calls()
}
