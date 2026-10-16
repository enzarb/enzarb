//! `retarget_change_request`: rebase enzarb's own stacked child branch onto the default branch
//! after its parent change request was squash-merged, then set the change request's base.
//!
//! This is the one job that force-pushes, and only to a branch enzarb itself pushed. The push
//! happens in the sandbox (`RunStart.retarget`, see `contracts.md`), which refuses anything but
//! an enzarb branch at an expected head; this module adds the checks that need the provider:
//!
//! 1. under the repository's merge lock, re-read the change request: the head must still be the
//!    one the job was queued for, else the branch moved and nothing is touched (`lease_failed`);
//! 2. mint a `read_write` credential and start the run. The job stays `running`; the callback
//!    (`callback::finish_retarget`) reads the run's result, sets the base with
//!    `Connector::set_base` and records the outcome.
//!
//! Git state, not the change request's current base, decides whether the rebase is still needed
//! (GitHub moves the base by itself when the parent branch is deleted, without rebasing).

use enzarb_connector::{Connector, CredScope, WorkItemKey};
use enzarb_proto::{Retarget, RunFinished, RunOutcome, RunStart};
use serde::Deserialize;
use serde_json::{Value, json};

use super::agent::repo_url;
use super::callback::{callback_token, run_id};
use super::{Claimed, Done, Executor, JobError};
use crate::machine::RetargetOutcome;
use crate::machine::stack::is_enzarb_branch;
use crate::results::JobResult;

/// Wall-clock limit of the sandbox run (fetch, rebase, push).
const RETARGET_TIMEOUT_SECS: u64 = 900;

/// How long a retarget waits before retrying when a merge holds the repository lock.
const LOCK_RETRY: std::time::Duration = std::time::Duration::from_secs(5);

/// `jobs.input` of a `retarget_change_request` job.
#[derive(Debug, Deserialize)]
pub(crate) struct RetargetInput {
    pub item: WorkItemKey,
    pub number: u64,
    pub head_ref: String,
    pub expected_sha: String,
    pub old_base: String,
    pub onto: String,
}

fn refused(outcome: &str, reason: &str) -> Done {
    Done::Output(json!({ "outcome": outcome, "reason": reason }))
}

fn lock_error(e: sqlx::Error) -> JobError {
    JobError::transient(format!("retarget lock: {e}"))
}

impl Executor {
    pub(crate) async fn start_retarget(&self, job: &Claimed) -> Result<Done, JobError> {
        let i: RetargetInput = serde_json::from_value(job.input.clone())
            .map_err(|e| JobError::permanent(format!("unreadable retarget input: {e}")))?;
        // Never even ask the sandbox to touch a branch that is not enzarb's own.
        if !is_enzarb_branch(&i.head_ref) || i.head_ref == i.onto {
            return Ok(refused("rejected", "the branch is not an enzarb branch"));
        }
        if i.expected_sha.is_empty() || i.old_base.is_empty() || i.onto.is_empty() {
            return Ok(refused("rejected", "the retarget input is incomplete"));
        }
        let project = job
            .project_id
            .clone()
            .ok_or_else(|| JobError::permanent("job has no project"))?;
        let repo = &i.item.repo;
        let url = repo_url(repo).ok_or_else(|| {
            JobError::permanent(format!("unsupported connector {:?}", repo.connector))
        })?;
        let connector = self.connector_for(&repo.connector)?;

        // One merge or retarget start at a time per repository: the transaction holds the
        // same advisory lock as a merge and ends with this call.
        let mut tx = self.pool.begin().await.map_err(lock_error)?;
        let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))")
            .bind(format!("merge:{repo}"))
            .fetch_one(&mut *tx)
            .await
            .map_err(lock_error)?;
        if !locked {
            return Err(JobError::contended(
                "another merge or retarget is running for this repository",
                LOCK_RETRY,
            ));
        }

        let pr = connector.fetch_thread(&i.item).await?.item;
        if pr.head_sha.as_deref() != Some(i.expected_sha.as_str()) {
            return Ok(refused(
                "lease_failed",
                "the branch changed since the retarget was queued",
            ));
        }
        if pr.head_ref.as_deref() != Some(i.head_ref.as_str()) {
            return Ok(refused("rejected", "the change request's branch changed"));
        }

        let session = self.dial(&project).await?;
        let rid = run_id(job.id, job.limit_wait_count + job.restart_retries);
        let run = match self.retarget_run(job, &i, &rid, url, connector).await {
            Ok(run) => run,
            Err(e) => {
                session.close().await;
                return Err(e);
            }
        };
        let done = self
            .launch_run(
                session,
                job.id,
                job.attempt,
                &rid,
                run,
                RETARGET_TIMEOUT_SECS,
            )
            .await?;
        tx.commit().await.map_err(lock_error)?;
        Ok(done)
    }

    /// The result of the finished run as the job's result. After a successful rebase and push
    /// the change request's base is set here (an already-correct base counts as success); the
    /// push cannot be undone, so a provider that refuses the change is reported as
    /// `base_set: false` for the machine to hand to a person.
    pub(crate) async fn finish_retarget(&self, input: &Value, body: &RunFinished) -> JobResult {
        if body.outcome != RunOutcome::Succeeded {
            return JobResult::Failed {
                error: format!(
                    "the retarget run ended as {:?}: {}",
                    body.outcome,
                    body.summary.chars().take(1000).collect::<String>()
                ),
            };
        }
        let raw = json!({ "outcome": "succeeded", "summary": body.summary });
        let outcome = RetargetOutcome::from_output(&raw);
        let mut output = serde_json::from_str::<Value>(body.summary.trim())
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| {
                json!({
                    "outcome": "rejected",
                    "reason": "the sandbox did not report a retarget result; it may not support it",
                })
            });
        if let RetargetOutcome::Retargeted { .. } = outcome {
            let set = async {
                let i: RetargetInput = serde_json::from_value(input.clone())
                    .map_err(|e| format!("unreadable retarget input: {e}"))?;
                let c = self
                    .connector_for(&i.item.repo.connector)
                    .map_err(|e| e.message)?;
                let key = format!("retarget-base:{}", body.run_id);
                c.set_base(&i.item, i.number, &i.onto, &key)
                    .await
                    .map_err(|e| e.to_string())
            };
            if let (Err(reason), Some(o)) = (set.await, output.as_object_mut()) {
                o.insert("base_set".into(), json!(false));
                o.insert("reason".into(), json!(reason));
            }
        }
        JobResult::Succeeded { output }
    }

    async fn retarget_run(
        &self,
        job: &Claimed,
        i: &RetargetInput,
        rid: &str,
        repo_url: String,
        connector: &std::sync::Arc<dyn Connector>,
    ) -> Result<RunStart, JobError> {
        let cred = connector
            .mint_clone_credential(
                &i.item.repo,
                CredScope::ReadWrite,
                &format!("job:{}:{}", job.id, job.attempt),
            )
            .await?;
        Ok(RunStart {
            run_id: rid.to_string(),
            repo_url,
            base_ref: i.onto.clone(),
            branch: i.head_ref.clone(),
            // Never an ordinary push: a sandbox that does not know `retarget` pushes nothing.
            push: false,
            clone_token: cred.token,
            deps_token: None,
            npm_token: None,
            ci_read_token: None,
            claude_token: None,
            prompt: format!(
                "retarget {} from {} onto {}",
                i.head_ref, i.old_base, i.onto
            ),
            timeout_secs: RETARGET_TIMEOUT_SECS,
            callback_url: format!("{}/v1/runs/{rid}/finished", self.cfg.callback_base_url),
            callback_token: callback_token(&self.cfg.callback_secret, rid),
            log_channel: None,
            retarget: Some(Retarget {
                head_ref: i.head_ref.clone(),
                onto: i.onto.clone(),
                old_base: i.old_base.clone(),
                expected_sha: i.expected_sha.clone(),
                protected: vec![i.onto.clone()],
            }),
            graph: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_parses() {
        let v = json!({
            "item": "github:o/r#7", "number": 7, "head_ref": "enzarb/issue-7",
            "expected_sha": "a", "old_base": "b", "onto": "main"
        });
        let i: RetargetInput = serde_json::from_value(v).unwrap();
        assert_eq!(i.number, 7);
        assert_eq!(i.onto, "main");
    }
}
