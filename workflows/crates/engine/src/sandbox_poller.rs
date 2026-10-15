//! Sandbox stats poller (ADR-0019 section 3): asks each running sandbox-init for a `stats`
//! snapshot every interval and stores it in `workflows.sandbox_stats`.
//!
//! * Only one replica polls: the holder of a Postgres session advisory lock ([`PgPollLock`]);
//!   the others retry the lock every interval. Duplicate inserts are no-ops anyway (primary
//!   key `(project, sandbox, at)`).
//! * Sandboxes are found through `Project.status` ([`SandboxResolver::running_sandboxes`]); a
//!   Project that is not ready is skipped silently.
//! * Each sandbox has its own timeout, a semaphore bounds how many are in flight, and start
//!   times are jittered per project, so one bad sandbox cannot stall the rest of the pass.
//! * An unreachable, slow or older sandbox (no `stats` feature, `unknown_message`) means *skip
//!   the sample*: nothing is written. Consecutive failures back off per project (doubling up to
//!   [`MAX_BACKOFF`]) and reset on success.
//! * The lock holder also prunes old rows, at most hourly ([`PRUNE_EVERY`]).
//!
//! The poller does no workflow work (no events, no jobs) and runs as its own task, so a stuck
//! sandbox cannot delay the ticker, reducer or executor.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use enzarb_proto::session::{Client, Events, Session};
use enzarb_proto::{ClientKind, Ctrl, SandboxStats, error_code, feature};
use futures::future::join_all;
use sqlx::PgPool;
use sqlx::pool::PoolConnection;
use tokio::sync::{Semaphore, watch};
use tokio::time::{Instant, MissedTickBehavior};

use crate::db;
use crate::executor::{RunningSandbox, SandboxResolver, SandboxTarget};

/// Accepted `ENZARB_STATS_INTERVAL` range in seconds; values outside it are clamped.
pub const MIN_INTERVAL_SECS: u64 = 15;
pub const MAX_INTERVAL_SECS: u64 = 300;

/// Longest wait before a failing project is polled again.
pub const MAX_BACKOFF: Duration = Duration::from_secs(300);

/// Minimum time between two retention prunes.
pub const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// Rows deleted per prune statement.
pub const PRUNE_BATCH: i64 = 10_000;

/// Key of the session advisory lock that elects the polling replica.
pub const POLL_LOCK_KEY: i64 = 0x656e_7a73_7461_7473;

#[derive(Debug, Clone)]
pub struct PollerConfig {
    /// Time between two passes.
    pub interval: Duration,
    /// Per-sandbox limit for the stats request and for storing the sample.
    pub timeout: Duration,
    /// Sandboxes asked at once.
    pub concurrency: usize,
    /// Rows older than this many days are pruned.
    pub retention_days: i32,
    /// Start times are spread over `[0, jitter)` by project name.
    pub jitter: Duration,
}

impl PollerConfig {
    /// Config for `interval_secs` (clamped to the accepted range) with the defaults of the rest.
    pub fn with_interval_secs(interval_secs: u64) -> Self {
        let interval =
            Duration::from_secs(interval_secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS));
        Self {
            interval,
            jitter: interval / 4,
            ..Self::default()
        }
    }
}

impl Default for PollerConfig {
    fn default() -> Self {
        let interval = Duration::from_secs(30);
        Self {
            interval,
            timeout: Duration::from_secs(5),
            concurrency: 8,
            retention_days: 30,
            jitter: interval / 4,
        }
    }
}

/// Why a sandbox gave no sample.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum StatsError {
    /// The sandbox cannot answer: no `stats` feature, or `unknown_message`/`unimplemented`.
    #[error("stats not supported by this sandbox")]
    Unsupported,
    #[error("sandbox unreachable: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait StatsClient: Send + Sync {
    /// One stats snapshot of the sandbox at `target`.
    async fn fetch(&self, target: &SandboxTarget) -> Result<SandboxStats, StatsError>;
}

/// Where samples go: [`PgSampleSink`] in production.
#[async_trait]
pub trait SampleSink: Send + Sync {
    /// Store one sample; `false` when `(project, sandbox, at)` was already recorded.
    async fn store(
        &self,
        project: &str,
        sandbox: &str,
        stats: &SandboxStats,
    ) -> Result<bool, String>;

    /// Delete rows past retention and beyond the per-sandbox cap; returns how many.
    async fn prune(&self, retention_days: i32) -> Result<u64, String>;
}

/// Elects the one replica that polls.
#[async_trait]
pub trait PollLock: Send + Sync {
    /// Whether this process holds the lock after the call (acquiring it when free).
    async fn try_acquire(&self) -> Result<bool, String>;
}

/// Counts of one pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PollStats {
    /// Samples stored (or already stored).
    pub sampled: usize,
    /// Sandboxes that cannot answer (older image).
    pub unsupported: usize,
    /// Unreachable, errored or failed to store.
    pub failed: usize,
    /// Per-sandbox timeouts.
    pub timed_out: usize,
    /// Not asked this pass because of their failure backoff.
    pub backed_off: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Sampled,
    Unsupported,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, Copy)]
struct Backoff {
    failures: u32,
    next_at: Instant,
}

/// Wait after `failures` consecutive failures: the interval doubled per failure, up to
/// [`MAX_BACKOFF`] (never below the interval itself).
pub fn backoff_delay(interval: Duration, failures: u32) -> Duration {
    let factor = 1u32.checked_shl(failures).unwrap_or(u32::MAX);
    interval
        .saturating_mul(factor)
        .min(MAX_BACKOFF.max(interval))
}

/// Stable start offset of `project` within `[0, window)`.
pub fn jitter_for(project: &str, window: Duration) -> Duration {
    let ms = window.as_millis() as u64;
    if ms == 0 {
        return Duration::ZERO;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    project.hash(&mut h);
    Duration::from_millis(h.finish() % ms)
}

pub struct SandboxPoller {
    resolver: Arc<dyn SandboxResolver>,
    client: Arc<dyn StatsClient>,
    sink: Arc<dyn SampleSink>,
    lock: Arc<dyn PollLock>,
    cfg: PollerConfig,
    backoff: Mutex<HashMap<String, Backoff>>,
    last_prune: Mutex<Option<Instant>>,
}

impl SandboxPoller {
    pub fn new(
        resolver: Arc<dyn SandboxResolver>,
        client: Arc<dyn StatsClient>,
        sink: Arc<dyn SampleSink>,
        lock: Arc<dyn PollLock>,
        cfg: PollerConfig,
    ) -> Self {
        Self {
            resolver,
            client,
            sink,
            lock,
            cfg,
            backoff: Mutex::default(),
            last_prune: Mutex::default(),
        }
    }

    /// One pass over every running sandbox that is not backing off.
    pub async fn poll_once(&self) -> Result<PollStats, crate::executor::SandboxError> {
        let all = self.resolver.running_sandboxes().await?;
        let now = Instant::now();
        let mut stats = PollStats::default();
        let due: Vec<RunningSandbox> = {
            let mut backoff = self.backoff.lock().expect("backoff lock");
            // Forget Projects that no longer have a sandbox.
            backoff.retain(|p, _| all.iter().any(|s| &s.project == p));
            all.into_iter()
                .filter(|s| match backoff.get(&s.project) {
                    Some(b) if now < b.next_at => {
                        stats.backed_off += 1;
                        false
                    }
                    _ => true,
                })
                .collect()
        };

        let permits = Arc::new(Semaphore::new(self.cfg.concurrency.max(1)));
        let results = join_all(due.iter().map(|s| {
            let permits = permits.clone();
            async move {
                tokio::time::sleep(jitter_for(&s.project, self.cfg.jitter)).await;
                let _permit = permits.acquire().await.expect("semaphore is never closed");
                (s, self.poll_sandbox(s).await)
            }
        }))
        .await;

        let now = Instant::now();
        let mut backoff = self.backoff.lock().expect("backoff lock");
        for (s, outcome) in results {
            match outcome {
                Outcome::Sampled => {
                    stats.sampled += 1;
                    backoff.remove(&s.project);
                    continue;
                }
                Outcome::Unsupported => stats.unsupported += 1,
                Outcome::Failed => stats.failed += 1,
                Outcome::TimedOut => stats.timed_out += 1,
            }
            let failures = backoff.get(&s.project).map_or(0, |b| b.failures) + 1;
            backoff.insert(
                s.project.clone(),
                Backoff {
                    failures,
                    next_at: now + backoff_delay(self.cfg.interval, failures),
                },
            );
        }
        Ok(stats)
    }

    async fn poll_sandbox(&self, s: &RunningSandbox) -> Outcome {
        let t = self.cfg.timeout;
        let stats = match tokio::time::timeout(t, self.client.fetch(&s.target)).await {
            Err(_) => {
                tracing::debug!(project = %s.project, "stats request timed out; skipped");
                return Outcome::TimedOut;
            }
            Ok(Err(StatsError::Unsupported)) => {
                tracing::debug!(project = %s.project, "sandbox does not support stats; skipped");
                return Outcome::Unsupported;
            }
            Ok(Err(e)) => {
                tracing::debug!(project = %s.project, error = %e, "stats request failed; skipped");
                return Outcome::Failed;
            }
            Ok(Ok(stats)) => stats,
        };
        match tokio::time::timeout(t, self.sink.store(&s.project, &s.sandbox, &stats)).await {
            Ok(Ok(_)) => Outcome::Sampled,
            Ok(Err(e)) => {
                tracing::warn!(project = %s.project, error = %e, "storing a sandbox sample failed");
                Outcome::Failed
            }
            Err(_) => {
                tracing::warn!(project = %s.project, "storing a sandbox sample timed out");
                Outcome::TimedOut
            }
        }
    }

    /// One scheduled round: take the lock, run a pass, prune when due.
    async fn round(&self) {
        match self.lock.try_acquire().await {
            Ok(true) => {}
            Ok(false) => {
                tracing::debug!("another replica polls sandbox stats");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "sandbox stats poll lock failed");
                return;
            }
        }
        match self.poll_once().await {
            Ok(s) => tracing::debug!(
                ok = s.sampled,
                skipped = s.unsupported + s.backed_off,
                failed = s.failed + s.timed_out,
                "sandbox stats round"
            ),
            Err(e) => tracing::warn!(error = %e, "listing sandboxes for stats failed"),
        }
        self.prune_if_due().await;
    }

    async fn prune_if_due(&self) {
        let now = Instant::now();
        {
            let mut last = self.last_prune.lock().expect("prune lock");
            if last.is_some_and(|t| now.duration_since(t) < PRUNE_EVERY) {
                return;
            }
            *last = Some(now);
        }
        match self.sink.prune(self.cfg.retention_days).await {
            Ok(n) => tracing::debug!(rows = n, "pruned sandbox stats"),
            Err(e) => tracing::warn!(error = %e, "pruning sandbox stats failed"),
        }
    }

    /// Poll every interval (the first pass at once) until `shutdown` flips. A pass that overruns
    /// the interval delays the next one instead of piling up; shutdown also ends a running pass.
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut ticks = tokio::time::interval(self.cfg.interval);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            if *shutdown.borrow() {
                return;
            }
            tokio::select! {
                _ = ticks.tick() => {}
                _ = shutdown.changed() => return,
            }
            tokio::select! {
                _ = self.round() => {}
                _ = shutdown.changed() => return,
            }
        }
    }
}

/// [`StatsClient`] over the mux websocket, authenticated like run dispatch.
#[derive(Debug, Clone, Copy, Default)]
pub struct MuxStatsClient;

#[async_trait]
impl StatsClient for MuxStatsClient {
    async fn fetch(&self, target: &SandboxTarget) -> Result<SandboxStats, StatsError> {
        let (session, mut events) =
            Client::connect(&target.endpoint, &target.token, ClientKind::Executor)
                .await
                .map_err(|e| StatsError::Unreachable(e.to_string()))?;
        let stats = request_stats(&session, &mut events).await;
        session.close().await;
        stats
    }
}

/// Ask an established session for its stats. Only sends the request when `welcome` listed the
/// `stats` feature; an old sandbox that answers `unknown_message` is also `Unsupported`.
pub async fn request_stats(
    session: &Session,
    events: &mut Events,
) -> Result<SandboxStats, StatsError> {
    use enzarb_proto::session::Event;
    if !session.has_feature(feature::STATS) {
        return Err(StatsError::Unsupported);
    }
    session
        .send_ctrl(&Ctrl::StatsRequest)
        .map_err(|e| StatsError::Unreachable(e.to_string()))?;
    while let Some(ev) = events.next().await {
        match ev {
            Event::Ctrl(Ctrl::Stats(s)) => return Ok(s),
            Event::Ctrl(Ctrl::Error { code, message, .. }) => {
                return Err(
                    if code == error_code::UNKNOWN_MESSAGE || code == error_code::UNIMPLEMENTED {
                        StatsError::Unsupported
                    } else {
                        StatsError::Other(format!("{code}: {message}"))
                    },
                );
            }
            _ => {}
        }
    }
    Err(StatsError::Unreachable("session ended".into()))
}

/// [`SampleSink`] writing through [`db::insert_sandbox_stats`].
#[derive(Clone)]
pub struct PgSampleSink {
    pool: PgPool,
}

impl PgSampleSink {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The lowest work item with a running agent job on `project`, or `None` (ADR-0019 section 3).
pub async fn attributed_work_item(
    pool: &PgPool,
    project: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT min(work_item_id) FROM workflows.jobs
         WHERE project_id = $1 AND status = 'running' AND kind LIKE 'agent\\_%'",
    )
    .bind(project)
    .fetch_one(pool)
    .await
}

#[async_trait]
impl SampleSink for PgSampleSink {
    async fn store(
        &self,
        project: &str,
        sandbox: &str,
        stats: &SandboxStats,
    ) -> Result<bool, String> {
        let item = attributed_work_item(&self.pool, project)
            .await
            .map_err(|e| e.to_string())?;
        db::insert_sandbox_stats(&self.pool, project, sandbox, item, stats)
            .await
            .map_err(|e| e.to_string())
    }

    async fn prune(&self, retention_days: i32) -> Result<u64, String> {
        let mut total = 0;
        loop {
            let n = db::prune_sandbox_stats(&self.pool, retention_days, PRUNE_BATCH)
                .await
                .map_err(|e| e.to_string())?;
            total += n;
            if n < PRUNE_BATCH as u64 {
                break;
            }
        }
        loop {
            let n = db::prune_sandbox_stats_over_cap(
                &self.pool,
                db::SANDBOX_STATS_ROW_CAP,
                PRUNE_BATCH,
            )
            .await
            .map_err(|e| e.to_string())?;
            total += n;
            if n < PRUNE_BATCH as u64 {
                break;
            }
        }
        Ok(total)
    }
}

/// [`PollLock`] on a Postgres session advisory lock. The lock lives on a connection taken out
/// of the pool, so it is released when the connection (or the process) goes away.
pub struct PgPollLock {
    pool: PgPool,
    held: tokio::sync::Mutex<Option<sqlx::PgConnection>>,
}

impl PgPollLock {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            held: Default::default(),
        }
    }
}

#[async_trait]
impl PollLock for PgPollLock {
    async fn try_acquire(&self) -> Result<bool, String> {
        let mut held = self.held.lock().await;
        if let Some(conn) = held.as_mut() {
            // A dead connection has lost the lock: drop it and compete again.
            if sqlx::query("SELECT 1").execute(&mut *conn).await.is_ok() {
                return Ok(true);
            }
            *held = None;
        }
        let mut conn: PoolConnection<sqlx::Postgres> =
            self.pool.acquire().await.map_err(|e| e.to_string())?;
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(POLL_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| e.to_string())?;
        if got {
            // Detached: closing it, not returning it to the pool, releases the lock.
            *held = Some(conn.detach());
        }
        Ok(got)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use enzarb_proto::ClientKind;
    use enzarb_proto::session::memory::session_pair_with;
    use enzarb_proto::session::{Event, ServerOptions};

    use super::*;
    use crate::executor::SandboxError;

    fn target(name: &str) -> SandboxTarget {
        SandboxTarget {
            endpoint: format!("ws://{name}"),
            token: "t".into(),
        }
    }

    fn sandbox(name: &str) -> RunningSandbox {
        RunningSandbox {
            project: name.into(),
            sandbox: format!("sb-{name}"),
            target: target(name),
        }
    }

    struct FakeResolver(Mutex<Vec<RunningSandbox>>);

    impl FakeResolver {
        fn of(names: &[&str]) -> Arc<Self> {
            Arc::new(Self(Mutex::new(names.iter().map(|n| sandbox(n)).collect())))
        }
    }

    #[async_trait]
    impl SandboxResolver for FakeResolver {
        async fn resolve(&self, p: &str) -> Result<SandboxTarget, SandboxError> {
            Ok(target(p))
        }

        async fn running_sandboxes(&self) -> Result<Vec<RunningSandbox>, SandboxError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    #[derive(Clone, Copy)]
    enum Behaviour {
        Ok,
        Hang,
        Unsupported,
        Unreachable,
        Slow(Duration),
    }

    /// Answers by endpoint (`ws://<project>`), tracking in-flight calls.
    #[derive(Default)]
    struct FakeClient {
        behaviour: Mutex<HashMap<String, Behaviour>>,
        calls: Mutex<Vec<String>>,
        in_flight: AtomicUsize,
        peak: AtomicUsize,
    }

    impl FakeClient {
        fn set(&self, project: &str, b: Behaviour) {
            self.behaviour.lock().unwrap().insert(project.into(), b);
        }

        fn calls_to(&self, project: &str) -> usize {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| *c == project)
                .count()
        }
    }

    fn snapshot(project: &str) -> SandboxStats {
        SandboxStats {
            ts_ms: 1_000 + project.len() as u64,
            memory_bytes: Some(project.len() as u64),
            ..Default::default()
        }
    }

    #[async_trait]
    impl StatsClient for FakeClient {
        async fn fetch(&self, t: &SandboxTarget) -> Result<SandboxStats, StatsError> {
            let project = t.endpoint.trim_start_matches("ws://").to_string();
            self.calls.lock().unwrap().push(project.clone());
            let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            let b = self
                .behaviour
                .lock()
                .unwrap()
                .get(&project)
                .copied()
                .unwrap_or(Behaviour::Ok);
            let r = match b {
                Behaviour::Ok => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok(snapshot(&project))
                }
                Behaviour::Slow(d) => {
                    tokio::time::sleep(d).await;
                    Ok(snapshot(&project))
                }
                Behaviour::Hang => std::future::pending().await,
                Behaviour::Unsupported => Err(StatsError::Unsupported),
                Behaviour::Unreachable => Err(StatsError::Unreachable("refused".into())),
            };
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            r
        }
    }

    #[derive(Default)]
    struct FakeSink {
        rows: Mutex<Vec<(String, String, SandboxStats)>>,
        fail: Mutex<Vec<String>>,
        prunes: AtomicUsize,
    }

    #[async_trait]
    impl SampleSink for FakeSink {
        async fn store(&self, p: &str, sb: &str, s: &SandboxStats) -> Result<bool, String> {
            if self.fail.lock().unwrap().iter().any(|f| f == p) {
                return Err("db down".into());
            }
            self.rows
                .lock()
                .unwrap()
                .push((p.into(), sb.into(), s.clone()));
            Ok(true)
        }

        async fn prune(&self, _: i32) -> Result<u64, String> {
            self.prunes.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }
    }

    struct FakeLock(Mutex<bool>);

    #[async_trait]
    impl PollLock for FakeLock {
        async fn try_acquire(&self) -> Result<bool, String> {
            Ok(*self.0.lock().unwrap())
        }
    }

    struct Rig {
        resolver: Arc<FakeResolver>,
        client: Arc<FakeClient>,
        sink: Arc<FakeSink>,
        lock: Arc<FakeLock>,
        poller: SandboxPoller,
    }

    fn rig(names: &[&str], cfg: PollerConfig) -> Rig {
        let client = Arc::new(FakeClient::default());
        let sink = Arc::new(FakeSink::default());
        let lock = Arc::new(FakeLock(Mutex::new(true)));
        let resolver = FakeResolver::of(names);
        let poller = SandboxPoller::new(
            resolver.clone(),
            client.clone(),
            sink.clone(),
            lock.clone(),
            cfg,
        );
        Rig {
            resolver,
            client,
            sink,
            lock,
            poller,
        }
    }

    fn cfg() -> PollerConfig {
        PollerConfig {
            interval: Duration::from_secs(30),
            timeout: Duration::from_secs(5),
            concurrency: 8,
            retention_days: 30,
            jitter: Duration::ZERO,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn every_running_sandbox_yields_one_sample() {
        let r = rig(&["a", "bb", "ccc"], cfg());
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!(
            s,
            PollStats {
                sampled: 3,
                ..Default::default()
            }
        );
        let mut rows = r.sink.rows.lock().unwrap().clone();
        rows.sort_by_key(|r| r.0.clone());
        assert_eq!(
            rows,
            [
                ("a".into(), "sb-a".into(), snapshot("a")),
                ("bb".into(), "sb-bb".into(), snapshot("bb")),
                ("ccc".into(), "sb-ccc".into(), snapshot("ccc")),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_sandbox_times_out_without_stalling_the_rest() {
        let r = rig(&["a", "hung", "b"], cfg());
        r.client.set("hung", Behaviour::Hang);
        let start = Instant::now();
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.sampled, s.timed_out, s.failed), (2, 1, 0));
        assert_eq!(start.elapsed(), Duration::from_secs(5));
        assert_eq!(r.sink.rows.lock().unwrap().len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_reply_within_the_timeout_is_kept() {
        let r = rig(&["a"], cfg());
        r.client.set("a", Behaviour::Slow(Duration::from_secs(4)));
        assert_eq!(r.poller.poll_once().await.unwrap().sampled, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_is_not_an_error_and_writes_nothing() {
        let r = rig(&["old"], cfg());
        r.client.set("old", Behaviour::Unsupported);
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!(
            (s.unsupported, s.failed, s.timed_out, s.sampled),
            (1, 0, 0, 0)
        );
        assert!(r.sink.rows.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unreachable_and_insert_errors_are_skipped_and_the_pass_continues() {
        let r = rig(&["down", "nodb", "ok"], cfg());
        r.client.set("down", Behaviour::Unreachable);
        r.sink.fail.lock().unwrap().push("nodb".into());
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.sampled, s.failed), (1, 2));
        assert_eq!(r.sink.rows.lock().unwrap()[0].0, "ok");
    }

    #[tokio::test(start_paused = true)]
    async fn concurrency_is_capped() {
        let names: Vec<String> = (0..10).map(|i| format!("p{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let r = rig(
            &refs,
            PollerConfig {
                concurrency: 3,
                ..cfg()
            },
        );
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!(s.sampled, 10);
        assert_eq!(r.client.peak.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn start_times_are_jittered_within_the_window() {
        let r = rig(
            &["a", "bb", "ccc", "dddd"],
            PollerConfig {
                jitter: Duration::from_secs(10),
                ..cfg()
            },
        );
        let start = Instant::now();
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!(s.sampled, 4);
        assert!(start.elapsed() < Duration::from_secs(10) + Duration::from_millis(10));
        assert_eq!(jitter_for("a", Duration::ZERO), Duration::ZERO);
        assert_eq!(
            jitter_for("a", Duration::from_secs(10)),
            jitter_for("a", Duration::from_secs(10))
        );
        assert!(jitter_for("a", Duration::from_secs(10)) < Duration::from_secs(10));
    }

    #[test]
    fn backoff_doubles_up_to_five_minutes() {
        let i = Duration::from_secs(30);
        let secs: Vec<u64> = (1..=6).map(|n| backoff_delay(i, n).as_secs()).collect();
        assert_eq!(secs, [60, 120, 240, 300, 300, 300]);
        assert_eq!(backoff_delay(i, 100), MAX_BACKOFF);
        // An interval above the cap is never shortened.
        assert_eq!(
            backoff_delay(Duration::from_secs(400), 1),
            Duration::from_secs(400)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failing_project_backs_off_and_recovers() {
        let r = rig(&["flaky", "fine"], cfg());
        r.client.set("flaky", Behaviour::Unreachable);

        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.failed, s.sampled, s.backed_off), (1, 1, 0));
        // Backoff after one failure is 60 s: skipped at +30 s, asked again at +60 s.
        tokio::time::advance(Duration::from_secs(30)).await;
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.failed, s.sampled, s.backed_off), (0, 1, 1));
        assert_eq!(r.client.calls_to("flaky"), 1);
        tokio::time::advance(Duration::from_secs(30)).await;
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.failed, s.backed_off), (1, 0));
        assert_eq!(r.client.calls_to("flaky"), 2);
        // Second failure: 120 s.
        tokio::time::advance(Duration::from_secs(90)).await;
        assert_eq!(r.poller.poll_once().await.unwrap().backed_off, 1);
        tokio::time::advance(Duration::from_secs(30)).await;
        // It recovers: the sample is stored and the backoff is gone.
        r.client.set("flaky", Behaviour::Ok);
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.sampled, s.backed_off), (2, 0));
        // Reset: a new failure backs off for 60 s again, not 240 s.
        r.client.set("flaky", Behaviour::Unreachable);
        assert_eq!(r.poller.poll_once().await.unwrap().failed, 1);
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(r.poller.poll_once().await.unwrap().failed, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unsupported_sandboxes_back_off_too() {
        let r = rig(&["old"], cfg());
        r.client.set("old", Behaviour::Unsupported);
        r.poller.poll_once().await.unwrap();
        let s = r.poller.poll_once().await.unwrap();
        assert_eq!((s.unsupported, s.backed_off), (0, 1));
    }

    #[tokio::test(start_paused = true)]
    async fn removed_projects_forget_their_backoff() {
        let r = rig(&["gone"], cfg());
        r.client.set("gone", Behaviour::Unreachable);
        r.poller.poll_once().await.unwrap();
        assert_eq!(r.poller.backoff.lock().unwrap().len(), 1);
        r.resolver.0.lock().unwrap().clear();
        r.poller.poll_once().await.unwrap();
        assert!(r.poller.backoff.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn passes_run_every_interval_starting_at_once() {
        let r = rig(&["a"], cfg());
        let client = r.client.clone();
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(r.poller.run(rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(client.calls_to("a"), 1, "the first pass is immediate");
        // N intervals later there have been N + 1 passes.
        tokio::time::sleep(Duration::from_millis(90_000 - 1_000 + 500)).await;
        assert_eq!(client.calls_to("a"), 4);
        tx.send(true).unwrap();
        task.await.unwrap();
        tokio::time::sleep(Duration::from_secs(120)).await;
        assert_eq!(client.calls_to("a"), 4, "no pass after shutdown");
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_pass_delays_the_next_instead_of_piling_up() {
        let r = rig(
            &["a"],
            PollerConfig {
                interval: Duration::from_secs(10),
                timeout: Duration::from_secs(25),
                ..cfg()
            },
        );
        r.client.set("a", Behaviour::Slow(Duration::from_secs(25)));
        let client = r.client.clone();
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(r.poller.run(rx));
        tokio::time::sleep(Duration::from_secs(60)).await;
        // Passes start at 0, 25 and 50, never overlapping.
        assert_eq!(client.calls_to("a"), 3);
        assert_eq!(client.peak.load(Ordering::SeqCst), 1);
        tx.send(true).unwrap();
        task.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_ends_the_loop_even_while_a_sandbox_hangs() {
        let r = rig(&["hung"], cfg());
        r.client.set("hung", Behaviour::Hang);
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(r.poller.run(rx));
        tokio::time::sleep(Duration::from_secs(1)).await;
        let start = Instant::now();
        tx.send(true).unwrap();
        task.await.unwrap();
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn a_poller_that_does_not_hold_the_lock_stays_idle() {
        let r = rig(&["a"], cfg());
        *r.lock.0.lock().unwrap() = false;
        r.poller.round().await;
        assert!(r.client.calls.lock().unwrap().is_empty());
        assert!(r.sink.rows.lock().unwrap().is_empty());
        assert_eq!(r.sink.prunes.load(Ordering::SeqCst), 0);
        // Once the other replica lets go, this one polls.
        *r.lock.0.lock().unwrap() = true;
        r.poller.round().await;
        assert_eq!(r.sink.rows.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn pruning_runs_at_most_hourly() {
        let r = rig(&["a"], cfg());
        r.poller.round().await;
        assert_eq!(r.sink.prunes.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(3000)).await;
        r.poller.round().await;
        assert_eq!(r.sink.prunes.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(700)).await;
        r.poller.round().await;
        assert_eq!(r.sink.prunes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn interval_is_clamped_to_the_accepted_range() {
        let secs = |n| PollerConfig::with_interval_secs(n).interval.as_secs();
        assert_eq!(
            (secs(0), secs(14), secs(30), secs(300), secs(301)),
            (15, 15, 30, 300, 300)
        );
    }

    fn stats_server(features: &[&str]) -> ServerOptions {
        ServerOptions {
            features: features.iter().map(|f| f.to_string()).collect(),
            ..Default::default()
        }
    }

    #[tokio::test(start_paused = true)]
    async fn no_request_is_sent_without_the_stats_feature() {
        let ((client, mut cev), (_server, mut sev)) =
            session_pair_with(ClientKind::Executor, stats_server(&[])).await;
        assert_eq!(
            request_stats(&client, &mut cev).await,
            Err(StatsError::Unsupported)
        );
        let nothing = tokio::time::timeout(Duration::from_millis(100), sev.next()).await;
        assert!(nothing.is_err(), "the old sandbox got a stats_request");
    }

    #[tokio::test(start_paused = true)]
    async fn the_stats_reply_is_returned_when_the_feature_is_listed() {
        let ((client, mut cev), (server, mut sev)) =
            session_pair_with(ClientKind::Executor, stats_server(&[feature::STATS])).await;
        let reply = snapshot("abc");
        let sent = reply.clone();
        tokio::spawn(async move {
            while let Some(ev) = sev.next().await {
                if let Event::Ctrl(Ctrl::StatsRequest) = ev {
                    server.send_ctrl(&Ctrl::Stats(sent.clone())).unwrap();
                }
            }
        });
        assert_eq!(request_stats(&client, &mut cev).await, Ok(reply));
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_message_means_unsupported_and_other_errors_do_not() {
        for (code, want) in [
            (error_code::UNKNOWN_MESSAGE, StatsError::Unsupported),
            (error_code::UNIMPLEMENTED, StatsError::Unsupported),
            (
                error_code::INTERNAL,
                StatsError::Other("internal: boom".into()),
            ),
        ] {
            let ((client, mut cev), (server, mut sev)) =
                session_pair_with(ClientKind::Executor, stats_server(&[feature::STATS])).await;
            tokio::spawn(async move {
                while let Some(ev) = sev.next().await {
                    if let Event::Ctrl(Ctrl::StatsRequest) = ev {
                        server.send_error(code, "boom").unwrap();
                    }
                }
            });
            assert_eq!(request_stats(&client, &mut cev).await, Err(want));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_that_ends_is_unreachable() {
        let ((client, mut cev), (server, _sev)) =
            session_pair_with(ClientKind::Executor, stats_server(&[feature::STATS])).await;
        server.close().await;
        assert!(matches!(
            request_stats(&client, &mut cev).await,
            Err(StatsError::Unreachable(_))
        ));
    }
}
