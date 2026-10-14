//! Stacked change requests (docs/adr/0017-work-item-dependencies.md, "Stacking").
//!
//! A work item whose single open dependency has an open change request written by enzarb is
//! implemented on top of that change request's branch and opens its own change request with
//! the parent's branch as base. When the parent is squash-merged, a `retarget_change_request`
//! job rebases enzarb's own child branch onto the default branch and sets the base.
//!
//! This module is the pure part: which parent to stack on, the marker that carries the
//! choice from the issue to the change request it opens, and the result of the retarget job.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Branches enzarb pushes start with this (`enzarb/issue-12`). Only such a branch is ever
/// stacked on or force-pushed.
pub const ENZARB_BRANCH_PREFIX: &str = "enzarb/";

/// Whether `head_ref` is a branch enzarb itself pushes.
pub fn is_enzarb_branch(head_ref: &str) -> bool {
    head_ref
        .strip_prefix(ENZARB_BRANCH_PREFIX)
        .is_some_and(|rest| !rest.is_empty())
}

/// The change request a work item is stacked on (or could be).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StackParent {
    /// Work item key of the parent change request.
    pub key: String,
    pub head_ref: String,
    pub head_sha: String,
    /// The parent change request was merged (only meaningful for an item already stacked).
    pub merged: bool,
    /// The parent change request was closed or cancelled without merging.
    pub closed: bool,
}

/// One dependency of an item with what the selection needs to know about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParentRow {
    /// `work_items.state` of the dependency; `None` when it has no work item yet.
    pub dep_state: Option<String>,
    /// The change request standing for the dependency: the dependency itself when it is one,
    /// else the one opened for the issue.
    pub cr_key: Option<String>,
    pub cr_state: Option<String>,
    pub head_ref: Option<String>,
    pub head_sha: Option<String>,
}

const CR_FINAL: &[&str] = &["Merged", "Closed", "Cancelled", "Failed"];

impl ParentRow {
    fn completed(&self) -> bool {
        match (&self.cr_state, &self.dep_state) {
            (Some(s), _) => s == "Merged",
            (None, Some(s)) => s == "Done" || s == "Merged",
            (None, None) => false,
        }
    }

    fn open_cr(&self) -> bool {
        self.cr_state
            .as_deref()
            .is_some_and(|s| !CR_FINAL.contains(&s))
    }
}

/// The parent to stack on: only when exactly one dependency is not completed, it has an open
/// change request, and that request's branch is enzarb's own with a known head. Several open
/// parents, a parent without a change request, and a closed or failed parent stack nothing;
/// the `dependencies` merge gate sequences those.
pub fn select_stack_parent(rows: &[ParentRow]) -> Option<StackParent> {
    let mut open = rows.iter().filter(|r| !r.completed());
    let r = open.next()?;
    if open.next().is_some() || !r.open_cr() {
        return None;
    }
    let (key, head_ref, head_sha) = (r.cr_key.clone()?, r.head_ref.clone()?, r.head_sha.clone()?);
    if !is_enzarb_branch(&head_ref) || head_sha.is_empty() {
        return None;
    }
    Some(StackParent {
        key,
        head_ref,
        head_sha,
        merged: false,
        closed: false,
    })
}

/// Hidden marker appended to the body of a stacked change request. It carries the parent
/// and the commit the branch was cut from from the issue to the change request it opens.
const MARKER: &str = "enzarb:stack";

pub fn stack_marker(p: &StackParent) -> String {
    format!(
        "<!-- {MARKER} parent={} ref={} tip={} -->",
        p.key, p.head_ref, p.head_sha
    )
}

/// The stack recorded in `body` by [`stack_marker`]: parent key, parent branch, tip.
pub fn parse_stack_marker(body: &str) -> Option<(String, String, String)> {
    let start = body.find(&format!("<!-- {MARKER} "))? + 5 + MARKER.len() + 1;
    let rest = &body[start..];
    let inner = rest[..rest.find("-->")?].trim();
    let get = |k: &str| {
        inner
            .split_whitespace()
            .find_map(|p| p.strip_prefix(k))
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    Some((get("parent=")?, get("ref=")?, get("tip=")?))
}

/// What the `retarget_change_request` job reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetargetOutcome {
    /// Rebased, pushed with a lease and the base set. `head_sha` is the new head.
    Retargeted { head_sha: String },
    /// Rebased and pushed, but the provider refused the base change.
    BaseNotSet { head_sha: String, reason: String },
    /// The branch already carries no parent commits; nothing to do.
    Already,
    /// The rebase conflicted; nothing was pushed.
    Conflict { reason: String },
    /// The remote head is not the one the lease expected (the branch moved).
    LeaseFailed { reason: String },
    /// Not safe to force-push (foreign commits, parent tip not in history, rejected ref,
    /// or a sandbox that does not support the operation).
    Refused { reason: String },
}

impl RetargetOutcome {
    /// Reads a job output: `{outcome, head_sha?, reason?, base_set?}`, or the run's own
    /// output whose `summary` is that object.
    pub fn from_output(output: &Value) -> Self {
        let inner;
        let v = if output
            .get("outcome")
            .and_then(Value::as_str)
            .is_some_and(|o| !matches!(o, "succeeded" | "failed" | "cancelled"))
        {
            output
        } else if let Some(s) = output
            .get("summary")
            .and_then(Value::as_str)
            .and_then(|s| serde_json::from_str::<Value>(s.trim()).ok())
        {
            inner = s;
            &inner
        } else {
            output
        };
        let text = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let reason = text("reason");
        match v.get("outcome").and_then(Value::as_str).unwrap_or_default() {
            "retargeted" if v.get("base_set").and_then(Value::as_bool) == Some(false) => {
                Self::BaseNotSet {
                    head_sha: text("head_sha"),
                    reason,
                }
            }
            "retargeted" => Self::Retargeted {
                head_sha: text("head_sha"),
            },
            "already_retargeted" => Self::Already,
            "conflict" => Self::Conflict { reason },
            "lease_failed" => Self::LeaseFailed { reason },
            "not_ancestor" | "foreign_commits" | "rejected" => Self::Refused { reason },
            other => Self::Refused {
                reason: format!(
                    "unexpected retarget result {other:?}: the sandbox may not support it"
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(cr_state: &str) -> ParentRow {
        ParentRow {
            dep_state: Some("Done".into()),
            cr_key: Some("github:o/r#5".into()),
            cr_state: Some(cr_state.into()),
            head_ref: Some("enzarb/issue-4".into()),
            head_sha: Some("a".repeat(40)),
        }
    }

    #[test]
    fn marker_round_trips() {
        let p = StackParent {
            key: "github:o/r#5".into(),
            head_ref: "enzarb/issue-4".into(),
            head_sha: "abc123".into(),
            ..Default::default()
        };
        let body = format!("Closes #7\n\n{}", stack_marker(&p));
        assert_eq!(
            parse_stack_marker(&body),
            Some((
                "github:o/r#5".into(),
                "enzarb/issue-4".into(),
                "abc123".into()
            ))
        );
        assert_eq!(parse_stack_marker("no marker"), None);
        assert_eq!(parse_stack_marker("<!-- enzarb:stack parent=x -->"), None);
    }

    #[test]
    fn selects_one_open_enzarb_parent() {
        let p = select_stack_parent(&[row("AwaitingChanges")]).unwrap();
        assert_eq!(p.head_ref, "enzarb/issue-4");
        assert_eq!(p.key, "github:o/r#5");
    }

    #[test]
    fn does_not_stack_otherwise() {
        // merged parent
        assert!(select_stack_parent(&[row("Merged")]).is_none());
        // closed / failed parent
        assert!(select_stack_parent(&[row("Closed")]).is_none());
        assert!(select_stack_parent(&[row("Failed")]).is_none());
        // no change request yet
        let mut none = row("x");
        none.cr_state = None;
        none.cr_key = None;
        none.dep_state = Some("Implementing".into());
        assert!(select_stack_parent(&[none]).is_none());
        // two open parents
        assert!(select_stack_parent(&[row("Reviewing"), row("AwaitingChanges")]).is_none());
        // someone else's branch
        let mut foreign = row("Reviewing");
        foreign.head_ref = Some("feature/x".into());
        assert!(select_stack_parent(&[foreign]).is_none());
        // no rows
        assert!(select_stack_parent(&[]).is_none());
    }

    #[test]
    fn a_merged_sibling_does_not_prevent_stacking() {
        let p = select_stack_parent(&[row("Merged"), row("Reviewing")]);
        assert!(p.is_some());
    }

    #[test]
    fn enzarb_branch_rule() {
        assert!(is_enzarb_branch("enzarb/issue-3"));
        assert!(!is_enzarb_branch("enzarb/"));
        assert!(!is_enzarb_branch("main"));
        assert!(!is_enzarb_branch("feature/enzarb/x"));
    }

    #[test]
    fn outcomes_parse() {
        let o = |v: Value| RetargetOutcome::from_output(&v);
        assert_eq!(
            o(json!({"outcome": "retargeted", "head_sha": "n"})),
            RetargetOutcome::Retargeted {
                head_sha: "n".into()
            }
        );
        assert!(matches!(
            o(json!({"outcome": "retargeted", "head_sha": "n", "base_set": false, "reason": "r"})),
            RetargetOutcome::BaseNotSet { .. }
        ));
        assert_eq!(
            o(json!({"outcome": "already_retargeted"})),
            RetargetOutcome::Already
        );
        assert!(matches!(
            o(json!({"outcome": "conflict"})),
            RetargetOutcome::Conflict { .. }
        ));
        assert!(matches!(
            o(json!({"outcome": "lease_failed"})),
            RetargetOutcome::LeaseFailed { .. }
        ));
        for k in ["not_ancestor", "foreign_commits", "rejected"] {
            assert!(matches!(
                o(json!({ "outcome": k })),
                RetargetOutcome::Refused { .. }
            ));
        }
        // the run's own output carrying the object in `summary`
        assert_eq!(
            o(json!({"outcome": "succeeded",
                     "summary": "{\"outcome\":\"retargeted\",\"head_sha\":\"z\"}"})),
            RetargetOutcome::Retargeted {
                head_sha: "z".into()
            }
        );
        // an old sandbox ran an agent instead: unusable text
        assert!(matches!(
            o(json!({"outcome": "succeeded", "summary": "hello"})),
            RetargetOutcome::Refused { .. }
        ));
    }
}
