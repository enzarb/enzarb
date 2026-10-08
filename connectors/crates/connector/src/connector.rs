use async_trait::async_trait;

use crate::{
    Actor, ChangeRequestSpec, CommentRef, CommitRef, CredScope, ItemRef, Permission, Progress,
    ProgressTarget, RepoKey, Result, ShortLivedCred, Status, Thread, WorkItemKey,
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

    async fn check_permission(&self, repo: &RepoKey, actor: &Actor) -> Result<Permission>;

    /// A short-lived token restricted to the single `repo`.
    async fn mint_clone_credential(
        &self,
        repo: &RepoKey,
        scope: CredScope,
        idempotency_key: &str,
    ) -> Result<ShortLivedCred>;
}
