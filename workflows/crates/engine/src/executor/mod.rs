//! Executors: claim queued jobs and run them.
//!
//! * **Connector jobs** (`set_progress`, `post_comment`, `open_change_request`) call the
//!   [`Connector`](enzarb_connector::Connector) for the job's repo, using `jobs.idempotency_key`
//!   as the provider idempotency key, and report through [`ingest_job_result`].
//! * **Agent jobs** (`agent_*`) prepare a run (thread, clone credential, prompt), send
//!   `run_start` to the Project's sandbox-init over `enzarb-proto`, and then *release the
//!   lease*. The job stays `running` with `jobs.deadline` set; sandbox-init reports the end
//!   of the run by POSTing `RunFinished` to the callback [`Router`](axum::Router)
//!   ([`Executor::callback_router`]).
//!
//! # Claiming
//! A claim is one statement under a global advisory lock, so the limits below hold across
//! replicas:
//! `UPDATE jobs SET status='running', attempt=attempt+1, lease_owner, lease_until` on the
//! first `queued` job with `run_after <= now()` in `priority DESC, run_after, id` order,
//! skipping jobs that are not eligible:
//! * at most [`effective_agent_limit`] agent jobs *running* per Project (`Project.spec.maxRuns`,
//!   default [`ExecutorConfig::default_agent_per_project`], clamped 1..=16 and capped by the
//!   service ceiling [`ExecutorConfig::max_agent_per_project`]); a run waiting for its callback
//!   still counts. An agent job holds a worker only while it is being accepted (dial,
//!   `prepare_run`, `run_start`); the run itself continues via the callback. `workers` must
//!   cover accepting agents plus the connector jobs, not the sum of the limits;
//! * at most one running connector job per work item (so progress markers and comments of an
//!   item are applied one at a time, in priority order);
//! * a `set_progress` `Working` marker waits until the agent job queued by the same event has
//!   been accepted by the sandbox (`deadline` set), and is dropped if that job already ended;
//! * agent jobs of a Project with a pending sandbox change (`status.pendingChange`) are not
//!   started until it clears, see [`sandbox_change`];
//! * agent jobs of a Project with an active `project_holds` row (a session limit was hit, see
//!   below) wait for `blocked_until`. Other jobs of that Project are not held.
//!
//! # Session limits
//! An agent run that fails because Claude hit its session limit is not a failure: the job goes
//! back to `queued` with `run_after` just after the reset (exponential back-off from 5 to 60
//! minutes when the reset time is unknown), its attempt is refunded (`max_attempts + 1`), the
//! Project is held until then, and `jobs.waiting_for` / `jobs.resumes_at` tell the UI. Each
//! wait starts a new sandbox run (`job-<id>-<waits>`). After 7 days of waiting the job fails.
//! See [`session_limit`].
//!
//! # Sandbox boot waits
//! A job whose Project has no sandbox yet ([`SandboxError::NotReady`]) is not retried with the
//! back-off below: it goes back to `queued` every [`ExecutorConfig::sandbox_boot_poll`] with its
//! attempt refunded (`max_attempts + 1`) and `waiting_for = 'sandbox'`, and fails once
//! [`ExecutorConfig::sandbox_boot_timeout`] has passed since the first miss. See
//! [`sandbox_wait`].
//!
//! # Sandbox restarts
//! An agent run that ends as cancelled (or unknown to sandbox-init) while its job is still
//! `running` was lost to a sandbox restart. The job is re-queued with backoff and its attempt
//! refunded, up to [`ExecutorConfig::restart_retry_cap`] times, as a new sandbox run
//! (`job-<id>-<waits + restart retries>`). See [`restart`].
//!
//! # Leases, retries, deadlines
//! * A claim holds a lease for [`ExecutorConfig::lease`]. The sweeper requeues `running`
//!   jobs whose lease expired (executor crashed); when `attempt >= max_attempts` it reports a
//!   failure instead. Connector calls are idempotent by key and `run_start` is idempotent by
//!   `run_id = job-<id>`, so redoing a half-done attempt is safe.
//! * A failed attempt that may succeed later (transient connector error, rate limit,
//!   sandbox unreachable) goes back to `queued` with
//!   `run_after = now() + backoff` (exponential, honoring `retry_after`). Permanent errors
//!   and exhausted attempts are reported as `JobResult::Failed`, which becomes `JobFailed`.
//! * An accepted agent run releases its lease and sets
//!   `deadline = now() + timeout_secs + deadline_grace`. The ticker emits `TimerFired {job_id}`
//!   at the deadline; the sweeper also asks the sandbox (`run_status`) for runs past their
//!   deadline and reports the outcome (or cancels the run and reports a failure).
//! * Jobs cancelled by a transition (`CancelJobs`) have their sandbox run cancelled by the
//!   sweeper (`run_cancel`).
//!
//! Late results are harmless: [`ingest_job_result`] drops results for attempts that are no
//! longer current.

mod agent;
mod callback;
pub mod claude_credential;
mod connectors;
mod jobs;
mod restart;
mod retarget;
mod sandbox;
mod sandbox_change;
mod sandbox_wait;
mod session_limit;
mod sweep;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sqlx::{FromRow, PgPool};
use tokio::sync::{Notify, watch};

use crate::error::Error;
use crate::machine::JobKind;
use crate::results::{JobResult, ingest_job_result};

pub use callback::{callback_token, router as callback_router};
pub use claude_credential::{ClaudeKeyring, DecryptError, KeyringError};
pub use connectors::ConnectorRegistry;
pub use sandbox::{
    KubeSandboxResolver, RunningSandbox, SandboxError, SandboxResolver, SandboxTarget,
    StaticSandboxResolver, max_runs, project_owner, project_repos, sandbox_name, secret_location,
};
pub use sandbox_change::{ProjectChangeState, change_state};

/// A credential whose `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(v: impl Into<String>) -> Self {
        Self(v.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Value of `jobs.lease_owner`.
    pub owner: String,
    /// Concurrent job workers. An agent job holds one only while it is accepted by the
    /// sandbox, not while the run is active.
    pub workers: usize,
    /// Service ceiling for agent jobs `running` at once per Project (one sandbox, one PVC);
    /// `Project.spec.maxRuns` is capped by it. Env `ENZARB_EXECUTOR_MAX_AGENT_PER_PROJECT`.
    pub max_agent_per_project: i64,
    /// Limit for Projects without `spec.maxRuns` (the sandbox default). Env
    /// `ENZARB_EXECUTOR_DEFAULT_AGENT_PER_PROJECT`.
    pub default_agent_per_project: i64,
    /// How long a resolved `maxRuns` is reused before the Project is read again.
    pub agent_limit_ttl: Duration,
    /// A `Project.status.pendingChange` hold older than this stops holding agent jobs (the
    /// operator never cleared it); a warning is logged. Env
    /// `ENZARB_EXECUTOR_SANDBOX_CHANGE_HOLD_CEILING_SECS`.
    pub sandbox_change_hold_ceiling: Duration,
    pub lease: Duration,
    /// Fallback poll when no `work_item_changed` notification arrives.
    pub poll_interval: Duration,
    pub sweep_interval: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    /// Added to `timeout_secs` for `jobs.deadline`, so the sandbox's own timeout fires first.
    pub deadline_grace: Duration,
    /// After `deadline` the sweeper retries unreachable sandboxes for this long, then fails
    /// the job.
    pub poll_give_up: Duration,
    /// Timeout for connecting to and talking to a sandbox.
    pub sandbox_timeout: Duration,
    /// How long a job waits for a Project's sandbox to exist (first boot, image pull) before
    /// failing. Does not count against `max_attempts`.
    pub sandbox_boot_timeout: Duration,
    /// Delay between tries while waiting for the sandbox.
    pub sandbox_boot_poll: Duration,
    /// `ENZARB_EXECUTOR_URL`: base of the callback URL, e.g.
    /// `http://enzarb-workflows-executor.enzarb.svc:8081`.
    pub callback_base_url: String,
    /// Key for the per-run callback tokens (`HMAC-SHA256(secret, run_id)`). All replicas must
    /// share it.
    pub callback_secret: Vec<u8>,
    /// Sandbox-restart retries per agent job before it fails.
    pub restart_retry_cap: i32,
    /// Optional long-lived token with `read:packages` (`ENZARB_NPM_TOKEN[_FILE]`), sent in
    /// `RunStart.npm_token` so the agent's `npm ci` can read private npm packages (GitHub App
    /// installation tokens cannot). Never stored in `jobs.input`, never logged.
    pub npm_token: Option<Secret>,
    /// Keyring for the per-user Claude token (`ENZARB_CLAUDE_KEYRING_DIR`). Every agent run
    /// reads the Project owner's row and sends the decrypted value as `RunStart.claude_token`.
    /// `None` (not mounted) fails every agent run with `credential_undecryptable`: there is no
    /// unauthenticated or shared-token fallback.
    pub claude_keyring: Option<ClaudeKeyring>,
}

impl ExecutorConfig {
    pub fn new(callback_base_url: impl Into<String>, callback_secret: impl Into<Vec<u8>>) -> Self {
        let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "executor".into());
        Self {
            owner: format!("{host}-{}", std::process::id()),
            workers: 24,
            max_agent_per_project: MAX_AGENT_LIMIT,
            default_agent_per_project: DEFAULT_AGENT_LIMIT,
            agent_limit_ttl: Duration::from_secs(15),
            sandbox_change_hold_ceiling: Duration::from_secs(3 * 3600),
            lease: Duration::from_secs(120),
            poll_interval: Duration::from_secs(2),
            sweep_interval: Duration::from_secs(30),
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(300),
            deadline_grace: Duration::from_secs(120),
            poll_give_up: Duration::from_secs(600),
            sandbox_timeout: Duration::from_secs(30),
            sandbox_boot_timeout: Duration::from_secs(300),
            sandbox_boot_poll: Duration::from_secs(10),
            callback_base_url: callback_base_url.into().trim_end_matches('/').to_string(),
            callback_secret: callback_secret.into(),
            restart_retry_cap: 3,
            npm_token: None,
            claude_keyring: None,
        }
    }

    /// Apply `ENZARB_EXECUTOR_WORKERS`, `ENZARB_EXECUTOR_MAX_AGENT_PER_PROJECT` and
    /// `ENZARB_EXECUTOR_DEFAULT_AGENT_PER_PROJECT`. Unset or invalid values keep the default.
    pub fn apply_env(&mut self, get: &dyn Fn(&str) -> Option<String>) {
        let num = |k: &str| {
            get(k)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|n| *n > 0)
        };
        if let Some(n) = num("ENZARB_EXECUTOR_SANDBOX_CHANGE_HOLD_CEILING_SECS") {
            self.sandbox_change_hold_ceiling = Duration::from_secs(n as u64);
        }
        if let Some(n) = num("ENZARB_EXECUTOR_WORKERS") {
            self.workers = n as usize;
        }
        if let Some(n) = num("ENZARB_EXECUTOR_MAX_AGENT_PER_PROJECT") {
            self.max_agent_per_project = n;
        }
        if let Some(n) = num("ENZARB_EXECUTOR_DEFAULT_AGENT_PER_PROJECT") {
            self.default_agent_per_project = n;
        }
    }

    /// Delay before attempt `attempt + 1` after `attempt` failed.
    pub fn backoff(&self, attempt: i32) -> Duration {
        let exp = u32::try_from(attempt.saturating_sub(1))
            .unwrap_or(0)
            .min(16);
        self.backoff_base
            .saturating_mul(2u32.saturating_pow(exp))
            .min(self.backoff_max)
    }
}

/// Hard upper bound and default service ceiling of agent jobs per Project.
pub const MAX_AGENT_LIMIT: i64 = 16;
/// Limit of a Project without `spec.maxRuns` (the sandbox default).
pub const DEFAULT_AGENT_LIMIT: i64 = 2;

/// `min(clamp(max_runs or default, 1, 16), clamp(ceiling, 1, 16))`.
pub fn effective_agent_limit(max_runs: Option<i64>, default: i64, ceiling: i64) -> i64 {
    max_runs
        .unwrap_or(default)
        .clamp(1, MAX_AGENT_LIMIT)
        .min(ceiling.clamp(1, MAX_AGENT_LIMIT))
}

/// A job this executor has claimed.
#[derive(Debug, Clone, FromRow)]
pub(crate) struct Claimed {
    pub id: i64,
    pub work_item_id: i64,
    pub project_id: Option<String>,
    pub kind: String,
    pub attempt: i32,
    pub max_attempts: i32,
    pub limit_wait_count: i32,
    pub restart_retries: i32,
    pub idempotency_key: String,
    pub input: Value,
    pub work_item_key: String,
}

/// Why an attempt failed.
#[derive(Debug)]
pub(crate) struct JobError {
    pub retryable: bool,
    pub retry_after: Option<Duration>,
    pub message: String,
    /// The Project's sandbox does not exist yet: waits on its own budget, see [`sandbox_wait`].
    pub not_ready: bool,
    /// Waiting for a lock another job holds: re-queued after `retry_after` without spending
    /// the attempt (`max_attempts + 1`), see [`Executor::requeue_refunded`].
    pub contended: bool,
}

impl JobError {
    pub fn permanent(m: impl Into<String>) -> Self {
        Self {
            retryable: false,
            retry_after: None,
            message: m.into(),
            not_ready: false,
            contended: false,
        }
    }

    pub fn transient(m: impl Into<String>) -> Self {
        Self {
            retryable: true,
            retry_after: None,
            message: m.into(),
            not_ready: false,
            contended: false,
        }
    }

    /// A lock is busy: try again in `delay`, free of charge.
    pub fn contended(m: impl Into<String>, delay: Duration) -> Self {
        Self {
            retry_after: Some(delay),
            contended: true,
            ..Self::transient(m)
        }
    }

    pub fn sandbox_not_ready(m: impl Into<String>) -> Self {
        Self {
            not_ready: true,
            ..Self::transient(m)
        }
    }
}

impl From<enzarb_connector::Error> for JobError {
    fn from(e: enzarb_connector::Error) -> Self {
        let retry_after = match &e {
            enzarb_connector::Error::RateLimited { retry_after } => *retry_after,
            _ => None,
        };
        Self {
            retryable: e.is_retryable(),
            retry_after,
            message: e.to_string(),
            not_ready: false,
            contended: false,
        }
    }
}

/// What happened to a claimed job.
pub(crate) enum Done {
    /// Finished; report this output.
    Output(Value),
    /// Agent run accepted; the callback reports the end.
    Accepted,
}

pub struct Executor {
    pub(crate) pool: PgPool,
    pub(crate) connectors: ConnectorRegistry,
    pub(crate) sandboxes: Arc<dyn SandboxResolver>,
    pub(crate) cfg: ExecutorConfig,
    /// project id -> (resolved `maxRuns`, fetched at).
    change_warned: std::sync::Mutex<std::collections::HashSet<String>>,
    limits: std::sync::Mutex<std::collections::HashMap<String, (Option<i64>, std::time::Instant)>>,
}

impl Executor {
    pub fn new(
        pool: PgPool,
        connectors: ConnectorRegistry,
        sandboxes: Arc<dyn SandboxResolver>,
        cfg: ExecutorConfig,
    ) -> Self {
        Self {
            pool,
            connectors,
            sandboxes,
            cfg,
            limits: Default::default(),
            change_warned: Default::default(),
        }
    }

    /// Effective agent limits for Projects with queued agent jobs. A lookup failure keeps the
    /// last known value; claiming never fails because of Kubernetes.
    async fn agent_limits(&self) -> (Vec<String>, Vec<i32>) {
        let projects: Vec<String> = sqlx::query_scalar(
            r"SELECT DISTINCT project_id FROM jobs
              WHERE status = 'queued' AND kind LIKE 'agent\_%' AND project_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        let (def, ceil) = (
            self.cfg.default_agent_per_project,
            self.cfg.max_agent_per_project,
        );
        let mut ids = Vec::new();
        let mut lims = Vec::new();
        for p in projects {
            let cached = self.limits.lock().expect("limits lock").get(&p).copied();
            let fresh = cached.is_some_and(|(_, at)| at.elapsed() < self.cfg.agent_limit_ttl);
            let max_runs = if fresh {
                cached.and_then(|c| c.0)
            } else {
                match self.sandboxes.project_max_runs(&p).await {
                    Ok(v) => {
                        self.limits
                            .lock()
                            .expect("limits lock")
                            .insert(p.clone(), (v, std::time::Instant::now()));
                        v
                    }
                    Err(e) => {
                        tracing::debug!(project = %p, error = %e, "maxRuns lookup failed; using last known");
                        cached.and_then(|c| c.0)
                    }
                }
            };
            ids.push(p);
            lims.push(effective_agent_limit(max_runs, def, ceil) as i32);
        }
        (ids, lims)
    }

    pub fn config(&self) -> &ExecutorConfig {
        &self.cfg
    }

    /// Router for sandbox-init's run-finished callback: `POST /v1/runs/{run_id}/finished`
    /// (bearer = [`callback_token`]) and `GET /healthz`. Mount it on `:8081`.
    pub fn callback_router(self: &Arc<Self>) -> axum::Router {
        callback::router(self.clone())
    }

    /// Claim and run one job. Returns false when nothing was claimable.
    pub async fn run_once(&self) -> Result<bool, Error> {
        let Some(job) = self.claim().await? else {
            return Ok(false);
        };
        self.execute(job).await;
        Ok(true)
    }

    /// Run [`Executor::run_once`] until the queue is empty. Returns how many jobs ran.
    pub async fn drain(&self) -> usize {
        let mut n = 0;
        loop {
            match self.run_once().await {
                Ok(true) => n += 1,
                Ok(false) => return n,
                Err(e) => {
                    tracing::warn!(error = %e, "executor: claim failed");
                    return n;
                }
            }
        }
    }

    /// Run `cfg.workers` workers and the sweeper until `shutdown` flips to true. Workers wake
    /// on `NOTIFY work_item_changed` and otherwise poll every `cfg.poll_interval`; in-flight
    /// jobs finish before this returns.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) -> Result<(), Error> {
        let wake = Arc::new(Notify::new());
        let mut listener = sqlx::postgres::PgListener::connect_with(&self.pool).await?;
        listener.listen("work_item_changed").await?;

        let mut tasks = tokio::task::JoinSet::new();
        {
            let wake = wake.clone();
            let mut shutdown = shutdown.clone();
            tasks.spawn(async move {
                loop {
                    tokio::select! {
                        r = listener.recv() => match r {
                            Ok(_) => wake.notify_waiters(),
                            Err(e) => {
                                tracing::warn!(error = %e, "executor: listener error");
                                tokio::time::sleep(Duration::from_secs(1)).await;
                            }
                        },
                        _ = shutdown.changed() => {}
                    }
                    if *shutdown.borrow() {
                        return;
                    }
                }
            });
        }
        for _ in 0..self.cfg.workers.max(1) {
            let me = self.clone();
            let wake = wake.clone();
            let mut shutdown = shutdown.clone();
            tasks.spawn(async move {
                while !*shutdown.borrow() {
                    match me.run_once().await {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(e) => tracing::warn!(error = %e, "executor: claim failed"),
                    }
                    tokio::select! {
                        _ = wake.notified() => {}
                        _ = tokio::time::sleep(me.cfg.poll_interval) => {}
                        _ = shutdown.changed() => {}
                    }
                }
            });
        }
        {
            let me = self.clone();
            let mut shutdown = shutdown.clone();
            tasks.spawn(async move {
                while !*shutdown.borrow() {
                    me.sweep().await;
                    tokio::select! {
                        _ = tokio::time::sleep(me.cfg.sweep_interval) => {}
                        _ = shutdown.changed() => {}
                    }
                }
            });
        }
        while tasks.join_next().await.is_some() {}
        Ok(())
    }

    /// Claim the next eligible job (see the module docs).
    pub(crate) async fn claim(&self) -> Result<Option<Claimed>, Error> {
        let (limit_ids, limits) = self.agent_limits().await;
        let default_limit = effective_agent_limit(
            None,
            self.cfg.default_agent_per_project,
            self.cfg.max_agent_per_project,
        );
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(7426001)")
            .execute(&mut *tx)
            .await?;
        let job: Option<Claimed> = sqlx::query_as(
            r"WITH picked AS (
                SELECT j.id FROM jobs j
                WHERE j.status = 'queued' AND j.run_after <= now()
                  AND (j.kind NOT LIKE 'agent\_%' OR j.project_id IS NULL
                       OR (SELECT count(*) FROM jobs r
                           WHERE r.status = 'running' AND r.project_id = j.project_id
                             AND r.kind LIKE 'agent\_%')
                           < COALESCE((SELECT l.lim FROM unnest($2::text[], $4::int[]) AS l(pid, lim)
                                       WHERE l.pid = j.project_id), $5))
                  AND (j.kind NOT LIKE 'agent\_%' OR j.project_id IS NULL
                       OR NOT EXISTS (SELECT 1 FROM project_holds h
                                      WHERE h.project_id = j.project_id
                                        AND h.blocked_until > now()))
                  AND (j.kind NOT LIKE 'agent\_%' OR j.project_id IS NULL
                       OR NOT EXISTS (SELECT 1 FROM project_sandbox_changes c
                                      WHERE c.project_id = j.project_id
                                        AND c.first_seen_at > now() - make_interval(secs => $6)))
                  AND (j.kind LIKE 'agent\_%'
                       OR NOT EXISTS (SELECT 1 FROM jobs r
                                      WHERE r.status = 'running' AND r.work_item_id = j.work_item_id
                                        AND r.kind NOT LIKE 'agent\_%'))
                  AND NOT (j.kind = 'set_progress' AND j.input->>'progress' = 'Working'
                           AND EXISTS (SELECT 1 FROM jobs a
                                       WHERE a.work_item_id = j.work_item_id
                                         AND a.kind LIKE 'agent\_%'
                                         AND starts_with(a.idempotency_key,
                                                 regexp_replace(j.idempotency_key, '[^:]*$', ''))
                                         AND (a.status = 'queued'
                                              OR (a.status = 'running' AND a.deadline IS NULL))))
                ORDER BY j.priority DESC, j.run_after, j.id
                LIMIT 1 FOR UPDATE OF j SKIP LOCKED)
              UPDATE jobs SET status = 'running', attempt = attempt + 1, lease_owner = $1,
                     lease_until = now() + make_interval(secs => $3), updated_at = now(),
                     waiting_for = NULL, resumes_at = NULL
              FROM picked WHERE jobs.id = picked.id
              RETURNING jobs.id, jobs.work_item_id, jobs.project_id, jobs.kind, jobs.attempt,
                        jobs.max_attempts, jobs.limit_wait_count, jobs.restart_retries,
                        jobs.idempotency_key, jobs.input,
                        (SELECT key FROM work_items w WHERE w.id = jobs.work_item_id) AS work_item_key",
        )
        .bind(&self.cfg.owner)
        .bind(&limit_ids)
        .bind(self.cfg.lease.as_secs_f64())
        .bind(&limits)
        .bind(default_limit as i32)
        .bind(self.cfg.sandbox_change_hold_ceiling.as_secs_f64())
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(job)
    }

    /// Run a claimed job and record what happened. Never fails: infrastructure errors are
    /// logged and left to lease expiry.
    pub(crate) async fn execute(&self, job: Claimed) {
        let outcome = match serde_json::from_value::<JobKind>(Value::String(job.kind.clone())) {
            Err(_) => Err(JobError::permanent(format!(
                "unknown job kind {:?}",
                job.kind
            ))),
            Ok(JobKind::RetargetChangeRequest) => self.start_retarget(&job).await,
            Ok(kind) if kind.is_agent() => self.start_agent(&job, kind).await,
            Ok(kind) => self.run_connector_job(&job, kind).await,
        };
        if let Err(e) = self.settle(&job, outcome).await {
            tracing::warn!(job_id = job.id, error = %e, "executor: could not record job outcome; lease will expire");
        }
    }

    async fn settle(&self, job: &Claimed, outcome: Result<Done, JobError>) -> Result<(), Error> {
        match outcome {
            Ok(Done::Accepted) => Ok(()),
            Ok(Done::Output(output)) => ingest_job_result(
                &self.pool,
                job.id,
                job.attempt,
                JobResult::Succeeded { output },
            )
            .await
            .map(|_| ()),
            Err(e) if e.not_ready => self.sandbox_not_ready_outcome(job, e).await,
            Err(e) if e.contended => {
                let delay = e.retry_after.unwrap_or_default();
                tracing::info!(job_id = job.id, ?delay, error = %e.message, "job waits for a lock");
                self.requeue_refunded(job, delay).await
            }
            Err(e) if e.retryable && job.attempt < job.max_attempts => {
                let delay = self
                    .cfg
                    .backoff(job.attempt)
                    .max(e.retry_after.unwrap_or_default());
                tracing::info!(job_id = job.id, attempt = job.attempt, ?delay, error = %e.message, "job attempt failed; retrying");
                self.requeue(job, delay).await
            }
            Err(e) => {
                let error = if e.retryable {
                    format!("{} (after {} attempts)", e.message, job.attempt)
                } else {
                    e.message
                };
                tracing::warn!(job_id = job.id, %error, "job failed");
                ingest_job_result(&self.pool, job.id, job.attempt, JobResult::Failed { error })
                    .await
                    .map(|_| ())
            }
        }
    }

    /// Back to `queued` after `delay` with the attempt refunded, so waiting for a lock never
    /// exhausts `max_attempts`; if this attempt still owns the job.
    async fn requeue_refunded(&self, job: &Claimed, delay: Duration) -> Result<(), Error> {
        sqlx::query(
            "UPDATE jobs SET status = 'queued', lease_owner = NULL, lease_until = NULL,
                    max_attempts = max_attempts + 1,
                    run_after = now() + make_interval(secs => $3), updated_at = now()
             WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job.id)
        .bind(job.attempt)
        .bind(delay.as_secs_f64())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Back to `queued` after `delay`, if this attempt still owns the job.
    async fn requeue(&self, job: &Claimed, delay: Duration) -> Result<(), Error> {
        sqlx::query(
            "UPDATE jobs SET status = 'queued', lease_owner = NULL, lease_until = NULL,
                    run_after = now() + make_interval(secs => $3), updated_at = now()
             WHERE id = $1 AND status = 'running' AND attempt = $2",
        )
        .bind(job.id)
        .bind(job.attempt)
        .bind(delay.as_secs_f64())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod limit_tests {
    use super::effective_agent_limit as eff;

    #[test]
    fn effective_agent_limit_clamps_defaults_and_caps() {
        assert_eq!(eff(Some(0), 2, 16), 1);
        assert_eq!(eff(Some(100), 2, 16), 16);
        assert_eq!(eff(None, 2, 16), 2);
        assert_eq!(eff(Some(8), 2, 4), 4);
        assert_eq!(eff(None, 2, 1), 1);
        assert_eq!(eff(Some(5), 2, 100), 5);
    }
}
