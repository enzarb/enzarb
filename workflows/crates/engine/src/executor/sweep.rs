//! Background upkeep: crashed leases, agent runs past their deadline, cancelled runs.

use enzarb_proto::RunPhase;
use enzarb_proto::session::SessionError;
use serde_json::{Value, json};

use super::callback::run_id;
use super::{Executor, JobError};
use crate::error::Error;
use crate::machine::AgentJobInput;
use crate::results::{JobResult, ingest_job_result};

impl Executor {
    /// One sweep. Errors are logged; the next sweep tries again.
    pub async fn sweep(&self) {
        if let Err(e) = self.reap_leases().await {
            tracing::warn!(error = %e, "sweep: lease recovery failed");
        }
        if let Err(e) = self.poll_deadlines().await {
            tracing::warn!(error = %e, "sweep: deadline polling failed");
        }
        if let Err(e) = self.cancel_runs().await {
            tracing::warn!(error = %e, "sweep: run cancellation failed");
        }
    }

    /// `running` jobs whose lease expired: requeue them, or report failure once their
    /// attempts are used up. Returns how many were touched.
    pub async fn reap_leases(&self) -> Result<usize, Error> {
        let requeued: Vec<i64> = sqlx::query_scalar(
            "UPDATE jobs SET status = 'queued', lease_owner = NULL, lease_until = NULL,
                    run_after = now(), updated_at = now()
             WHERE status = 'running' AND lease_until IS NOT NULL AND lease_until < now()
               AND attempt < max_attempts
             RETURNING id",
        )
        .fetch_all(&self.pool)
        .await?;
        for id in &requeued {
            tracing::warn!(job_id = id, "lease expired; job requeued");
        }
        let exhausted: Vec<(i64, i32)> = sqlx::query_as(
            "SELECT id, attempt FROM jobs
             WHERE status = 'running' AND lease_until IS NOT NULL AND lease_until < now()
               AND attempt >= max_attempts",
        )
        .fetch_all(&self.pool)
        .await?;
        for (id, attempt) in &exhausted {
            let error =
                format!("lease expired on attempt {attempt}; the executor stopped responding");
            ingest_job_result(&self.pool, *id, *attempt, JobResult::Failed { error }).await?;
        }
        Ok(requeued.len() + exhausted.len())
    }

    /// Fallback for missing callbacks: ask the sandbox about runs past their deadline.
    ///
    /// * terminal run: report it (a succeeded run reports `pushed: false`, because only the
    ///   callback knows the head commit);
    /// * run still going: cancel it and report a failure;
    /// * unknown run: report a failure;
    /// * sandbox unreachable: try again next sweep, and give up after `poll_give_up`.
    pub async fn poll_deadlines(&self) -> Result<usize, Error> {
        let due: Vec<(i64, i32, Option<String>, Value, bool)> = sqlx::query_as(
            r"SELECT id, attempt, project_id, input,
                     deadline + make_interval(secs => $1) < now() AS give_up
              FROM jobs
              WHERE status = 'running' AND kind LIKE 'agent\_%' AND deadline IS NOT NULL
                AND deadline <= now()
              ORDER BY deadline LIMIT 50",
        )
        .bind(self.cfg.poll_give_up.as_secs_f64())
        .fetch_all(&self.pool)
        .await?;
        let mut reported = 0;
        for (id, attempt, project, input, give_up) in due {
            let result = match self.ask_sandbox(id, project.as_deref(), &input).await {
                Ok(r) => r,
                Err(e) if give_up => JobResult::Failed {
                    error: format!(
                        "the agent run passed its deadline and the sandbox is unreachable: {}",
                        e.message
                    ),
                },
                Err(e) => {
                    tracing::warn!(job_id = id, error = %e.message, "deadline poll failed; will retry");
                    continue;
                }
            };
            if ingest_job_result(&self.pool, id, attempt, result)
                .await?
                .is_some()
            {
                reported += 1;
            }
        }
        Ok(reported)
    }

    async fn ask_sandbox(
        &self,
        job_id: i64,
        project: Option<&str>,
        input: &Value,
    ) -> Result<JobResult, JobError> {
        let project = project.ok_or_else(|| JobError::permanent("job has no project"))?;
        let session = self.dial(project).await?;
        let rid = run_id(job_id);
        let status = tokio::time::timeout(self.cfg.sandbox_timeout, session.run_status(&rid)).await;
        let result = match status {
            Err(_) => Err(JobError::transient("run_status timed out")),
            Ok(Err(SessionError::Remote { code, .. })) if code == "unknown_run" => {
                Ok(JobResult::Failed {
                    error: "the sandbox does not know this run".into(),
                })
            }
            Ok(Err(e)) => Err(JobError::transient(format!("run_status: {e}"))),
            Ok(Ok(info)) => Ok(match info.state {
                RunPhase::Succeeded => {
                    let branch = serde_json::from_value::<AgentJobInput>(input.clone())
                        .map(|i| i.branch)
                        .unwrap_or_default();
                    JobResult::Succeeded {
                        output: json!({
                            "run_id": rid, "outcome": "succeeded", "exit_code": info.exit_code,
                            "summary": info.summary.unwrap_or_default(), "branch": branch,
                            "head_sha": "", "pushed": false,
                        }),
                    }
                }
                RunPhase::Failed | RunPhase::Cancelled => JobResult::Failed {
                    error: format!(
                        "the agent run ended as {:?}: {}",
                        info.state,
                        info.summary.unwrap_or_default()
                    ),
                },
                RunPhase::Queued | RunPhase::Running => {
                    let _ = session.run_cancel(&rid);
                    JobResult::Failed {
                        error: "the agent run exceeded its deadline".into(),
                    }
                }
            }),
        };
        session.close().await;
        result
    }

    /// Cancel sandbox runs of jobs that a transition cancelled after the run was accepted
    /// (`deadline` is the marker; it is cleared once the cancel was sent).
    pub async fn cancel_runs(&self) -> Result<usize, Error> {
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
            r"SELECT id, project_id FROM jobs
              WHERE status = 'cancelled' AND kind LIKE 'agent\_%' AND deadline IS NOT NULL
                AND updated_at > now() - interval '1 day'
              ORDER BY id LIMIT 50",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut sent = 0;
        for (id, project) in rows {
            let Some(project) = project else { continue };
            match self.dial(&project).await {
                Ok(session) => {
                    let ok = session.run_cancel(&run_id(id)).is_ok();
                    session.close().await;
                    if ok {
                        sqlx::query("UPDATE jobs SET deadline = NULL WHERE id = $1")
                            .bind(id)
                            .execute(&self.pool)
                            .await?;
                        sent += 1;
                    }
                }
                Err(e) => {
                    tracing::warn!(job_id = id, error = %e.message, "run cancel failed; will retry")
                }
            }
        }
        Ok(sent)
    }
}
