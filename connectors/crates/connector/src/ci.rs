//! CI state, merge readiness and the aggregation rule behind it.

use serde::{Deserialize, Serialize};

/// Result of CI, either for one check/status (event `data.ci`) or aggregated
/// over a head commit (`MergeReadiness::ci`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CiState {
    Success,
    Failure,
    Pending,
}

/// Outcome of [`Connector::merge_change_request`](crate::Connector::merge_change_request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum MergeOutcome {
    /// This call merged the change request; `sha` is the squash commit.
    Merged { sha: String },
    /// The change request was already merged (by an earlier attempt or by
    /// someone else). `sha` is the merge commit when known. The merged head may
    /// differ from the one the caller expected.
    AlreadyMerged { sha: Option<String> },
    /// The head moved since `expected_head_sha`; nothing was merged.
    HeadMoved,
    /// Provider refused (conflicts, failing required checks, branch rules...).
    NotMergeable { reason: String },
}

/// Snapshot of whether a change request can be merged now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeReadiness {
    pub head_sha: String,
    /// `None` while the provider is still computing it. Never read as `false`.
    pub mergeable: Option<bool>,
    /// Provider's free-form state (GitHub: `clean`, `dirty`, `blocked`, ...).
    /// Empty when absent.
    pub mergeable_state: String,
    /// Aggregate over every check and status on the head; see [`aggregate_ci`].
    pub ci: CiState,
}

/// One check run, reduced to what aggregation needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSummary {
    /// `queued`, `in_progress`, `completed`, ...
    pub status: String,
    pub conclusion: Option<String>,
}

/// The combined commit status of a head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSummary {
    /// Number of statuses; the provider reports `pending` when this is zero.
    pub total_count: u64,
    /// `success`, `pending`, `failure` or `error`.
    pub state: String,
}

/// State of one check run.
pub fn check_state(c: &CheckSummary) -> CiState {
    if c.status != "completed" {
        return CiState::Pending;
    }
    match c.conclusion.as_deref() {
        Some("success" | "neutral" | "skipped") => CiState::Success,
        Some("failure" | "timed_out" | "cancelled" | "action_required" | "stale") => {
            CiState::Failure
        }
        // Completed without a (known) conclusion: not yet settled.
        _ => CiState::Pending,
    }
}

/// Aggregates every check and status on a head commit (not only required ones).
///
/// * Any failing check or status gives `failure`.
/// * Otherwise any unfinished check, or a `pending` combined status that has
///   statuses, gives `pending`.
/// * Otherwise `success`. A head with no checks and no statuses is `success`
///   (a repo without CI counts as passing).
///
/// Check conclusions `success`, `neutral`, `skipped` are success; `failure`,
/// `timed_out`, `cancelled`, `action_required`, `stale` are failure; a missing
/// conclusion or an unfinished run is pending.
pub fn aggregate_ci(checks: &[CheckSummary], statuses: &StatusSummary) -> CiState {
    let mut pending = false;
    for c in checks {
        match check_state(c) {
            CiState::Failure => return CiState::Failure,
            CiState::Pending => pending = true,
            CiState::Success => {}
        }
    }
    if statuses.total_count > 0 {
        match statuses.state.as_str() {
            "failure" | "error" => return CiState::Failure,
            "success" => {}
            _ => pending = true,
        }
    }
    if pending {
        CiState::Pending
    } else {
        CiState::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(status: &str, conclusion: Option<&str>) -> CheckSummary {
        CheckSummary {
            status: status.into(),
            conclusion: conclusion.map(Into::into),
        }
    }

    fn st(total_count: u64, state: &str) -> StatusSummary {
        StatusSummary {
            total_count,
            state: state.into(),
        }
    }

    #[test]
    fn conclusion_table() {
        for c in ["success", "neutral", "skipped"] {
            assert_eq!(check_state(&check("completed", Some(c))), CiState::Success);
        }
        for c in [
            "failure",
            "timed_out",
            "cancelled",
            "action_required",
            "stale",
        ] {
            assert_eq!(check_state(&check("completed", Some(c))), CiState::Failure);
        }
        assert_eq!(check_state(&check("completed", None)), CiState::Pending);
        assert_eq!(check_state(&check("queued", None)), CiState::Pending);
        assert_eq!(check_state(&check("in_progress", None)), CiState::Pending);
    }

    #[test]
    fn empty_head_is_success() {
        assert_eq!(aggregate_ci(&[], &st(0, "pending")), CiState::Success);
    }

    #[test]
    fn failure_beats_pending() {
        let checks = [
            check("in_progress", None),
            check("completed", Some("failure")),
        ];
        assert_eq!(aggregate_ci(&checks, &st(0, "pending")), CiState::Failure);
        assert_eq!(
            aggregate_ci(&[check("queued", None)], &st(1, "error")),
            CiState::Failure
        );
    }

    #[test]
    fn pending_sources() {
        assert_eq!(
            aggregate_ci(&[check("queued", None)], &st(0, "pending")),
            CiState::Pending
        );
        assert_eq!(aggregate_ci(&[], &st(2, "pending")), CiState::Pending);
    }

    #[test]
    fn all_green() {
        let checks = [
            check("completed", Some("success")),
            check("completed", Some("neutral")),
            check("completed", Some("skipped")),
        ];
        assert_eq!(aggregate_ci(&checks, &st(1, "success")), CiState::Success);
    }
}
