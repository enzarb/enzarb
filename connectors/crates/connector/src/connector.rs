use async_trait::async_trait;

use crate::{
    Actor, ChangeRequestSpec, CommentRef, CommitRef, CredScope, ItemRef, MergeOutcome,
    MergeReadiness, Permission, Progress, ProgressTarget, ReadCred, RepoKey, Result,
    ShortLivedCred, Status, Thread, WorkItemKey,
};

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

    /// Current head commit sha of `branch`, or `None` if the branch does not
    /// exist. The default implementation returns `Ok(None)`.
    async fn branch_head(&self, _repo: &RepoKey, _branch: &str) -> Result<Option<String>> {
        Ok(None)
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
}
