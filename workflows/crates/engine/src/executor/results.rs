//! Run results pulled over the mux (`feature::RUN_RESULTS`, ADR-0008 amendment of
//! 2026-10-15), replacing the bearer-token HTTP callback.
//!
//! Every [`ExecutorConfig::results_interval`](super::ExecutorConfig::results_interval) a pass
//! ([`Executor::watch_results`]) lists the accepted runs (`status = 'running'`, `deadline`
//! set) by Project, and keeps **one executor session per Project sandbox** open while it has
//! any:
//! * the session subscribes to the runs (`run_subscribe`) and ingests every `run_finished`
//!   push as soon as it arrives;
//! * each pass also asks `run_status` for every listed run and ingests `run_state.result`
//!   when present. That is the sweep that covers dropped sessions, executor restarts and
//!   sandbox-init restarts (the sandbox keeps results on disk);
//! * a session that drops is re-dialed on the next pass; a Project with no accepted runs left
//!   has its session closed.
//!
//! A sandbox without the feature gets no session (it is dialed again each pass, so it is
//! picked up once it rolls); its runs still carry a `callback_url` and report through the
//! deprecated HTTP callback, with the deadline poll as the fallback.
//!
//! Every replica runs the passes: ingestion ([`Executor::ingest_run_finished`]) is idempotent
//! and only accepts runs of the Project whose sandbox reported them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use enzarb_proto::session::{Event, Session, SessionError};
use enzarb_proto::{Ctrl, RunFinished, feature};
use serde_json::Value;
use tokio::sync::watch;

use super::Executor;
use super::callback::{current_run_id, recovery_used};
use super::finish::IngestError;
use crate::error::Error;

/// The watcher of one Project's sandbox.
pub(crate) struct Watcher {
    runs: watch::Sender<Vec<String>>,
    task: tokio::task::JoinHandle<()>,
}

impl Executor {
    /// One pass (module docs). Returns the number of Projects watched.
    pub async fn watch_results(self: &Arc<Self>) -> Result<usize, Error> {
        let rows: Vec<(i64, i32, String, Value)> = sqlx::query_as(
            r"SELECT id, limit_wait_count + restart_retries, project_id, input FROM jobs
              WHERE status = 'running' AND deadline IS NOT NULL AND project_id IS NOT NULL
                AND (kind LIKE 'agent\_%' OR kind = 'retarget_change_request')
              ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut by_project: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, waits, project, input) in rows {
            by_project.entry(project).or_default().push(current_run_id(
                id,
                waits,
                recovery_used(&input),
            ));
        }
        let mut watchers = self.watchers.lock().expect("watchers lock");
        watchers.retain(|p, _| by_project.contains_key(p));
        for (project, runs) in &by_project {
            match watchers.get(project) {
                Some(w) if !w.task.is_finished() => {
                    // A pass also re-checks every run (`send_replace` always wakes).
                    w.runs.send_replace(runs.clone());
                }
                _ => {
                    let (tx, rx) = watch::channel(runs.clone());
                    let me = self.clone();
                    let p = project.clone();
                    let task = tokio::spawn(async move { me.watch_project(p, rx).await });
                    watchers.insert(project.clone(), Watcher { runs: tx, task });
                }
            }
        }
        Ok(by_project.len())
    }

    /// Close every watcher session (shutdown).
    pub(crate) fn stop_watchers(&self) {
        self.watchers.lock().expect("watchers lock").clear();
    }

    async fn watch_project(
        self: Arc<Self>,
        project: String,
        mut runs: watch::Receiver<Vec<String>>,
    ) {
        loop {
            let (session, mut events) = match self.dial_with_events(&project).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(%project, error = %e.message, "results: sandbox unreachable");
                    if runs.changed().await.is_err() {
                        return;
                    }
                    continue;
                }
            };
            if !session.has_feature(feature::RUN_RESULTS) {
                // An older sandbox: its runs report through the HTTP callback.
                session.close().await;
                if runs.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let mut subscribed = HashSet::new();
            let ids = runs.borrow_and_update().clone();
            let mut alive = self.sync(&session, &project, &ids, &mut subscribed).await;
            while alive {
                tokio::select! {
                    changed = runs.changed() => {
                        if changed.is_err() {
                            session.close().await;
                            return;
                        }
                        let ids = runs.borrow_and_update().clone();
                        alive = self.sync(&session, &project, &ids, &mut subscribed).await;
                    }
                    ev = events.next() => match ev {
                        None => alive = false,
                        Some(Event::Ctrl(Ctrl::RunFinished(fin))) => {
                            self.ingest_delivered(&project, fin, "run_finished").await;
                        }
                        Some(_) => {}
                    },
                }
            }
            session.close().await;
            tracing::debug!(%project, "results: session ended; re-dialing on the next pass");
            if runs.changed().await.is_err() {
                return;
            }
        }
    }

    /// Subscribe to new runs and ask `run_status` for all of them. False when the session is
    /// gone.
    async fn sync(
        &self,
        session: &Session,
        project: &str,
        ids: &[String],
        subscribed: &mut HashSet<String>,
    ) -> bool {
        subscribed.retain(|r| ids.contains(r));
        let new: Vec<String> = ids
            .iter()
            .filter(|r| !subscribed.contains(*r))
            .cloned()
            .collect();
        if !new.is_empty() {
            if session.run_subscribe(new.clone()).is_err() {
                return false;
            }
            subscribed.extend(new);
        }
        for rid in ids {
            let status =
                tokio::time::timeout(self.cfg.sandbox_timeout, session.run_status(rid)).await;
            match status {
                Err(_) => return false,
                Ok(Ok(info)) => {
                    if let Some(fin) = info.result.filter(|f| f.run_id == *rid) {
                        self.ingest_delivered(project, fin, "run_status").await;
                    }
                }
                // Unknown runs (a restarted sandbox) are the deadline poll's business.
                Ok(Err(SessionError::Remote { .. })) => {}
                Ok(Err(_)) => return false,
            }
        }
        true
    }

    async fn ingest_delivered(&self, project: &str, fin: RunFinished, via: &str) {
        let run_id = fin.run_id.clone();
        match self.ingest_run_finished(Some(project), fin).await {
            Ok(true) => tracing::info!(%project, %run_id, via, "run result recorded"),
            Ok(false) => {}
            Err(e @ (IngestError::BadRunId | IngestError::UnknownJob)) => {
                tracing::warn!(%project, %run_id, via, error = %e, "run result rejected")
            }
            Err(e) => {
                tracing::warn!(%project, %run_id, via, error = %e, "run result not recorded; retrying")
            }
        }
    }
}

/// Watchers by Project id.
pub(crate) type Watchers = std::sync::Mutex<HashMap<String, Watcher>>;
