//! Connector jobs: `set_progress`, `post_comment`, `open_change_request`,
//! `check_merge_readiness`, `merge_change_request`.

use std::collections::HashMap;
use std::sync::Arc;

use enzarb_connector::{
    ChangeRequestSpec, CiState, CloseReason, Connector, MergeOutcome, MergeReadiness, Progress,
    ProgressTarget, UpdateBranchOutcome, WorkItemKey, epic,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Claimed, Done, Executor, JobError};
use crate::conflict::emit_merge_conflict;
use crate::machine::stack::is_enzarb_branch;
use crate::machine::{JobKind, SubIssue, substitute_refs};

#[derive(Deserialize)]
struct SetProgress {
    target: ProgressTarget,
    progress: Progress,
}

#[derive(Deserialize)]
struct PostComment {
    item: WorkItemKey,
    body: String,
}

#[derive(Deserialize)]
struct CloseIssue {
    item: WorkItemKey,
    reason: CloseReason,
}

#[derive(Deserialize)]
struct OpenCr {
    spec: ChangeRequestSpec,
}

#[derive(Deserialize)]
struct CheckReadiness {
    item: WorkItemKey,
    number: u64,
}

#[derive(Deserialize)]
struct MergeCr {
    item: WorkItemKey,
    number: u64,
    title: String,
    expected_head_sha: String,
    #[serde(default)]
    actor: String,
    /// The head branch to delete after the merge, when the user asked for that.
    #[serde(default)]
    delete_branch: Option<String>,
}

#[derive(Deserialize)]
struct UpdateBranch {
    item: WorkItemKey,
    number: u64,
    expected_head_sha: String,
}

#[derive(Deserialize)]
struct CreateSubIssues {
    item: WorkItemKey,
    depth: u32,
    /// In creation order: every entry comes after the ones it depends on.
    sub_issues: Vec<SubIssue>,
}

/// A title on one line without `#`, backticks or line breaks, so it cannot read as an issue
/// reference or open a code span in the text around it.
fn plain_title(title: &str) -> String {
    title
        .replace(['#', '`'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The description of a sub-issue: the plan's text, what it waits for (parsed as text
/// dependencies when the issue opens), and the hidden markers that tie it to its epic.
///
/// `refs` turns a `{{issue:key}}` placeholder of the plan's text into what the description
/// shows for it. The footer has one `Blocked by #N (title)` line per blocker: the text
/// dependency parser reads one reference per keyword, so a title never hides a blocker, and
/// titles are stripped of anything it could take for a reference.
fn child_body(
    epic: &WorkItemKey,
    depth: u32,
    s: &SubIssue,
    blockers: &[(WorkItemKey, &str)],
    refs: impl Fn(&str) -> Option<String>,
) -> String {
    let mut body = substitute_refs(s.body.trim(), refs);
    body.push_str(&format!("\n\nPart of the epic #{}.", epic.number));
    if !blockers.is_empty() {
        let lines: Vec<String> = blockers
            .iter()
            .map(|(b, title)| format!("Blocked by #{} ({})", b.number, plain_title(title)))
            .collect();
        body.push_str(&format!("\n\n{}", lines.join("\n")));
    }
    format!(
        "{body}\n\n{}\n{}",
        epic::epic_marker(epic, &s.key),
        epic::depth_marker(depth)
    )
}

/// How long a merge waits before retrying when another merge holds the repository lock.
const MERGE_LOCK_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

fn lock_error(e: sqlx::Error) -> JobError {
    JobError::transient(format!("merge lock: {e}"))
}

fn ci_wire(ci: CiState) -> &'static str {
    match ci {
        CiState::Success => "success",
        CiState::Failure => "failure",
        CiState::Pending => "pending",
    }
}

fn input<T: serde::de::DeserializeOwned>(job: &Claimed) -> Result<T, JobError> {
    serde_json::from_value(job.input.clone())
        .map_err(|e| JobError::permanent(format!("unreadable {} input: {e}", job.kind)))
}

impl Executor {
    pub(crate) fn connector_for(&self, name: &str) -> Result<&Arc<dyn Connector>, JobError> {
        self.connectors
            .get(name)
            .ok_or_else(|| JobError::permanent(format!("no connector named {name:?}")))
    }

    pub(crate) async fn run_connector_job(
        &self,
        job: &Claimed,
        kind: JobKind,
    ) -> Result<Done, JobError> {
        let key = job.idempotency_key.as_str();
        let output = match kind {
            JobKind::SetProgress => {
                let i: SetProgress = input(job)?;
                if i.progress == Progress::Working && self.agent_run_ended(job).await {
                    return Ok(Done::Output(json!({ "skipped": "the run already ended" })));
                }
                let c = self.connector_for(&i.target.item.repo.connector)?;
                c.set_progress(&i.target, i.progress, key).await?;
                json!({})
            }
            JobKind::PostComment => {
                let i: PostComment = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                to_value(c.post_comment(&i.item, &i.body, key).await?)
            }
            JobKind::CloseIssue => {
                let i: CloseIssue = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                c.close_issue(&i.item, i.reason).await?;
                json!({})
            }
            JobKind::OpenChangeRequest => {
                let i: OpenCr = input(job)?;
                let c = self.connector_for(&i.spec.repo.connector)?;
                to_value(c.open_change_request(i.spec, key).await?)
            }
            JobKind::CheckMergeReadiness => {
                let i: CheckReadiness = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                let r = c.get_merge_readiness(&i.item, i.number).await?;
                self.emit_if_conflicting(&i.item, &r).await;
                json!({
                    "head_sha": r.head_sha,
                    "ci": ci_wire(r.ci),
                    "mergeable": r.mergeable,
                    "mergeable_state": r.mergeable_state,
                    "base_sha": r.base_sha,
                })
            }
            JobKind::MergeChangeRequest => {
                let i: MergeCr = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                // One merge at a time per repository, across all workers: the transaction
                // holds an advisory lock that ends with it, so a crashed worker frees it.
                let mut tx = self.pool.begin().await.map_err(lock_error)?;
                let locked: bool =
                    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))")
                        .bind(format!("merge:{}", i.item.repo))
                        .fetch_one(&mut *tx)
                        .await
                        .map_err(lock_error)?;
                if !locked {
                    return Err(JobError::contended(
                        "another merge is running for this repository",
                        MERGE_LOCK_RETRY,
                    ));
                }
                let out = self.merge(c.as_ref(), &i, key).await?;
                tx.commit().await.map_err(lock_error)?;
                out
            }
            JobKind::UpdateBranch => {
                let i: UpdateBranch = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                match c
                    .update_branch(&i.item, i.number, &i.expected_head_sha)
                    .await?
                {
                    UpdateBranchOutcome::Updated => json!({ "outcome": "updated" }),
                    UpdateBranchOutcome::Refused { reason } => {
                        json!({ "outcome": "refused", "reason": reason })
                    }
                }
            }
            JobKind::CreateSubIssues => {
                let i: CreateSubIssues = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                self.create_sub_issues(c.as_ref(), &i).await?
            }
            other => {
                return Err(JobError::permanent(format!(
                    "{} is not a connector job",
                    other.as_str()
                )));
            }
        };
        Ok(Done::Output(output))
    }

    /// Create the epic's sub-issues, link them to it and record who waits for whom. Safe to
    /// run again after a failure part-way: each issue is found by its marker, links and
    /// dependencies are idempotent, so a retry resumes where the last run stopped.
    async fn create_sub_issues(
        &self,
        c: &dyn Connector,
        i: &CreateSubIssues,
    ) -> Result<Value, JobError> {
        let mut made: HashMap<&str, WorkItemKey> = HashMap::new();
        let mut children: Vec<Value> = Vec::new();
        let mut edges: Vec<(WorkItemKey, Vec<WorkItemKey>)> = Vec::new();
        let titles: HashMap<&str, &str> = i
            .sub_issues
            .iter()
            .map(|s| (s.key.as_str(), s.title.as_str()))
            .collect();
        // Pass 1: create in dependency order. A sibling's number is not known yet, so the
        // text names it by title only.
        let mut created_bodies: Vec<String> = Vec::new();
        for s in &i.sub_issues {
            let mut blockers = Vec::new();
            for d in &s.depends_on {
                let Some(b) = made.get(d.as_str()) else {
                    return Err(JobError::permanent(format!(
                        "sub-issue {:?} depends on {d:?}, which is not created before it",
                        s.key
                    )));
                };
                blockers.push((b.clone(), titles.get(d.as_str()).copied().unwrap_or("")));
            }
            let body = child_body(&i.item, i.depth, s, &blockers, |k| {
                titles.get(k).map(|t| plain_title(t))
            });
            let idem = format!("epic-{}-{}", i.item, s.key);
            let child = c
                .create_issue(&i.item.repo, &s.title, &body, &[], &idem)
                .await?;
            made.insert(&s.key, child.clone());
            children.push(json!({ "key": s.key, "item": child, "title": s.title }));
            created_bodies.push(body);
            edges.push((child, blockers.into_iter().map(|(b, _)| b).collect()));
        }
        // Pass 2: every child exists, so a placeholder can become `#N (title)`. The result
        // depends only on the plan and the numbers, so a retry writes the same text.
        for ((s, (child, _)), first) in i.sub_issues.iter().zip(&edges).zip(&created_bodies) {
            let blockers: Vec<(WorkItemKey, &str)> = s
                .depends_on
                .iter()
                .filter_map(|d| Some((made.get(d.as_str())?.clone(), *titles.get(d.as_str())?)))
                .collect();
            let body = child_body(&i.item, i.depth, s, &blockers, |k| {
                let n = made.get(k)?;
                Some(format!("#{} ({})", n.number, plain_title(titles.get(k)?)))
            });
            if *first != body {
                c.update_issue(child, &body).await?;
            }
        }
        for (child, blockers) in &edges {
            // Sub-issue links are a presentation of the breakdown; a connector without them
            // does not stop the epic. Anything worth retrying does.
            match c.link_sub_issue(&i.item, child).await {
                Ok(()) => {}
                Err(e) if e.is_retryable() => return Err(e.into()),
                Err(e) => {
                    tracing::warn!(error = %e, epic = %i.item, child = %child, "linking a sub-issue failed")
                }
            }
            // Native "blocked by" relationships, best effort: the database below is the
            // source of truth for the engine.
            for blocker in blockers {
                let key = format!("epic-dep-{}-{}-{}", i.item, child, blocker);
                if let Err(e) = c.add_dependency(child, blocker, &key).await {
                    tracing::debug!(error = %e, "adding a native dependency failed");
                }
            }
            let key = format!("epic-dep-{}-{}-{}", i.item, i.item, child);
            if let Err(e) = c.add_dependency(&i.item, child, &key).await {
                tracing::debug!(error = %e, "adding the epic's native dependency failed");
            }
        }
        crate::dependencies::record_epic(&self.pool, &i.item, &edges)
            .await
            .map_err(|e| JobError::transient(format!("recording epic dependencies: {e}")))?;
        Ok(json!({ "children": children }))
    }

    /// Squash-merge after re-checking readiness. A refused merge is an output the machine
    /// reads (`head_moved`, `not_mergeable`), not a job error, so it is never retried.
    async fn merge(&self, c: &dyn Connector, i: &MergeCr, key: &str) -> Result<Value, JobError> {
        let outcome = |outcome: &str, extra: Value| {
            let mut v = json!({ "outcome": outcome, "actor": i.actor });
            if let (Some(m), Some(e)) = (v.as_object_mut(), extra.as_object()) {
                m.extend(e.clone());
            }
            v
        };
        let r = c.get_merge_readiness(&i.item, i.number).await?;
        self.emit_if_conflicting(&i.item, &r).await;
        if r.head_sha != i.expected_head_sha {
            return Ok(outcome("head_moved", json!({})));
        }
        if r.mergeable_state == "behind" {
            // Never merge on a stale base: merge the base into the branch (no history
            // rewrite) and let CI run on the new head.
            return Ok(
                match c
                    .update_branch(&i.item, i.number, &i.expected_head_sha)
                    .await?
                {
                    UpdateBranchOutcome::Updated => outcome("updated_branch", json!({})),
                    UpdateBranchOutcome::Refused { reason } => {
                        outcome("not_mergeable", json!({ "reason": reason }))
                    }
                },
            );
        }
        if r.ci != CiState::Success {
            let reason = format!("CI is not passing on the head ({})", ci_wire(r.ci));
            return Ok(outcome("not_mergeable", json!({ "reason": reason })));
        }
        if r.mergeable != Some(true) {
            let reason = match r.mergeable {
                Some(_) if !r.mergeable_state.is_empty() => {
                    format!("not mergeable ({})", r.mergeable_state)
                }
                Some(_) => "not mergeable".to_string(),
                None => "mergeability is still being computed".to_string(),
            };
            return Ok(outcome("not_mergeable", json!({ "reason": reason })));
        }
        Ok(
            match c
                .merge_change_request(&i.item, i.number, &i.expected_head_sha, &i.title, None, key)
                .await?
            {
                MergeOutcome::Merged { sha } => {
                    let branch = self.delete_merged_branch(c, i, key).await;
                    outcome("merged", json!({ "sha": sha, "branch": branch }))
                }
                MergeOutcome::AlreadyMerged { sha } => {
                    // The deletion is idempotent, so a restarted job simply tries again.
                    let branch = self.delete_merged_branch(c, i, key).await;
                    outcome("already_merged", json!({ "sha": sha, "branch": branch }))
                }
                MergeOutcome::HeadMoved => outcome("head_moved", json!({})),
                MergeOutcome::NotMergeable { reason } => {
                    outcome("not_mergeable", json!({ "reason": reason }))
                }
            },
        )
    }

    /// Delete the head branch of a merged change request when the user asked for it, and say
    /// what happened (the machine records it in the transition note). Best effort: nothing here
    /// can fail the merge. Only enzarb's own branches are deleted, and not while another open
    /// change request is stacked on it: GitHub would then change that request's base itself,
    /// without the rebase the retarget flow does.
    async fn delete_merged_branch(&self, c: &dyn Connector, i: &MergeCr, key: &str) -> String {
        let Some(branch) = i.delete_branch.as_deref().filter(|b| !b.is_empty()) else {
            return String::new();
        };
        if !is_enzarb_branch(branch) {
            return format!("branch {branch} kept: it is not an enzarb branch");
        }
        let stacked: Result<i64, _> = sqlx::query_scalar(
            "SELECT count(*) FROM workflows.work_items
             WHERE workflow = 'change_request' AND state_data->>'stacked_on' = $1
               AND state NOT IN ('Merged', 'Closed', 'Cancelled')",
        )
        .bind(i.item.to_string())
        .fetch_one(&self.pool)
        .await;
        match stacked {
            Ok(0) => {}
            Ok(_) => {
                return format!(
                    "branch {branch} kept: change requests stacked on it are not retargeted yet"
                );
            }
            Err(e) => {
                return format!("branch {branch} kept: could not check stacked requests ({e})");
            }
        }
        match c
            .delete_branch(&i.item.repo, branch, &format!("{key}:delete_branch"))
            .await
        {
            Ok(()) => format!("branch {branch} deleted"),
            Err(e) => format!("branch {branch} not deleted: {e}"),
        }
    }

    /// Emit `MergeConflictDetected` when the provider explicitly says the change request is
    /// unmergeable (`mergeable == Some(false)`) and reported the base head. A failed emit is
    /// logged, never a job error: the next readiness poll retries and the dedupe key makes
    /// that safe.
    async fn emit_if_conflicting(&self, item: &WorkItemKey, r: &MergeReadiness) {
        if r.mergeable != Some(false) {
            return;
        }
        let Some(base_sha) = r.base_sha.as_deref() else {
            return;
        };
        if let Err(e) = emit_merge_conflict(&self.pool, &item.to_string(), base_sha).await {
            tracing::warn!(error = %e, item = %item, "emitting MergeConflictDetected failed");
        }
    }

    /// True when the agent job queued by the same event already succeeded or failed, so a
    /// `Working` marker would overwrite its `Done` / `Failed`.
    async fn agent_run_ended(&self, job: &Claimed) -> bool {
        sqlx::query_scalar(
            r"SELECT EXISTS (SELECT 1 FROM jobs a
                  WHERE a.work_item_id = $1 AND a.kind LIKE 'agent\_%'
                    AND a.status IN ('succeeded', 'failed')
                    AND starts_with(a.idempotency_key, regexp_replace($2, '[^:]*$', '')))",
        )
        .bind(job.work_item_id)
        .bind(&job.idempotency_key)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}
