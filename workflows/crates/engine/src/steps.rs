//! Running steps in a Project's sandbox over the mux (proto step frames). The engine only
//! ever talks to sandbox-init through [`StepRunner`]; tests use [`FakeStepRunner`].

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use enzarb_proto::session::{Client, Event};
use enzarb_proto::{ClientKind, Ctrl, SessionEvent, StepFinished, StepPhase, StepStart, feature};
use tokio::sync::{Notify, mpsc};

use crate::sandbox::{SandboxError, SandboxResolver};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    /// The sandbox cannot be reached or went away; the job retries with backoff.
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("sandbox: {0}")]
    Other(String),
}

impl From<SandboxError> for StepError {
    fn from(e: SandboxError) -> Self {
        StepError::Unavailable(e.to_string())
    }
}

/// How a run ended, plus whether it had to be resumed (after a sandbox restart).
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub finished: StepFinished,
    pub resumed: bool,
}

#[async_trait]
pub trait StepRunner: Send + Sync {
    /// Starts the step (or attaches to it when the sandbox already knows `run_id`), streams its
    /// session events with `seq > after_seq` into `events`, and returns when it finishes. An
    /// interrupted step is resumed, never restarted.
    async fn run(
        &self,
        project: &str,
        start: StepStart,
        after_seq: u64,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Result<RunOutcome, StepError>;

    async fn steer(&self, project: &str, run_id: &str, text: &str) -> Result<(), StepError>;

    async fn stop(&self, project: &str, run_id: &str) -> Result<(), StepError>;
}

/// The real runner: one executor session per run.
pub struct MuxStepRunner {
    resolver: Arc<dyn SandboxResolver>,
}

impl MuxStepRunner {
    pub fn new(resolver: Arc<dyn SandboxResolver>) -> Self {
        Self { resolver }
    }

    async fn connect(
        &self,
        project: &str,
    ) -> Result<
        (
            enzarb_proto::session::Session,
            enzarb_proto::session::Events,
        ),
        StepError,
    > {
        let t = self.resolver.resolve(project).await?;
        let (s, ev) = Client::connect(&t.endpoint, &t.token, ClientKind::Executor)
            .await
            .map_err(|e| StepError::Unavailable(e.to_string()))?;
        if !s.has_feature(feature::STEPS) {
            return Err(StepError::Unavailable(
                "sandbox does not support steps (old sandbox image)".into(),
            ));
        }
        Ok((s, ev))
    }
}

fn unavailable(e: impl std::fmt::Display) -> StepError {
    StepError::Unavailable(e.to_string())
}

#[async_trait]
impl StepRunner for MuxStepRunner {
    async fn run(
        &self,
        project: &str,
        start: StepStart,
        after_seq: u64,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Result<RunOutcome, StepError> {
        let run_id = start.run_id.clone();
        let (s, mut ev) = self.connect(project).await?;
        // Forward events while the handshake calls below are pending.
        let fwd_run = run_id.clone();
        let (fin_tx, mut fin_rx) = mpsc::unbounded_channel::<StepFinished>();
        let pump = tokio::spawn(async move {
            while let Some(e) = ev.next().await {
                match e {
                    Event::Ctrl(Ctrl::SessionEvent(se)) if se.run_id == fwd_run => {
                        let _ = events.send(se);
                    }
                    Event::Ctrl(Ctrl::StepFinished(f)) if f.run_id == fwd_run => {
                        let _ = fin_tx.send(f);
                    }
                    _ => {}
                }
            }
        });
        let info = s
            .step_status(&run_id, after_seq)
            .await
            .map_err(unavailable)?;
        let mut resumed = false;
        let _ = &mut resumed;
        if let Some(r) = info.result {
            pump.abort();
            return Ok(RunOutcome {
                finished: r,
                resumed,
            });
        }
        match info.state {
            StepPhase::Running => {}
            // After a sandbox restart secrets live nowhere: re-sending step_start for the known
            // run continues it with fresh secrets (ACP session/load), it does not restart it.
            StepPhase::Interrupted | StepPhase::Finished => {
                resumed = true;
                s.step_start(start).await.map_err(unavailable)?;
            }
            StepPhase::Unknown => {
                s.step_start(start).await.map_err(unavailable)?;
            }
        }
        // Subscribe (and replay what was logged since `after_seq`) in case the sandbox did not
        // subscribe the starting session; a step that already ended answers with its result.
        let info = s
            .step_status(&run_id, after_seq)
            .await
            .map_err(unavailable)?;
        if let Some(r) = info.result {
            pump.abort();
            return Ok(RunOutcome {
                finished: r,
                resumed,
            });
        }
        let out = tokio::select! {
            f = fin_rx.recv() => f.ok_or_else(|| unavailable("session ended before step_finished")),
            end = s.closed() => Err(unavailable(format!("session ended: {end:?}"))),
        };
        pump.abort();
        out.map(|finished| RunOutcome { finished, resumed })
    }

    async fn steer(&self, project: &str, run_id: &str, text: &str) -> Result<(), StepError> {
        let (s, _ev) = self.connect(project).await?;
        s.step_steer(run_id, text).map_err(unavailable)?;
        s.close().await;
        Ok(())
    }

    async fn stop(&self, project: &str, run_id: &str) -> Result<(), StepError> {
        let (s, _ev) = self.connect(project).await?;
        s.step_stop(run_id).map_err(unavailable)?;
        s.close().await;
        Ok(())
    }
}

// ---------------------------------------------------------------- fake

#[derive(Default)]
struct FakeState {
    started: Vec<StepStart>,
    events: HashMap<String, VecDeque<SessionEvent>>,
    results: HashMap<String, StepFinished>,
    /// Runs that return `Unavailable` once (simulates an interrupted executor/sandbox).
    fail_once: HashMap<String, bool>,
    steers: Vec<(String, String)>,
    stops: Vec<String>,
}

/// In-memory runner: tests script events and results per run id.
#[derive(Default, Clone)]
pub struct FakeStepRunner {
    st: Arc<Mutex<FakeState>>,
    changed: Arc<Notify>,
}

impl FakeStepRunner {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn started(&self) -> Vec<StepStart> {
        self.lock().started.clone()
    }

    pub fn push_event(&self, run_id: &str, kind: &str, payload: serde_json::Value) {
        let mut st = self.lock();
        let q = st.events.entry(run_id.to_string()).or_default();
        let seq = q.back().map(|e| e.seq).unwrap_or(0) + 1;
        q.push_back(SessionEvent {
            run_id: run_id.into(),
            seq,
            at: chrono::Utc::now().to_rfc3339(),
            kind: kind.into(),
            payload,
            truncated: false,
        });
        drop(st);
        self.changed.notify_waiters();
    }

    pub fn finish(&self, f: StepFinished) {
        self.lock().results.insert(f.run_id.clone(), f);
        self.changed.notify_waiters();
    }

    /// The next `run` of `run_id` fails as if the sandbox went away.
    pub fn interrupt_once(&self, run_id: &str) {
        self.lock().fail_once.insert(run_id.to_string(), true);
        self.changed.notify_waiters();
    }

    pub fn steers(&self) -> Vec<(String, String)> {
        self.lock().steers.clone()
    }

    pub fn stops(&self) -> Vec<String> {
        self.lock().stops.clone()
    }
}

#[async_trait]
impl StepRunner for FakeStepRunner {
    async fn run(
        &self,
        _project: &str,
        start: StepStart,
        after_seq: u64,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> Result<RunOutcome, StepError> {
        let run_id = start.run_id.clone();
        let resumed = self.lock().started.iter().any(|s| s.run_id == run_id);
        self.lock().started.push(start);
        let mut sent = after_seq;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = self.lock();
                if let Some(q) = st.events.get(&run_id) {
                    let fresh: Vec<_> = q.iter().filter(|e| e.seq > sent).cloned().collect();
                    for e in fresh {
                        sent = e.seq;
                        let _ = events.send(e);
                    }
                }
                if st.fail_once.remove(&run_id).is_some() {
                    return Err(StepError::Unavailable("fake interruption".into()));
                }
                if let Some(f) = st.results.get(&run_id) {
                    return Ok(RunOutcome {
                        finished: f.clone(),
                        resumed,
                    });
                }
            }
            notified.await;
        }
    }

    async fn steer(&self, _project: &str, run_id: &str, text: &str) -> Result<(), StepError> {
        self.lock().steers.push((run_id.into(), text.into()));
        Ok(())
    }

    async fn stop(&self, _project: &str, run_id: &str) -> Result<(), StepError> {
        self.lock().stops.push(run_id.into());
        Ok(())
    }
}
