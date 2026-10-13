//! Waiting for a Project's sandbox to boot.
//!
//! A freshly created Project has no sandbox until its image is pulled, which takes longer than
//! the generic retry budget (5 attempts, about 75 s). A job whose dial fails with
//! [`SandboxError::NotReady`](super::SandboxError::NotReady) is therefore not retried like other
//! transient errors: [`Executor::sandbox_not_ready_outcome`] re-queues it every
//! [`ExecutorConfig::sandbox_boot_poll`](super::ExecutorConfig::sandbox_boot_poll) without
//! spending `max_attempts`, until [`ExecutorConfig::sandbox_boot_timeout`] has passed since the
//! first miss (`jobs.sandbox_wait_started_at`, cleared when the run is accepted).

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};

use super::{Claimed, Executor, JobError};
use crate::error::Error;
use crate::results::{JobResult, ingest_job_result};

/// `jobs.waiting_for` while a job waits for its sandbox.
pub(crate) const WAITING_FOR: &str = "sandbox";

/// Has the wait that began at `started` (`None`: this is the first miss) used up `timeout`?
pub(crate) fn budget_spent(
    started: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    timeout: Duration,
) -> bool {
    let Some(started) = started else {
        return false;
    };
    let timeout = TimeDelta::from_std(timeout).unwrap_or(TimeDelta::MAX);
    now - started > timeout
}

impl Executor {
    /// An attempt failed because the sandbox is not ready: wait (refunding the attempt) or, when
    /// the boot budget is spent, fail the job.
    pub(crate) async fn sandbox_not_ready_outcome(
        &self,
        job: &Claimed,
        e: JobError,
    ) -> Result<(), Error> {
        let now = Utc::now();
        let row: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
            "SELECT sandbox_wait_started_at FROM jobs
             WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job.id)
        .bind(job.attempt)
        .fetch_optional(&self.pool)
        .await?;
        // A stale attempt: nothing to do, and ingesting a result would drop it anyway.
        let Some((started,)) = row else {
            return Ok(());
        };

        let timeout = self.cfg.sandbox_boot_timeout;
        if budget_spent(started, now, timeout) {
            let error = format!(
                "{} (sandbox not ready after {}m)",
                e.message,
                (timeout.as_secs() / 60).max(1)
            );
            tracing::warn!(job_id = job.id, %error, "job failed");
            return ingest_job_result(&self.pool, job.id, job.attempt, JobResult::Failed { error })
                .await
                .map(|_| ());
        }

        let poll = self.cfg.sandbox_boot_poll;
        tracing::info!(job_id = job.id, ?poll, error = %e.message, "sandbox not ready; waiting");
        sqlx::query(
            "UPDATE jobs SET status = 'queued', lease_owner = NULL, lease_until = NULL,
                    deadline = NULL, max_attempts = max_attempts + 1, -- bounded by boot_timeout / boot_poll
                    run_after = now() + make_interval(secs => $3),
                    resumes_at = now() + make_interval(secs => $3),
                    sandbox_wait_started_at = COALESCE(sandbox_wait_started_at, now()),
                    waiting_for = $4, updated_at = now()
             WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job.id)
        .bind(job.attempt)
        .bind(poll.as_secs_f64())
        .bind(WAITING_FOR)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_measured_from_the_first_miss() {
        let now = Utc::now();
        let t = Duration::from_secs(300);
        assert!(!budget_spent(None, now, t));
        assert!(!budget_spent(Some(now - TimeDelta::seconds(299)), now, t));
        assert!(!budget_spent(Some(now - TimeDelta::seconds(300)), now, t));
        assert!(budget_spent(Some(now - TimeDelta::seconds(301)), now, t));
    }

    #[test]
    fn only_not_ready_is_a_sandbox_wait() {
        assert!(JobError::sandbox_not_ready("x").not_ready);
        assert!(JobError::sandbox_not_ready("x").retryable);
        assert!(!JobError::transient("x").not_ready);
        assert!(!JobError::permanent("x").not_ready);
    }
}
