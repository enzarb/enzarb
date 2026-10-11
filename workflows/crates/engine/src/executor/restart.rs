//! Sandbox restarts: an agent run that ends as cancelled (or vanishes from sandbox-init) while
//! its job is still `running` in the database was lost to a sandbox restart. A user or
//! transition cancel marks the job `cancelled` first, so it never gets here.
//! [`Executor::restart_outcome`] re-queues such a job with backoff, refunding the attempt, up
//! to [`super::ExecutorConfig::restart_retry_cap`] times.

use super::Executor;
use crate::error::Error;

/// What to do with a run lost to a sandbox restart.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RestartOutcome {
    /// The job was re-queued.
    Requeued,
    /// The attempt is no longer the running one; nothing was done.
    Stale,
    /// The retry cap is used up; fail with this message.
    GaveUp(String),
}

impl Executor {
    /// A running agent job's attempt ended because of a sandbox restart (`reason` says how).
    pub(crate) async fn restart_outcome(
        &self,
        job_id: i64,
        attempt: i32,
        reason: &str,
    ) -> Result<RestartOutcome, Error> {
        let cap = self.cfg.restart_retry_cap;
        let row: Option<i32> = sqlx::query_scalar(
            "SELECT restart_retries FROM jobs WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job_id)
        .bind(attempt)
        .fetch_optional(&self.pool)
        .await?;
        let Some(retries) = row else {
            return Ok(RestartOutcome::Stale);
        };
        if retries >= cap {
            return Ok(RestartOutcome::GaveUp(format!(
                "{reason} (interrupted by a sandbox restart; gave up after {cap} retries)"
            )));
        }
        let delay = self.cfg.backoff(retries + 1).as_secs_f64();
        let updated = sqlx::query(
            "UPDATE jobs SET status = 'queued', lease_owner = NULL, lease_until = NULL,
                    deadline = NULL, run_after = now() + make_interval(secs => $3),
                    max_attempts = max_attempts + 1, restart_retries = restart_retries + 1,
                    updated_at = now()
             WHERE id = $1 AND status = 'running' AND attempt = $2 AND restart_retries = $4",
        )
        .bind(job_id)
        .bind(attempt)
        .bind(delay)
        .bind(retries)
        .execute(&self.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(RestartOutcome::Stale);
        }
        tracing::warn!(
            job_id,
            retry = retries + 1,
            reason,
            "agent run lost to a sandbox restart; job requeued"
        );
        Ok(RestartOutcome::Requeued)
    }
}
