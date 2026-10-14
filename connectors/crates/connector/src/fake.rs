//! In-memory [`Connector`] that records calls, for tests (feature `fake`).

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, MutexGuard};

use async_trait::async_trait;
use chrono::{Duration, Utc};

use crate::{
    Actor, ChangeRequestSpec, CommentRef, CommitRef, Connector, CredScope, Error, ItemKind,
    ItemRef, MergeOutcome, MergeReadiness, Permission, Progress, ProgressTarget, ReadCred, RepoKey,
    Result, ShortLivedCred, Status, Thread, ThreadComment, UpdateBranchOutcome, WorkItemKey,
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
    BranchHead {
        repo: RepoKey,
        branch: String,
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
}

/// A recorded call. `replayed` is true when the idempotency key had been seen
/// before, so the call had no new effect.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCall {
    pub call: Call,
    pub replayed: bool,
}

#[derive(Default)]
struct State {
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
    merge_outcomes: VecDeque<MergeOutcome>,
    seen_merges: HashMap<String, MergeOutcome>,
    readiness: HashMap<WorkItemKey, MergeReadiness>,
    update_outcomes: VecDeque<UpdateBranchOutcome>,
    branch_heads: HashMap<(RepoKey, String), String>,
    dependencies: HashMap<WorkItemKey, Vec<WorkItemKey>>,
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
    pub fn set_merge_readiness(&self, item: WorkItemKey, readiness: MergeReadiness) {
        self.st().readiness.insert(item, readiness);
    }

    /// Seed the head sha returned by `branch_head` (unseeded branches are `None`).
    pub fn set_branch_head(&self, repo: RepoKey, branch: &str, sha: &str) {
        self.st()
            .branch_heads
            .insert((repo, branch.to_string()), sha.to_string());
    }

    /// Seed the relationships `list_dependencies` returns for `item`.
    pub fn set_dependencies(&self, item: WorkItemKey, depends_on: Vec<WorkItemKey>) {
        self.st().dependencies.insert(item, depends_on);
    }

    /// Make the next call (any method) fail with `error`, once.
    pub fn fail_next(&self, error: Error) {
        self.st().fail_next = Some(error);
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
        st.readiness
            .get(item)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("{item} readiness")))
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
        if !deps.contains(depends_on) {
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
}
