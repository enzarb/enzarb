//! Ticker: turns due timers into `TimerFired` events.
//!
//! * `work_items.wake_at <= now()` becomes `TimerFired {}`, dedupe key
//!   `timer:<work_item_id>:<wake_at>` (RFC 3339, microseconds, UTC).
//! * A `running` job past `jobs.deadline` becomes `TimerFired {job_id}`, dedupe key
//!   `timer:<work_item_id>:job:<job_id>:<attempt>`.
//!
//! Events go in through `workflows_api.ingest_event`, so a timer is emitted once per
//! (item, wake_at) however many tickers run or how often they tick.

use std::time::Duration;

use serde_json::json;
use sqlx::PgPool;
use tokio::sync::watch;

use crate::error::Error;
use crate::grants;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TickStats {
    /// `TimerFired` events created for `wake_at`.
    pub wakes: usize,
    /// `TimerFired {job_id}` events created for job deadlines.
    pub deadlines: usize,
}

#[derive(Clone)]
pub struct Ticker {
    pool: PgPool,
    interval: Duration,
}

const BATCH: i64 = 500;

impl Ticker {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            interval: Duration::from_secs(5),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// One sweep.
    pub async fn tick(&self) -> Result<TickStats, Error> {
        let mut stats = TickStats::default();

        let wakes: Vec<(String, String)> = sqlx::query_as(
            "WITH due AS (
                 SELECT key, 'timer:' || id || ':' ||
                        to_char(wake_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS dk,
                        wake_at
                 FROM work_items WHERE wake_at IS NOT NULL AND wake_at <= now())
             SELECT key, dk FROM due
             WHERE NOT EXISTS (SELECT 1 FROM events e WHERE e.dedupe_key = due.dk)
             ORDER BY wake_at LIMIT $1",
        )
        .bind(BATCH)
        .fetch_all(&self.pool)
        .await?;
        for (key, dk) in wakes {
            if self.emit(&dk, &key, json!({})).await? {
                stats.wakes += 1;
            }
        }

        let deadlines: Vec<(String, String, i64)> = sqlx::query_as(
            "WITH due AS (
                 SELECT w.key, j.id AS job_id, j.deadline,
                        'timer:' || w.id || ':job:' || j.id || ':' || j.attempt AS dk
                 FROM jobs j JOIN work_items w ON w.id = j.work_item_id
                 WHERE j.status = 'running' AND j.deadline IS NOT NULL AND j.deadline <= now())
             SELECT key, dk, job_id FROM due
             WHERE NOT EXISTS (SELECT 1 FROM events e WHERE e.dedupe_key = due.dk)
             ORDER BY deadline LIMIT $1",
        )
        .bind(BATCH)
        .fetch_all(&self.pool)
        .await?;
        for (key, dk, job_id) in deadlines {
            if self.emit(&dk, &key, json!({ "job_id": job_id })).await? {
                stats.deadlines += 1;
            }
        }
        Ok(stats)
    }

    async fn emit(
        &self,
        dedupe: &str,
        key: &str,
        payload: serde_json::Value,
    ) -> Result<bool, Error> {
        let id: Option<i64> =
            sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
                .bind("engine")
                .bind(dedupe)
                .bind(key)
                .bind("TimerFired")
                .bind(payload)
                .fetch_one(&self.pool)
                .await?;
        Ok(id.is_some())
    }

    /// Tick every `interval` until `shutdown` flips to true.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut warned_missing = false;
        loop {
            match grants::reconcile_realtime(&self.pool).await {
                Ok(true) => tracing::info!("granted enzarb_realtime its read access"),
                Ok(false) => {}
                Err(e) if grants::is_missing_function(&e) => {
                    if !warned_missing {
                        warned_missing = true;
                        tracing::warn!("realtime grants reconciler missing; run `migrate` first");
                    }
                }
                Err(e) => tracing::warn!(error = %e, "realtime grants reconcile failed"),
            }
            if let Err(e) = self.tick().await {
                tracing::warn!(error = %e, "ticker sweep failed");
            }
            tokio::select! {
                _ = tokio::time::sleep(self.interval) => {}
                _ = shutdown.changed() => {}
            }
            if *shutdown.borrow() {
                return;
            }
        }
    }
}
