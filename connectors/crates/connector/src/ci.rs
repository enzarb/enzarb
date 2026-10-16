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

/// Outcome of [`Connector::update_branch`](crate::Connector::update_branch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum UpdateBranchOutcome {
    /// The base was merged into the head branch (a new head commit, no history rewrite).
    Updated,
    /// The head moved since `expected_head_sha`, or the provider refused (conflicts...).
    Refused { reason: String },
}

/// Snapshot of whether a change request can be merged now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeReadiness {
    pub head_sha: String,
    /// The change request's base commit. `None` when the provider does not
    /// report it (or the payload predates this field).
    #[serde(default)]
    pub base_sha: Option<String>,
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

/// Env var holding the [`CiIgnore`] pattern (helm `workflows.ciIgnoreChecks`).
pub const CI_IGNORE_CHECKS_ENV: &str = "ENZARB_CI_IGNORE_CHECKS";
/// Default [`CiIgnore`] pattern: Copilot code review is advisory, never CI.
pub const CI_IGNORE_CHECKS_DEFAULT: &str = "(?i)copilot";

/// Check names / status contexts that never count toward CI (advisory bots such as
/// Copilot code review): left out of [`aggregate_ci`] inputs and of per-check events.
#[derive(Debug, Clone)]
pub struct CiIgnore(Option<regex::Regex>);

impl Default for CiIgnore {
    fn default() -> Self {
        Self::new(CI_IGNORE_CHECKS_DEFAULT).expect("default pattern compiles")
    }
}

impl CiIgnore {
    /// An empty (or blank) pattern ignores nothing.
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        let p = pattern.trim();
        if p.is_empty() {
            return Ok(Self(None));
        }
        regex::Regex::new(p).map(|r| Self(Some(r)))
    }

    /// Nothing is ignored.
    pub fn none() -> Self {
        Self(None)
    }

    /// From [`CI_IGNORE_CHECKS_ENV`]: unset means [`CI_IGNORE_CHECKS_DEFAULT`], set but empty
    /// means nothing is ignored.
    pub fn from_env() -> Result<Self, regex::Error> {
        match std::env::var(CI_IGNORE_CHECKS_ENV) {
            Ok(v) => Self::new(&v),
            Err(_) => Ok(Self::default()),
        }
    }

    /// `name` (a check run name or a commit status context) never counts toward CI.
    pub fn is_ignored(&self, name: &str) -> bool {
        self.0.as_ref().is_some_and(|r| r.is_match(name))
    }
}

/// Combined state of individual commit statuses `(context, state)`, as GitHub combines
/// them, leaving out the ignored contexts. Used instead of the provider's combined state
/// when a context is ignored.
pub fn aggregate_statuses<'a>(
    statuses: impl IntoIterator<Item = (&'a str, &'a str)>,
    ignore: &CiIgnore,
) -> StatusSummary {
    let mut total = 0;
    let mut state = "success";
    for (ctx, st) in statuses {
        if ignore.is_ignored(ctx) {
            continue;
        }
        total += 1;
        match st {
            "failure" | "error" => state = "failure",
            "success" => {}
            _ if state != "failure" => state = "pending",
            _ => {}
        }
    }
    StatusSummary {
        total_count: total,
        state: if total == 0 { "pending" } else { state }.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_copilot_by_default() {
        let i = CiIgnore::default();
        assert!(i.is_ignored("Copilot code review"));
        assert!(i.is_ignored("Running Copilot Code Review"));
        assert!(!i.is_ignored("scripts"));
        assert!(!CiIgnore::new("").unwrap().is_ignored("Copilot"));
        assert!(CiIgnore::new("(").is_err());
    }

    #[test]
    fn ignored_status_does_not_fail_aggregate() {
        let i = CiIgnore::default();
        let s = aggregate_statuses([("Copilot", "error"), ("lint", "success")], &i);
        assert_eq!(s, st(1, "success"));
        assert_eq!(aggregate_ci(&[], &s), CiState::Success);
        let only = aggregate_statuses([("Copilot", "failure")], &i);
        assert_eq!(aggregate_ci(&[], &only), CiState::Success);
        let s = aggregate_statuses([("a", "pending"), ("b", "failure")], &i);
        assert_eq!(s.state, "failure");
    }

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
