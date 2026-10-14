//! Connector jobs: `set_progress`, `post_comment`, `open_change_request`,
//! `check_merge_readiness`, `merge_change_request`.

use std::sync::Arc;

use enzarb_connector::{
    ChangeRequestSpec, CiState, Connector, MergeOutcome, MergeReadiness, Progress, ProgressTarget,
    UpdateBranchOutcome, WorkItemKey,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Claimed, Done, Executor, JobError};
use crate::conflict::emit_merge_conflict;
use crate::machine::JobKind;

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
            other => {
                return Err(JobError::permanent(format!(
                    "{} is not a connector job",
                    other.as_str()
                )));
            }
        };
        Ok(Done::Output(output))
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
                MergeOutcome::Merged { sha } => outcome("merged", json!({ "sha": sha })),
                MergeOutcome::AlreadyMerged { sha } => {
                    outcome("already_merged", json!({ "sha": sha }))
                }
                MergeOutcome::HeadMoved => outcome("head_moved", json!({})),
                MergeOutcome::NotMergeable { reason } => {
                    outcome("not_mergeable", json!({ "reason": reason }))
                }
            },
        )
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
