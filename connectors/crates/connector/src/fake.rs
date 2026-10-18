//! In-memory [`Connector`] that records calls, for tests (feature `fake`).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{Duration, Utc};

use crate::{
    Actor, ChangeRequestSpec, CloseReason, CommentRef, CommitRef, Connector, CredScope, Error,
    ItemKind, ItemRef, MergeOutcome, MergeReadiness, Permission, Progress, ProgressTarget,
    ReadCred, RepoKey, Result, ShortLivedCred, Status, Thread, ThreadComment, UpdateBranchOutcome,
    UpsertOutcome, UpsertedIssue, WorkItemKey, WorkflowRunInfo,
};

/// One call made against the fake, with its arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    FetchThread {
        item: WorkItemKey,
    },
    PostComment {
        item: WorkItemKey,
        body: String,
        idempotency_key: String,
    },
    OpenChangeRequest {
        req: ChangeRequestSpec,
        idempotency_key: String,
    },
    SetStatus {
        target: CommitRef,
        status: Status,
        idempotency_key: String,
    },
    SetProgress {
        target: ProgressTarget,
        progress: Progress,
        idempotency_key: String,
    },
    CheckPermission {
        repo: RepoKey,
        actor: Actor,
    },
    MintCloneCredential {
        repo: RepoKey,
        scope: CredScope,
        idempotency_key: String,
    },
    MintReadCredential {
        repos: Vec<RepoKey>,
        purpose: String,
    },
    MintCiReadCredential {
        repo: RepoKey,
    },
    UpdateBranch {
        item: WorkItemKey,
        number: u64,
        expected_head_sha: String,
    },
    MergeChangeRequest {
        item: WorkItemKey,
        number: u64,
        expected_head_sha: String,
        commit_title: String,
        commit_body: Option<String>,
        idempotency_key: String,
    },
    GetMergeReadiness {
        item: WorkItemKey,
        number: u64,
    },
    SetBase {
        item: WorkItemKey,
        number: u64,
        new_base: String,
        idempotency_key: String,
    },
    BranchHead {
        repo: RepoKey,
        branch: String,
    },
    AheadBy {
        repo: RepoKey,
        base: String,
        head: String,
    },
    GetFile {
        repo: RepoKey,
        git_ref: String,
        path: String,
    },
    DeleteBranch {
        repo: RepoKey,
        branch: String,
        idempotency_key: String,
    },
    ListDependencies {
        item: WorkItemKey,
    },
    AddDependency {
        item: WorkItemKey,
        depends_on: WorkItemKey,
        idempotency_key: String,
    },
    RemoveDependency {
        item: WorkItemKey,
        depends_on: WorkItemKey,
        idempotency_key: String,
    },
    CreateIssue {
        repo: RepoKey,
        title: String,
        body: String,
        labels: Vec<String>,
        idempotency_key: String,
    },
    UpdateIssue {
        item: WorkItemKey,
        body: String,
    },
    UpsertIssue {
        repo: RepoKey,
        title: String,
        body: String,
        labels: Vec<String>,
        dedupe_key: String,
    },
    LinkSubIssue {
        parent: WorkItemKey,
        child: WorkItemKey,
    },
    CloseIssue {
        item: WorkItemKey,
        reason: CloseReason,
    },
    /// An ADR-0022 generic call (`add_label`, `update_pull_request`, `mint_step_token`, ...).
    Generic {
        name: String,
        args: serde_json::Value,
    },
}

/// A recorded call. `replayed` is true when the idempotency key had been seen
/// before, so the call had no new effect.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCall {
    pub call: Call,
    pub replayed: bool,
}

/// A request the provider rejected, which the connector then absorbed (the
/// real connector turns a 422 on an existing edge into `Ok(())`).
#[derive(Debug, Clone, PartialEq)]
pub struct Rejection {
    /// The operation, e.g. `link_sub_issue` or `add_dependency`.
    pub op: &'static str,
    pub status: u16,
    pub message: String,
}

#[derive(Default)]
struct State {
    mergeable_reads: HashMap<WorkItemKey, u32>,
    rejections: Vec<Rejection>,
    calls: Vec<RecordedCall>,
    threads: HashMap<WorkItemKey, Thread>,
    permissions: HashMap<(RepoKey, String), Permission>,
    default_permission: Option<Permission>,
    comments: HashMap<WorkItemKey, Vec<ThreadComment>>,
    progress: HashMap<ProgressTarget, Progress>,
    statuses: Vec<(CommitRef, Status)>,
    change_requests: Vec<ItemRef>,
    seen_comments: HashMap<String, CommentRef>,
    seen_change_requests: HashMap<String, ItemRef>,
    seen_keys: std::collections::HashSet<String>,
    seen_creds: HashMap<String, ShortLivedCred>,
    next_id: u64,
    fail_next: Option<Error>,
    fail_delete_branch: Option<Error>,
    fail_set_base: Option<Error>,
    fail_step_token: Option<Error>,
    merge_outcomes: VecDeque<MergeOutcome>,
    seen_merges: HashMap<String, MergeOutcome>,
    readiness: HashMap<WorkItemKey, MergeReadiness>,
    update_outcomes: VecDeque<UpdateBranchOutcome>,
    branch_heads: HashMap<(RepoKey, String), String>,
    ahead: HashMap<(RepoKey, String, String), u64>,
    files: HashMap<(RepoKey, String, String), String>,
    dependencies: HashMap<WorkItemKey, Vec<WorkItemKey>>,
    seen_issues: HashMap<String, WorkItemKey>,
    created_issues: Vec<(WorkItemKey, String, String)>,
    sub_issues: HashMap<WorkItemKey, Vec<WorkItemKey>>,
    closed_issues: HashMap<WorkItemKey, CloseReason>,
    next_issue: u64,
    refs: HashMap<(RepoKey, String), String>,
    open_issues: HashMap<RepoKey, Vec<ItemRef>>,
    pr_refs: HashMap<WorkItemKey, Vec<u64>>,
    labels: HashMap<WorkItemKey, Vec<String>>,
    ci: HashMap<(RepoKey, String), serde_json::Value>,
    reviews: HashMap<WorkItemKey, serde_json::Value>,
    contains: HashMap<(RepoKey, String, String), bool>,
    workflow_runs: HashMap<(RepoKey, String, String), WorkflowRunInfo>,
    default_branches: HashMap<RepoKey, String>,
}

/// Records every call and keeps simple in-memory state. Cheap to share by
/// reference; all methods take `&self`.
#[derive(Default)]
pub struct FakeConnector {
    state: Mutex<State>,
}

impl FakeConnector {
    pub fn new() -> Self {
        Self::default()
    }

    fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Seed `resolve_ref` (falls back to `branch_heads`).
    pub fn set_ref(&self, repo: RepoKey, git_ref: &str, sha: &str) {
        self.st()
            .refs
            .insert((repo, git_ref.to_string()), sha.to_string());
    }

    /// Seed `list_open_issues`.
    pub fn set_open_issues(&self, repo: RepoKey, issues: Vec<ItemRef>) {
        self.st().open_issues.insert(repo, issues);
    }

    /// Seed `pr_references`.
    pub fn set_pr_references(&self, pr: WorkItemKey, issues: Vec<u64>) {
        self.st().pr_refs.insert(pr, issues);
    }

    /// Seed `ci_details`.
    pub fn set_ci(&self, repo: RepoKey, sha: &str, ci: serde_json::Value) {
        self.st().ci.insert((repo, sha.to_string()), ci);
    }

    /// Seed `list_reviews`.
    pub fn set_reviews(&self, pr: WorkItemKey, reviews: serde_json::Value) {
        self.st().reviews.insert(pr, reviews);
    }

    /// Seed `contains`.
    pub fn set_contains(&self, repo: RepoKey, sha: &str, git_ref: &str, yes: bool) {
        self.st()
            .contains
            .insert((repo, sha.to_string(), git_ref.to_string()), yes);
    }

    /// Seed `latest_workflow_run` for `workflow` on `branch`.
    pub fn set_workflow_run(
        &self,
        repo: RepoKey,
        workflow: &str,
        branch: &str,
        run: WorkflowRunInfo,
    ) {
        self.st()
            .workflow_runs
            .insert((repo, workflow.to_string(), branch.to_string()), run);
    }

    /// The default branch `latest_workflow_run(.., None)` resolves to (`main` when unset).
    pub fn set_default_branch(&self, repo: RepoKey, branch: &str) {
        self.st().default_branches.insert(repo, branch.to_string());
    }

    /// Current labels set through `add_label`/`remove_label`.
    pub fn labels(&self, item: &WorkItemKey) -> Vec<String> {
        self.st().labels.get(item).cloned().unwrap_or_default()
    }

    /// Generic calls named `name`, in order.
    pub fn generic_calls(&self, name: &str) -> Vec<serde_json::Value> {
        self.st()
            .calls
            .iter()
            .filter_map(|c| match &c.call {
                Call::Generic { name: n, args } if n == name => Some(args.clone()),
                _ => None,
            })
            .collect()
    }

    fn generic(&self, name: &str, args: serde_json::Value) -> Result<MutexGuard<'_, State>> {
        let mut st = self.st();
        let call = Call::Generic {
            name: name.to_string(),
            args,
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st)
    }

    /// Seed a thread returned by `fetch_thread`.
    pub fn add_thread(&self, key: WorkItemKey, thread: Thread) {
        self.st().threads.insert(key, thread);
    }

    /// Seed one actor's permission on a repo.
    pub fn set_permission(&self, repo: RepoKey, login: &str, permission: Permission) {
        self.st()
            .permissions
            .insert((repo, login.to_string()), permission);
    }

    /// Permission returned for actors without a specific entry (default: none).
    pub fn set_default_permission(&self, permission: Permission) {
        self.st().default_permission = Some(permission);
    }

    /// Queue the outcome of the next `merge_change_request` that has an unseen
    /// idempotency key. With the queue empty, merges succeed with a fake sha.
    pub fn queue_merge_outcome(&self, outcome: MergeOutcome) {
        self.st().merge_outcomes.push_back(outcome);
    }

    /// Queue the outcome of the next `update_branch`; with the queue empty it succeeds.
    pub fn queue_update_outcome(&self, outcome: UpdateBranchOutcome) {
        self.st().update_outcomes.push_back(outcome);
    }

    /// Seed the readiness returned for `item` (repeatable; last one wins).
    ///
    /// Like GitHub, which computes mergeability lazily, the first read after
    /// seeding a known `mergeable` reports `mergeable: None` and
    /// `mergeable_state: "unknown"`; later reads return the seeded value. A
    /// seeded `mergeable: None` stays unknown on every read.
    pub fn set_merge_readiness(&self, item: WorkItemKey, readiness: MergeReadiness) {
        let mut st = self.st();
        st.mergeable_reads.remove(&item);
        st.readiness.insert(item, readiness);
    }

    /// Seed the readiness with mergeability already computed: the first read
    /// returns it as is.
    pub fn set_merge_readiness_immediate(&self, item: WorkItemKey, readiness: MergeReadiness) {
        let mut st = self.st();
        st.mergeable_reads.insert(item.clone(), 1);
        st.readiness.insert(item, readiness);
    }

    /// Requests the provider answered with 422 (duplicate edges), in order.
    pub fn rejected_duplicates(&self) -> Vec<Rejection> {
        self.st().rejections.clone()
    }

    /// The dependencies (blocked-by edges) of `item`, in order.
    pub fn dependencies(&self, item: &WorkItemKey) -> Vec<WorkItemKey> {
        self.st()
            .dependencies
            .get(item)
            .cloned()
            .unwrap_or_default()
    }

    /// Seed the head sha returned by `branch_head` (unseeded branches are `None`).
    pub fn set_branch_head(&self, repo: RepoKey, branch: &str, sha: &str) {
        self.st()
            .branch_heads
            .insert((repo, branch.to_string()), sha.to_string());
    }

    /// Seed what `ahead_by(repo, base, head)` returns (unseeded pairs are `None`).
    pub fn set_ahead_by(&self, repo: RepoKey, base: &str, head: &str, n: u64) {
        self.st()
            .ahead
            .insert((repo, base.to_string(), head.to_string()), n);
    }

    /// Seed the text `get_file` returns for `path` at `git_ref` (unseeded files are `None`).
    pub fn set_file(&self, repo: RepoKey, git_ref: &str, path: &str, text: &str) {
        self.st().files.insert(
            (repo, git_ref.to_string(), path.to_string()),
            text.to_string(),
        );
    }

    /// Seed the relationships `list_dependencies` returns for `item`.
    pub fn set_dependencies(&self, item: WorkItemKey, depends_on: Vec<WorkItemKey>) {
        self.st().dependencies.insert(item, depends_on);
    }

    /// The first number `create_issue` hands out (default 1000).
    pub fn set_next_issue_number(&self, n: u64) {
        self.st().next_issue = n;
    }

    /// Issues made by `create_issue` as `(key, title, body)`, in order.
    pub fn created_issues(&self) -> Vec<(WorkItemKey, String, String)> {
        self.st().created_issues.clone()
    }

    /// How `item` was closed by `close_issue`, if it was.
    pub fn closed_issue(&self, item: &WorkItemKey) -> Option<CloseReason> {
        self.st().closed_issues.get(item).copied()
    }

    /// The sub-issues linked to `parent`, in order.
    pub fn sub_issues(&self, parent: &WorkItemKey) -> Vec<WorkItemKey> {
        self.st()
            .sub_issues
            .get(parent)
            .cloned()
            .unwrap_or_default()
    }

    /// Make the next call (any method) fail with `error`, once.
    pub fn fail_next(&self, error: Error) {
        self.st().fail_next = Some(error);
    }

    /// Make the next `delete_branch` call fail with `error`, once; other calls are unaffected.
    pub fn fail_delete_branch(&self, error: Error) {
        self.st().fail_delete_branch = Some(error);
    }

    /// Make the next `set_base` call fail with `error`, once; other calls are unaffected.
    pub fn fail_set_base(&self, error: Error) {
        self.st().fail_set_base = Some(error);
    }

    /// Make the next `mint_step_token` call fail with `error`, once; other calls are unaffected.
    pub fn fail_step_token(&self, error: Error) {
        self.st().fail_step_token = Some(error);
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.st().calls.clone()
    }

    /// Calls that had an effect (not idempotent replays).
    pub fn applied_calls(&self) -> Vec<Call> {
        self.st()
            .calls
            .iter()
            .filter(|c| !c.replayed)
            .map(|c| c.call.clone())
            .collect()
    }

    pub fn comments(&self, item: &WorkItemKey) -> Vec<ThreadComment> {
        self.st().comments.get(item).cloned().unwrap_or_default()
    }

    pub fn progress(&self, target: &ProgressTarget) -> Option<Progress> {
        self.st().progress.get(target).copied()
    }

    pub fn statuses(&self) -> Vec<(CommitRef, Status)> {
        self.st().statuses.clone()
    }

    pub fn change_requests(&self) -> Vec<ItemRef> {
        self.st().change_requests.clone()
    }

    /// Every `delete_branch` call so far as `(repo, branch)`, in order (replays included).
    pub fn delete_branch_calls(&self) -> Vec<(RepoKey, String)> {
        self.st()
            .calls
            .iter()
            .filter_map(|c| match &c.call {
                Call::DeleteBranch { repo, branch, .. } => Some((repo.clone(), branch.clone())),
                _ => None,
            })
            .collect()
    }

    /// Every `set_base` call so far as `(item, number, new_base)`, in order (replays included).
    pub fn set_base_calls(&self) -> Vec<(WorkItemKey, u64, String)> {
        self.st()
            .calls
            .iter()
            .filter_map(|c| match &c.call {
                Call::SetBase {
                    item,
                    number,
                    new_base,
                    ..
                } => Some((item.clone(), *number, new_base.clone())),
                _ => None,
            })
            .collect()
    }

    fn record(st: &mut State, call: Call, replayed: bool) {
        st.calls.push(RecordedCall { call, replayed });
    }

    fn take_failure(st: &mut State, call: Call) -> Result<()> {
        if let Some(e) = st.fail_next.take() {
            Self::record(st, call, false);
            return Err(e);
        }
        Ok(())
    }
}

fn bot() -> Actor {
    Actor {
        login: "enzarb[bot]".into(),
        id: 0,
        is_bot: true,
    }
}

#[async_trait]
impl Connector for FakeConnector {
    async fn fetch_thread(&self, item: &WorkItemKey) -> Result<Thread> {
        let mut st = self.st();
        let call = Call::FetchThread { item: item.clone() };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        let mut thread = st
            .threads
            .get(item)
            .cloned()
            .ok_or_else(|| Error::NotFound(item.to_string()))?;
        thread
            .comments
            .extend(st.comments.get(item).cloned().unwrap_or_default());
        Ok(thread)
    }

    async fn post_comment(
        &self,
        item: &WorkItemKey,
        body: &str,
        idempotency_key: &str,
    ) -> Result<CommentRef> {
        let mut st = self.st();
        let call = Call::PostComment {
            item: item.clone(),
            body: body.to_string(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(prev) = st.seen_comments.get(idempotency_key).cloned() {
            Self::record(&mut st, call, true);
            return Ok(prev);
        }
        st.next_id += 1;
        let c = CommentRef {
            id: st.next_id,
            kind: Default::default(),
            body: body.to_string(),
            url: format!("https://fake.invalid/{item}#comment-{}", st.next_id),
        };
        st.seen_comments
            .insert(idempotency_key.to_string(), c.clone());
        st.comments
            .entry(item.clone())
            .or_default()
            .push(ThreadComment {
                author: bot(),
                comment: c.clone(),
                created_at: Some(Utc::now()),
            });
        Self::record(&mut st, call, false);
        Ok(c)
    }

    async fn open_change_request(
        &self,
        req: ChangeRequestSpec,
        idempotency_key: &str,
    ) -> Result<ItemRef> {
        let mut st = self.st();
        let call = Call::OpenChangeRequest {
            req: req.clone(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(prev) = st.seen_change_requests.get(idempotency_key).cloned() {
            Self::record(&mut st, call, true);
            return Ok(prev);
        }
        st.next_id += 1;
        let number = 1000 + st.next_id;
        let item = ItemRef {
            kind: ItemKind::ChangeRequest,
            number,
            url: format!("https://fake.invalid/{}/pull/{number}", req.repo),
            title: req.title.clone(),
            body: req.body.clone(),
            author: bot().login,
            head_ref: Some(req.head_ref.clone()),
            base_ref: Some(req.base_ref.clone()),
            head_sha: None,
        };
        st.seen_change_requests
            .insert(idempotency_key.to_string(), item.clone());
        st.change_requests.push(item.clone());
        Self::record(&mut st, call, false);
        Ok(item)
    }

    async fn set_status(
        &self,
        target: &CommitRef,
        status: Status,
        idempotency_key: &str,
    ) -> Result<()> {
        let mut st = self.st();
        let call = Call::SetStatus {
            target: target.clone(),
            status: status.clone(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        let replayed = !st.seen_keys.insert(format!("status:{idempotency_key}"));
        if !replayed {
            st.statuses.push((target.clone(), status));
        }
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn set_progress(
        &self,
        target: &ProgressTarget,
        progress: Progress,
        idempotency_key: &str,
    ) -> Result<()> {
        let mut st = self.st();
        let call = Call::SetProgress {
            target: target.clone(),
            progress,
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        let replayed = !st.seen_keys.insert(format!("progress:{idempotency_key}"));
        if !replayed {
            st.progress.insert(target.clone(), progress);
        }
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn merge_change_request(
        &self,
        item: &WorkItemKey,
        number: u64,
        expected_head_sha: &str,
        commit_title: &str,
        commit_body: Option<&str>,
        idempotency_key: &str,
    ) -> Result<MergeOutcome> {
        let mut st = self.st();
        let call = Call::MergeChangeRequest {
            item: item.clone(),
            number,
            expected_head_sha: expected_head_sha.to_string(),
            commit_title: commit_title.to_string(),
            commit_body: commit_body.map(str::to_string),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(prev) = st.seen_merges.get(idempotency_key).cloned() {
            Self::record(&mut st, call, true);
            return Ok(prev);
        }
        st.next_id += 1;
        let id = st.next_id;
        let outcome = st
            .merge_outcomes
            .pop_front()
            .unwrap_or_else(|| MergeOutcome::Merged {
                sha: format!("fake-merge-{id}"),
            });
        st.seen_merges
            .insert(idempotency_key.to_string(), outcome.clone());
        Self::record(&mut st, call, false);
        Ok(outcome)
    }

    async fn get_merge_readiness(&self, item: &WorkItemKey, number: u64) -> Result<MergeReadiness> {
        let mut st = self.st();
        let call = Call::GetMergeReadiness {
            item: item.clone(),
            number,
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        let mut r = st
            .readiness
            .get(item)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("{item} readiness")))?;
        let reads = st.mergeable_reads.entry(item.clone()).or_default();
        *reads += 1;
        if *reads == 1 && r.mergeable.is_some() {
            r.mergeable = None;
            r.mergeable_state = "unknown".into();
        }
        Ok(r)
    }

    async fn update_branch(
        &self,
        item: &WorkItemKey,
        number: u64,
        expected_head_sha: &str,
    ) -> Result<UpdateBranchOutcome> {
        let mut st = self.st();
        let call = Call::UpdateBranch {
            item: item.clone(),
            number,
            expected_head_sha: expected_head_sha.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st
            .update_outcomes
            .pop_front()
            .unwrap_or(UpdateBranchOutcome::Updated))
    }

    async fn set_base(
        &self,
        item: &WorkItemKey,
        number: u64,
        new_base: &str,
        idempotency_key: &str,
    ) -> Result<()> {
        let mut st = self.st();
        let call = Call::SetBase {
            item: item.clone(),
            number,
            new_base: new_base.to_string(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(e) = st.fail_set_base.take() {
            Self::record(&mut st, call, false);
            return Err(e);
        }
        // A change request is known either as a seeded thread or one opened through the fake.
        let mut found = false;
        if let Some(t) = st.threads.get_mut(item)
            && t.item.kind == ItemKind::ChangeRequest
            && t.item.number == number
        {
            t.item.base_ref = Some(new_base.to_string());
            found = true;
        }
        for cr in st.change_requests.iter_mut().filter(|c| c.number == number) {
            cr.base_ref = Some(new_base.to_string());
            found = true;
        }
        if !found {
            Self::record(&mut st, call, false);
            return Err(Error::NotFound(format!("{item} change request {number}")));
        }
        let replayed = !st.seen_keys.insert(format!("set_base:{idempotency_key}"));
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn delete_branch(
        &self,
        repo: &RepoKey,
        branch: &str,
        idempotency_key: &str,
    ) -> Result<()> {
        let mut st = self.st();
        let call = Call::DeleteBranch {
            repo: repo.clone(),
            branch: branch.to_string(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(e) = st.fail_delete_branch.take() {
            Self::record(&mut st, call, false);
            return Err(e);
        }
        // Deleting a branch that is not there succeeds, like the provider's already-gone answer.
        st.branch_heads.remove(&(repo.clone(), branch.to_string()));
        let replayed = !st
            .seen_keys
            .insert(format!("delete_branch:{idempotency_key}"));
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn branch_head(&self, repo: &RepoKey, branch: &str) -> Result<Option<String>> {
        let mut st = self.st();
        let call = Call::BranchHead {
            repo: repo.clone(),
            branch: branch.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st
            .branch_heads
            .get(&(repo.clone(), branch.to_string()))
            .cloned())
    }

    async fn ahead_by(&self, repo: &RepoKey, base: &str, head: &str) -> Result<Option<u64>> {
        let mut st = self.st();
        let call = Call::AheadBy {
            repo: repo.clone(),
            base: base.to_string(),
            head: head.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st
            .ahead
            .get(&(repo.clone(), base.to_string(), head.to_string()))
            .copied())
    }

    async fn get_file(&self, repo: &RepoKey, git_ref: &str, path: &str) -> Result<Option<String>> {
        let mut st = self.st();
        let call = Call::GetFile {
            repo: repo.clone(),
            git_ref: git_ref.to_string(),
            path: path.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st
            .files
            .get(&(repo.clone(), git_ref.to_string(), path.to_string()))
            .cloned())
    }

    async fn list_files(
        &self,
        repo: &RepoKey,
        git_ref: &str,
    ) -> Result<Option<Vec<(String, u64)>>> {
        let st = self.generic(
            "list_files",
            serde_json::json!({"repo": repo.to_string(), "ref": git_ref}),
        )?;
        let mut v: Vec<(String, u64)> = st
            .files
            .iter()
            .filter(|((r, g, _), _)| r == repo && g == git_ref)
            .map(|((_, _, p), t)| (p.clone(), t.len() as u64))
            .collect();
        v.sort();
        Ok(Some(v))
    }

    async fn list_dependencies(&self, item: &WorkItemKey) -> Result<Vec<WorkItemKey>> {
        let mut st = self.st();
        let call = Call::ListDependencies { item: item.clone() };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st.dependencies.get(item).cloned().unwrap_or_default())
    }

    async fn add_dependency(
        &self,
        item: &WorkItemKey,
        depends_on: &WorkItemKey,
        idempotency_key: &str,
    ) -> Result<()> {
        let mut st = self.st();
        let call = Call::AddDependency {
            item: item.clone(),
            depends_on: depends_on.clone(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        let replayed = !st.seen_keys.insert(idempotency_key.to_string());
        Self::record(&mut st, call, replayed);
        let deps = st.dependencies.entry(item.clone()).or_default();
        if deps.contains(depends_on) {
            st.rejections.push(Rejection {
                op: "add_dependency",
                status: 422,
                message: "Validation Failed: dependency already exists".into(),
            });
        } else {
            deps.push(depends_on.clone());
        }
        Ok(())
    }

    async fn remove_dependency(
        &self,
        item: &WorkItemKey,
        depends_on: &WorkItemKey,
        idempotency_key: &str,
    ) -> Result<()> {
        let mut st = self.st();
        let call = Call::RemoveDependency {
            item: item.clone(),
            depends_on: depends_on.clone(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        let replayed = !st.seen_keys.insert(idempotency_key.to_string());
        Self::record(&mut st, call, replayed);
        if let Some(deps) = st.dependencies.get_mut(item) {
            deps.retain(|d| d != depends_on);
        }
        Ok(())
    }

    async fn create_issue(
        &self,
        repo: &RepoKey,
        title: &str,
        body: &str,
        labels: &[String],
        idempotency_key: &str,
    ) -> Result<WorkItemKey> {
        let mut st = self.st();
        let call = Call::CreateIssue {
            repo: repo.clone(),
            title: title.to_string(),
            body: body.to_string(),
            labels: labels.to_vec(),
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(existing) = st.seen_issues.get(idempotency_key).cloned() {
            Self::record(&mut st, call, true);
            return Ok(existing);
        }
        Self::record(&mut st, call, false);
        if st.next_issue == 0 {
            st.next_issue = 1000;
        }
        let key = WorkItemKey::new(repo.clone(), st.next_issue);
        st.next_issue += 1;
        st.seen_issues
            .insert(idempotency_key.to_string(), key.clone());
        st.created_issues
            .push((key.clone(), title.to_string(), body.to_string()));
        Ok(key)
    }

    async fn update_issue(&self, item: &WorkItemKey, body: &str) -> Result<()> {
        let mut st = self.st();
        let call = Call::UpdateIssue {
            item: item.clone(),
            body: body.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        let Some(entry) = st.created_issues.iter_mut().find(|(k, _, _)| k == item) else {
            return Err(Error::NotFound(format!("issue {item}")));
        };
        let replayed = entry.2 == body;
        entry.2 = body.to_string();
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn upsert_issue(
        &self,
        repo: &RepoKey,
        title: &str,
        body: &str,
        labels: &[String],
        dedupe_key: &str,
    ) -> Result<UpsertedIssue> {
        let mut st = self.st();
        let call = Call::UpsertIssue {
            repo: repo.clone(),
            title: title.to_string(),
            body: body.to_string(),
            labels: labels.to_vec(),
            dedupe_key: dedupe_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(existing) = st.seen_issues.get(dedupe_key).cloned() {
            let entry = st
                .created_issues
                .iter_mut()
                .find(|(k, _, _)| *k == existing)
                .expect("a seen issue was created");
            let unchanged = entry.1 == title && entry.2 == body;
            entry.1 = title.to_string();
            entry.2 = body.to_string();
            Self::record(&mut st, call, unchanged);
            let outcome = if unchanged {
                UpsertOutcome::Unchanged
            } else {
                UpsertOutcome::Updated
            };
            return Ok(UpsertedIssue {
                item: existing,
                outcome,
            });
        }
        Self::record(&mut st, call, false);
        if st.next_issue == 0 {
            st.next_issue = 1000;
        }
        let key = WorkItemKey::new(repo.clone(), st.next_issue);
        st.next_issue += 1;
        st.seen_issues.insert(dedupe_key.to_string(), key.clone());
        st.created_issues
            .push((key.clone(), title.to_string(), body.to_string()));
        Ok(UpsertedIssue {
            item: key,
            outcome: UpsertOutcome::Created,
        })
    }

    async fn close_issue(&self, item: &WorkItemKey, reason: CloseReason) -> Result<()> {
        let mut st = self.st();
        let call = Call::CloseIssue {
            item: item.clone(),
            reason,
        };
        Self::take_failure(&mut st, call.clone())?;
        let replayed = st.closed_issues.contains_key(item);
        if !replayed {
            st.closed_issues.insert(item.clone(), reason);
        }
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn link_sub_issue(&self, parent: &WorkItemKey, child: &WorkItemKey) -> Result<()> {
        let mut st = self.st();
        let call = Call::LinkSubIssue {
            parent: parent.clone(),
            child: child.clone(),
        };
        Self::take_failure(&mut st, call.clone())?;
        let linked = st.sub_issues.entry(parent.clone()).or_default();
        let replayed = linked.contains(child);
        if !replayed {
            linked.push(child.clone());
        }
        if replayed {
            st.rejections.push(Rejection {
                op: "link_sub_issue",
                status: 422,
                message: "Validation Failed: sub-issue already exists".into(),
            });
        }
        Self::record(&mut st, call, replayed);
        Ok(())
    }

    async fn check_permission(&self, repo: &RepoKey, actor: &Actor) -> Result<Permission> {
        let mut st = self.st();
        let call = Call::CheckPermission {
            repo: repo.clone(),
            actor: actor.clone(),
        };
        Self::take_failure(&mut st, call.clone())?;
        Self::record(&mut st, call, false);
        Ok(st
            .permissions
            .get(&(repo.clone(), actor.login.clone()))
            .copied()
            .or(st.default_permission)
            .unwrap_or(Permission::None))
    }

    async fn mint_clone_credential(
        &self,
        repo: &RepoKey,
        scope: CredScope,
        idempotency_key: &str,
    ) -> Result<ShortLivedCred> {
        let mut st = self.st();
        let call = Call::MintCloneCredential {
            repo: repo.clone(),
            scope,
            idempotency_key: idempotency_key.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if let Some(prev) = st.seen_creds.get(idempotency_key).cloned() {
            Self::record(&mut st, call, true);
            return Ok(prev);
        }
        st.next_id += 1;
        let cred = ShortLivedCred {
            token: format!("fake-token-{}", st.next_id),
            repo: repo.clone(),
            scope,
            expires_at: Utc::now() + Duration::hours(1),
        };
        st.seen_creds
            .insert(idempotency_key.to_string(), cred.clone());
        Self::record(&mut st, call, false);
        Ok(cred)
    }

    async fn mint_read_credential(
        &self,
        repos: &[RepoKey],
        purpose: &str,
    ) -> Result<Option<ReadCred>> {
        let mut st = self.st();
        let call = Call::MintReadCredential {
            repos: repos.to_vec(),
            purpose: purpose.to_string(),
        };
        Self::take_failure(&mut st, call.clone())?;
        if repos.is_empty() {
            Self::record(&mut st, call, false);
            return Err(Error::Invalid("no repos for read credential".into()));
        }
        st.next_id += 1;
        let cred = ReadCred {
            token: format!("fake-read-token-{}", st.next_id),
            repos: repos.to_vec(),
            expires_at: Utc::now() + Duration::hours(1),
        };
        Self::record(&mut st, call, false);
        Ok(Some(cred))
    }

    async fn mint_ci_read_credential(&self, repo: &RepoKey) -> Result<Option<ReadCred>> {
        let mut st = self.st();
        let call = Call::MintCiReadCredential { repo: repo.clone() };
        Self::take_failure(&mut st, call.clone())?;
        st.next_id += 1;
        let cred = ReadCred {
            token: format!("fake-ci-token-{}", st.next_id),
            repos: vec![repo.clone()],
            expires_at: Utc::now() + Duration::hours(1),
        };
        Self::record(&mut st, call, false);
        Ok(Some(cred))
    }

    async fn resolve_ref(&self, repo: &RepoKey, git_ref: &str) -> Result<Option<String>> {
        let st = self.generic(
            "resolve_ref",
            serde_json::json!({"repo": repo.to_string(), "ref": git_ref}),
        )?;
        let k = (repo.clone(), git_ref.to_string());
        Ok(st.refs.get(&k).or_else(|| st.branch_heads.get(&k)).cloned())
    }

    async fn list_open_issues(&self, repo: &RepoKey) -> Result<Vec<ItemRef>> {
        let st = self.generic(
            "list_open_issues",
            serde_json::json!({"repo": repo.to_string()}),
        )?;
        Ok(st.open_issues.get(repo).cloned().unwrap_or_default())
    }

    async fn pr_references(&self, item: &WorkItemKey) -> Result<Vec<u64>> {
        let st = self.generic(
            "pr_references",
            serde_json::json!({"item": item.to_string()}),
        )?;
        Ok(st.pr_refs.get(item).cloned().unwrap_or_default())
    }

    async fn add_label(&self, item: &WorkItemKey, label: &str) -> Result<()> {
        let mut st = self.generic(
            "add_label",
            serde_json::json!({"item": item.to_string(), "label": label}),
        )?;
        let l = st.labels.entry(item.clone()).or_default();
        if !l.iter().any(|x| x == label) {
            l.push(label.to_string());
        }
        Ok(())
    }

    async fn remove_label(&self, item: &WorkItemKey, label: &str) -> Result<()> {
        let mut st = self.generic(
            "remove_label",
            serde_json::json!({"item": item.to_string(), "label": label}),
        )?;
        st.labels
            .entry(item.clone())
            .or_default()
            .retain(|x| x != label);
        Ok(())
    }

    async fn update_pull_request(
        &self,
        item: &WorkItemKey,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Result<()> {
        let _st = self.generic(
            "update_pull_request",
            serde_json::json!({"item": item.to_string(), "title": title, "body": body}),
        )?;
        Ok(())
    }

    async fn ci_details(&self, repo: &RepoKey, sha: &str) -> Result<serde_json::Value> {
        let st = self.generic(
            "ci_details",
            serde_json::json!({"repo": repo.to_string(), "sha": sha}),
        )?;
        Ok(st
            .ci
            .get(&(repo.clone(), sha.to_string()))
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"state": "pending", "failing": []})))
    }

    async fn list_reviews(&self, item: &WorkItemKey) -> Result<serde_json::Value> {
        let st = self.generic(
            "list_reviews",
            serde_json::json!({"item": item.to_string()}),
        )?;
        Ok(st
            .reviews
            .get(item)
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])))
    }

    async fn contains(&self, repo: &RepoKey, sha: &str, git_ref: &str) -> Result<Option<bool>> {
        let st = self.generic(
            "contains",
            serde_json::json!({"repo": repo.to_string(), "sha": sha, "ref": git_ref}),
        )?;
        Ok(st
            .contains
            .get(&(repo.clone(), sha.to_string(), git_ref.to_string()))
            .copied())
    }

    async fn latest_workflow_run(
        &self,
        repo: &RepoKey,
        workflow: &str,
        branch: Option<&str>,
    ) -> Result<Option<WorkflowRunInfo>> {
        let st = self.generic(
            "latest_workflow_run",
            serde_json::json!({"repo": repo.to_string(), "workflow": workflow, "branch": branch}),
        )?;
        let branch = branch
            .map(str::to_string)
            .or_else(|| st.default_branches.get(repo).cloned())
            .unwrap_or_else(|| "main".to_string());
        Ok(st
            .workflow_runs
            .get(&(repo.clone(), workflow.to_string(), branch))
            .cloned())
    }

    async fn mint_step_token(
        &self,
        repos: &[RepoKey],
        permissions: &std::collections::BTreeMap<String, String>,
    ) -> Result<ReadCred> {
        let mut st = self.generic(
            "mint_step_token",
            serde_json::json!({
                "repos": repos.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
                "permissions": permissions,
            }),
        )?;
        if let Some(e) = st.fail_step_token.take() {
            return Err(e);
        }
        st.next_id += 1;
        Ok(ReadCred {
            token: format!("fake-step-token-{}", st.next_id),
            repos: repos.to_vec(),
            expires_at: Utc::now() + Duration::hours(1),
        })
    }
}
