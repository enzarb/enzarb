use async_trait::async_trait;

use crate::{
    Actor, ChangeRequestSpec, CloseReason, CommentRef, CommitRef, CredScope, Error, ItemRef,
    MergeOutcome, MergeReadiness, Permission, Progress, ProgressTarget, ReadCred, RepoKey, Result,
    ShortLivedCred, Status, Thread, UpdateBranchOutcome, WorkItemKey, WorkflowRunInfo,
};

/// What [`Connector::upsert_issue`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Created,
    Updated,
    Unchanged,
}

impl UpsertOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
        }
    }
}

/// The issue [`Connector::upsert_issue`] created or found, and what it did to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertedIssue {
    pub item: WorkItemKey,
    pub outcome: UpsertOutcome,
}

/// Outbound operations against a provider.
///
/// Every call that changes provider state takes an `idempotency_key`: repeating
/// a call with the same key must not repeat its effect (for example a hidden
/// marker in a comment body checked before posting), and returns the original
/// result where there is one. Keys are chosen by the caller (the workflow job
/// id and attempt-independent step name).
///
/// Items are addressed by [`WorkItemKey`] (which carries the repo) rather than
/// the envelope's [`ItemRef`], which has no repo field.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn fetch_thread(&self, item: &WorkItemKey) -> Result<Thread>;

    async fn post_comment(
        &self,
        item: &WorkItemKey,
        body: &str,
        idempotency_key: &str,
    ) -> Result<CommentRef>;

    async fn open_change_request(
        &self,
        req: ChangeRequestSpec,
        idempotency_key: &str,
    ) -> Result<ItemRef>;

    async fn set_status(
        &self,
        target: &CommitRef,
        status: Status,
        idempotency_key: &str,
    ) -> Result<()>;

    /// Replaces any progress marker this connector previously set on `target`.
    async fn set_progress(
        &self,
        target: &ProgressTarget,
        progress: Progress,
        idempotency_key: &str,
    ) -> Result<()>;

    /// Squash-merges change request `number`, but only if its head is still
    /// `expected_head_sha`. Called only on a user's explicit action.
    ///
    /// A refused merge is a [`MergeOutcome`], not an `Err`. Idempotency comes
    /// from the change request's state rather than from `idempotency_key`
    /// (providers have no marker for it): a change request that is already
    /// merged yields `AlreadyMerged`, so repeating the call is safe.
    async fn merge_change_request(
        &self,
        item: &WorkItemKey,
        number: u64,
        expected_head_sha: &str,
        commit_title: &str,
        commit_body: Option<&str>,
        idempotency_key: &str,
    ) -> Result<MergeOutcome>;

    /// Current mergeability and aggregate CI of a change request's head.
    ///
    /// `ci` covers all checks and statuses on the head (see
    /// [`aggregate_ci`](crate::aggregate_ci)); consumers should call this to
    /// re-evaluate rather than fold `CiStatusChanged` events. `mergeable` is
    /// `None` while the provider is still computing it.
    async fn get_merge_readiness(&self, item: &WorkItemKey, number: u64) -> Result<MergeReadiness>;

    /// Merge the base branch into the change request's head branch (GitHub's "update
    /// branch"). Never rewrites history. A head that is not `expected_head_sha` or a
    /// merge conflict is [`UpdateBranchOutcome::Refused`], not an `Err`. The default
    /// implementation is unsupported.
    async fn update_branch(
        &self,
        _item: &WorkItemKey,
        _number: u64,
        _expected_head_sha: &str,
    ) -> Result<UpdateBranchOutcome> {
        Err(Error::Other("update_branch is not supported".into()))
    }

    /// Changes the base branch of change request `number` to `new_base` (stacked change
    /// request retargeting). Setting the base it already has succeeds. The default
    /// implementation is unsupported.
    async fn set_base(
        &self,
        _item: &WorkItemKey,
        _number: u64,
        _new_base: &str,
        _idempotency_key: &str,
    ) -> Result<()> {
        Err(Error::Other("set_base is not supported".into()))
    }

    /// Deletes `branch` of `repo`. A branch that is already gone succeeds, so repeating the
    /// call is safe. The default implementation is unsupported, so connectors roll out
    /// independently of the workflows that call it.
    async fn delete_branch(
        &self,
        _repo: &RepoKey,
        _branch: &str,
        _idempotency_key: &str,
    ) -> Result<()> {
        Err(Error::Other("delete_branch is not supported".into()))
    }

    /// Current head commit sha of `branch`, or `None` if the branch does not
    /// exist. The default implementation returns `Ok(None)`.
    async fn branch_head(&self, _repo: &RepoKey, _branch: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// How many commits `head` has that `base` does not (both a branch name or commit sha), or
    /// `None` when either is unknown to the provider. The default implementation returns
    /// `Ok(None)`, so connectors roll out independently of the workflows that call it.
    async fn ahead_by(&self, _repo: &RepoKey, _base: &str, _head: &str) -> Result<Option<u64>> {
        Ok(None)
    }

    /// The text of the file at `path` in `repo` at `git_ref` (a branch, tag or commit sha), or
    /// `None` when there is no such file (or it is a directory). Read with the connector's own
    /// credential; nothing enters a sandbox. The default implementation reports the capability
    /// as unsupported, so connectors roll out independently of the workflows that call it.
    async fn get_file(
        &self,
        _repo: &RepoKey,
        _git_ref: &str,
        _path: &str,
    ) -> Result<Option<String>> {
        Err(Error::Invalid("reading files is not supported".into()))
    }

    /// Every file (blob) of `repo` at `git_ref` as `(path, size in bytes)`, in one call, so a
    /// caller reading many files of one commit can fetch them concurrently. `Ok(None)` when the
    /// listing is unsupported (the default) or incomplete (e.g. truncated by the provider);
    /// callers then read files one by one with [`get_file`](Self::get_file).
    async fn list_files(
        &self,
        _repo: &RepoKey,
        _git_ref: &str,
    ) -> Result<Option<Vec<(String, u64)>>> {
        Ok(None)
    }

    /// The items `item` is blocked by, from the provider's own relationships (GitHub's
    /// native issue dependencies). Not the text ones: those come from
    /// [`dependencies::text_dependencies`](crate::dependencies::text_dependencies). The
    /// default implementation reports the capability as unsupported.
    async fn list_dependencies(&self, _item: &WorkItemKey) -> Result<Vec<WorkItemKey>> {
        Err(Error::Invalid("dependencies are not supported".into()))
    }

    /// Records that `item` is blocked by `depends_on`. Adding an existing relationship
    /// succeeds.
    async fn add_dependency(
        &self,
        _item: &WorkItemKey,
        _depends_on: &WorkItemKey,
        _idempotency_key: &str,
    ) -> Result<()> {
        Err(Error::Invalid("dependencies are not supported".into()))
    }

    /// Removes the relationship; removing one that does not exist succeeds.
    async fn remove_dependency(
        &self,
        _item: &WorkItemKey,
        _depends_on: &WorkItemKey,
        _idempotency_key: &str,
    ) -> Result<()> {
        Err(Error::Invalid("dependencies are not supported".into()))
    }

    /// Creates an issue in `repo` and returns its key. A call repeated with the same
    /// `idempotency_key` returns the issue the first call created instead of making another
    /// (GitHub: a hidden marker in the body, looked up before creating). Labels that do not
    /// exist are created by the provider or ignored. The default implementation reports the
    /// capability as unsupported.
    async fn create_issue(
        &self,
        _repo: &RepoKey,
        _title: &str,
        _body: &str,
        _labels: &[String],
        _idempotency_key: &str,
    ) -> Result<WorkItemKey> {
        Err(Error::Invalid("creating issues is not supported".into()))
    }

    /// Replaces the description of an issue made by `create_issue`, keeping whatever hidden
    /// idempotency marker the provider embedded there. Setting the same body again succeeds.
    /// The default implementation reports the capability as unsupported.
    async fn update_issue(&self, _item: &WorkItemKey, _body: &str) -> Result<()> {
        Err(Error::Invalid("updating issues is not supported".into()))
    }

    /// Files an issue under a stable `dedupe_key`. The first call creates it (with `labels`);
    /// later calls with the same key find that issue, whatever its state, and replace its
    /// title and description when they differ, never its state, labels or comments. A call
    /// that changes nothing makes no write. The default implementation reports the
    /// capability as unsupported.
    async fn upsert_issue(
        &self,
        _repo: &RepoKey,
        _title: &str,
        _body: &str,
        _labels: &[String],
        _dedupe_key: &str,
    ) -> Result<UpsertedIssue> {
        Err(Error::Invalid("filing issues is not supported".into()))
    }

    /// Closes an issue with `reason`. Closing an issue that is already closed succeeds
    /// without changing it, so a retried job is harmless. The default implementation
    /// reports the capability as unsupported.
    async fn close_issue(&self, _item: &WorkItemKey, _reason: CloseReason) -> Result<()> {
        Err(Error::Invalid("closing issues is not supported".into()))
    }

    /// Makes `child` a sub-issue of `parent`. Linking an existing sub-issue succeeds. A
    /// connector without native sub-issues may record the link in the parent's body instead.
    /// The default implementation reports the capability as unsupported.
    async fn link_sub_issue(&self, _parent: &WorkItemKey, _child: &WorkItemKey) -> Result<()> {
        Err(Error::Invalid("sub-issues are not supported".into()))
    }

    async fn check_permission(&self, repo: &RepoKey, actor: &Actor) -> Result<Permission>;

    /// A short-lived token restricted to the single `repo`.
    async fn mint_clone_credential(
        &self,
        repo: &RepoKey,
        scope: CredScope,
        idempotency_key: &str,
    ) -> Result<ShortLivedCred>;

    /// A short-lived, read-only token (`contents: read`) restricted to exactly
    /// `repos`, for fetching private git dependencies. `purpose` is a short
    /// label for audit/logging. Connectors without this capability return
    /// `Ok(None)` (the default). Implementations must reject an empty list and
    /// repos that are not reachable through a single installation.
    async fn mint_read_credential(
        &self,
        _repos: &[RepoKey],
        _purpose: &str,
    ) -> Result<Option<ReadCred>> {
        Ok(None)
    }

    /// A short-lived token restricted to the single `repo` with read-only access
    /// to CI data (Actions runs/logs and check runs, no contents), so an agent can
    /// read failure logs with `gh`. Connectors without this capability return
    /// `Ok(None)` (the default); so does a connector whose App was not granted the
    /// permission yet (the caller then runs without it).
    async fn mint_ci_read_credential(&self, _repo: &RepoKey) -> Result<Option<ReadCred>> {
        Ok(None)
    }

    // ---- ADR-0022 generic building blocks. Defaults report "unsupported" so
    // ---- connectors roll out independently of the engine.

    /// The commit sha `git_ref` (branch, tag or sha) points at, or `None` when unknown.
    async fn resolve_ref(&self, _repo: &RepoKey, _git_ref: &str) -> Result<Option<String>> {
        Err(Error::Invalid("resolve_ref is not supported".into()))
    }

    /// Open issues of `repo` (not pull requests), for the engine's adopt pass.
    async fn list_open_issues(&self, _repo: &RepoKey) -> Result<Vec<ItemRef>> {
        Err(Error::Invalid("listing issues is not supported".into()))
    }

    /// Issue numbers (same repo) a pull request references: closing keywords and
    /// the provider's development links.
    async fn pr_references(&self, _item: &WorkItemKey) -> Result<Vec<u64>> {
        Ok(Vec::new())
    }

    /// Adds a label (an existing label is a no-op).
    async fn add_label(&self, _item: &WorkItemKey, _label: &str) -> Result<()> {
        Err(Error::Invalid("labels are not supported".into()))
    }

    /// Removes a label (a missing label is a no-op).
    async fn remove_label(&self, _item: &WorkItemKey, _label: &str) -> Result<()> {
        Err(Error::Invalid("labels are not supported".into()))
    }

    /// Changes a pull request's title and/or body.
    async fn update_pull_request(
        &self,
        _item: &WorkItemKey,
        _title: Option<&str>,
        _body: Option<&str>,
    ) -> Result<()> {
        Err(Error::Invalid(
            "updating pull requests is not supported".into(),
        ))
    }

    /// CI of `sha` in the shape of the interpreter's `facts.ci`:
    /// `{state, failing: [{name, url?, log_excerpt?}]}`.
    async fn ci_details(&self, _repo: &RepoKey, _sha: &str) -> Result<serde_json::Value> {
        Err(Error::Invalid("ci details are not supported".into()))
    }

    /// Reviews of a pull request: `[{author, state, body}]`, latest per author.
    async fn list_reviews(&self, _item: &WorkItemKey) -> Result<serde_json::Value> {
        Err(Error::Invalid("reviews are not supported".into()))
    }

    /// Whether commit `sha` is reachable from `git_ref`; `None` when unknown.
    async fn contains(&self, _repo: &RepoKey, _sha: &str, _git_ref: &str) -> Result<Option<bool>> {
        Ok(None)
    }

    /// The latest *finished* run of CI workflow file `workflow` (`release.yml`) on `branch`;
    /// `None` when it has none. A run still in progress is skipped, so it never masks the
    /// outcome before it. The default implementation reports the capability as unsupported.
    async fn latest_workflow_run(
        &self,
        _repo: &RepoKey,
        _workflow: &str,
        _branch: &str,
    ) -> Result<Option<WorkflowRunInfo>> {
        Err(Error::Invalid("workflow runs are not supported".into()))
    }

    /// A short-lived token restricted to `repos` with exactly `permissions`
    /// (`{"contents": "write", "actions": "read"}`), for one sandbox step.
    async fn mint_step_token(
        &self,
        _repos: &[RepoKey],
        _permissions: &std::collections::BTreeMap<String, String>,
    ) -> Result<ReadCred> {
        Err(Error::Invalid("step tokens are not supported".into()))
    }
}
