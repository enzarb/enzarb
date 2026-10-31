//! Talking to a Project's sandbox-init and to the credential store.
//!
//! The executor only sees the traits here, so tests drive it with a fake sandbox that speaks the
//! real step frames (`enzarb_proto::session::memory`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use enzarb_crds::Project;
use enzarb_proto::session::{Client, Event, Events, Session, StepInfo};
use enzarb_proto::{ClientKind, Ctrl, Secret, StepFinished, StepPhase, StepStart, feature};
use k8s_openapi::api::core::v1::Secret as KubeSecret;
use kube::{Api, Client as KubeClient};
use tokio::sync::Notify;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Worth retrying later (sandbox down, connection lost): the run stays as it is.
    Transient(String),
    /// The sandbox cannot run the step at all (`ez_steps` missing, bad request).
    Permanent(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(m) | Self::Permanent(m) => f.write_str(m),
        }
    }
}

/// A connected, handshaken mux session to one sandbox.
#[async_trait]
pub trait Link: Send + Sync {
    /// `step_start`. Idempotent per `run_id`: the sandbox answers "existing" for a known one.
    async fn start(&self, step: StepStart) -> Result<StepPhase, LinkError>;
    /// `step_status`: the phase, and the result once terminal.
    async fn status(&self, run_id: &str) -> Result<(StepPhase, Option<StepFinished>), LinkError>;
    /// Resolves when the step has finished (pushed `step_finished` or polled `step_status`).
    async fn finished(&self, run_id: &str) -> Result<StepFinished, LinkError>;
    /// `step_stop`.
    fn stop(&self, run_id: &str);
    /// When the step last showed progress (a session event not seen before). `None`: unknown.
    fn last_activity(&self, _run_id: &str) -> Option<Instant> {
        None
    }
}

#[async_trait]
pub trait Sandboxes: Send + Sync {
    /// Dials the sandbox of Project `project`.
    async fn connect(&self, project: &str) -> Result<Arc<dyn Link>, LinkError>;
}

/// The Project owner's Claude token (`CLAUDE_CODE_OAUTH_TOKEN`).
#[async_trait]
pub trait ClaudeTokens: Send + Sync {
    /// `Ok(None)`: the owner has not stored one. `Err` is transient.
    async fn token_for(&self, owner: &str) -> Result<Option<Secret>, String>;
    /// The Project's owner (its spec).
    async fn owner_of(&self, project: &str) -> Result<Option<String>, String>;
}

// ----- real implementation over enzarb-proto's Session -----

/// A [`Link`] over a real session. A task drains the unsolicited events into a map.
pub struct SessionLink {
    session: Session,
    done: Arc<Mutex<HashMap<String, StepFinished>>>,
    /// Per run: the last `error` frame the sandbox sent for it that no request was waiting for.
    refused: Arc<Mutex<HashMap<String, String>>>,
    /// Per run: the highest session event `seq` seen and when it arrived.
    progress: Arc<Mutex<HashMap<String, (u64, Instant)>>>,
    wake: Arc<Notify>,
}

impl SessionLink {
    /// The highest session event `seq` received for `run_id` (0: none yet).
    fn seen(&self, run_id: &str) -> u64 {
        self.progress
            .lock()
            .unwrap()
            .get(run_id)
            .map_or(0, |(seq, _)| *seq)
    }

    pub fn new(session: Session, mut events: Events) -> Self {
        let done: Arc<Mutex<HashMap<String, StepFinished>>> = Arc::default();
        let refused: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let progress: Arc<Mutex<HashMap<String, (u64, Instant)>>> = Arc::default();
        let wake = Arc::new(Notify::new());
        let (d, r, p, w) = (
            done.clone(),
            refused.clone(),
            progress.clone(),
            wake.clone(),
        );
        tokio::spawn(async move {
            while let Some(ev) = events.next().await {
                let Event::Ctrl(c) = ev else { continue };
                match c {
                    Ctrl::StepFinished(f) => {
                        d.lock().unwrap().insert(f.run_id.clone(), f);
                        w.notify_waiters();
                    }
                    // A status poll replays old events: only a higher `seq` is progress.
                    Ctrl::SessionEvent(e) => {
                        let mut p = p.lock().unwrap();
                        let seen = p.get(&e.run_id).map_or(0, |(s, _)| *s);
                        if e.seq > seen {
                            p.insert(e.run_id, (e.seq, Instant::now()));
                        }
                    }
                    Ctrl::Error {
                        code,
                        message,
                        run_id: Some(id),
                        ..
                    } => {
                        r.lock().unwrap().insert(id, format!("{code}: {message}"));
                        w.notify_waiters();
                    }
                    _ => {}
                }
            }
            w.notify_waiters();
        });
        Self {
            session,
            done,
            refused,
            progress,
            wake,
        }
    }
}

fn transient(e: impl std::fmt::Display) -> LinkError {
    LinkError::Transient(e.to_string())
}

#[async_trait]
impl Link for SessionLink {
    async fn start(&self, step: StepStart) -> Result<StepPhase, LinkError> {
        if step.workspaces.iter().any(|w| w.start_ref.is_some())
            && !self.session.has_feature(feature::STEP_START_REF)
        {
            return Err(LinkError::Permanent(
                "the sandbox image does not support step_start_ref, which a pull request run needs (update the Project's sandbox)".into(),
            ));
        }
        let a = self.session.step_start(step).await.map_err(transient)?;
        Ok(a.state)
    }

    async fn status(&self, run_id: &str) -> Result<(StepPhase, Option<StepFinished>), LinkError> {
        let i: StepInfo = self
            .session
            // Only events after the highest `seq` already seen: a poll that started at 0 would
            // replay the whole session every time it is called (the liveness check polls).
            .step_status(run_id, self.seen(run_id))
            .await
            .map_err(transient)?;
        Ok((i.state, i.result))
    }

    async fn finished(&self, run_id: &str) -> Result<StepFinished, LinkError> {
        loop {
            if let Some(f) = self.done.lock().unwrap().get(run_id) {
                return Ok(f.clone());
            }
            if let Some(why) = self.refused.lock().unwrap().get(run_id) {
                return Err(LinkError::Transient(format!("sandbox refused: {why}")));
            }
            if self.session.is_closed() {
                return Err(LinkError::Transient("the sandbox connection closed".into()));
            }
            let notified = self.wake.notified();
            // Backstop for a missed push: poll the status now and then.
            if tokio::time::timeout(Duration::from_secs(5), notified)
                .await
                .is_err()
            {
                let (phase, result) = self.status(run_id).await?;
                if let Some(f) = result {
                    return Ok(f);
                }
                if phase == StepPhase::Unknown {
                    return Err(LinkError::Transient("the sandbox lost the step".into()));
                }
            }
        }
    }

    fn stop(&self, run_id: &str) {
        let _ = self.session.step_stop(run_id);
    }

    fn last_activity(&self, run_id: &str) -> Option<Instant> {
        self.progress.lock().unwrap().get(run_id).map(|(_, at)| *at)
    }
}

/// Dials sandboxes found through the Project CR: `status.sandbox.endpoint` and the auth Secret
/// `status.sandbox.authSecretRef` (`sb-*-auth`) in the Project's namespace.
pub struct KubeSandboxes {
    client: KubeClient,
}

impl KubeSandboxes {
    pub async fn new() -> anyhow::Result<Self> {
        Ok(Self {
            client: KubeClient::try_default().await?,
        })
    }
}

#[async_trait]
impl Sandboxes for KubeSandboxes {
    async fn connect(&self, project: &str) -> Result<Arc<dyn Link>, LinkError> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let list = projects
            .list(&Default::default())
            .await
            .map_err(|e| transient(format!("listing Projects: {e}")))?;
        let p = list
            .items
            .into_iter()
            .find(|p| p.metadata.name.as_deref() == Some(project))
            .ok_or_else(|| LinkError::Permanent(format!("there is no Project {project}")))?;
        let st = p.status.unwrap_or_default();
        let (Some(ns), Some(sb)) = (st.namespace, st.sandbox) else {
            return Err(LinkError::Transient(format!(
                "the sandbox of {project} is not ready"
            )));
        };
        let secrets: Api<KubeSecret> = Api::namespaced(self.client.clone(), &ns);
        let secret = secrets
            .get(&sb.auth_secret_ref.name)
            .await
            .map_err(|e| transient(format!("reading the sandbox auth Secret: {e}")))?;
        let token = secret
            .data
            .and_then(|mut d| d.remove(&sb.auth_secret_ref.key))
            .and_then(|b| String::from_utf8(b.0).ok())
            .ok_or_else(|| transient("the sandbox auth Secret has no token yet"))?;
        let (session, events) = tokio::time::timeout(
            Duration::from_secs(15),
            Client::connect(&sb.endpoint, token.trim(), ClientKind::Executor),
        )
        .await
        .map_err(|_| transient("dialing the sandbox timed out"))?
        .map_err(|e| transient(format!("dialing the sandbox: {e}")))?;
        if !session.has_feature(feature::EZ_STEPS) {
            session.close().await;
            return Err(LinkError::Permanent(
                "the sandbox image does not support ez_steps (update the Project's sandbox)".into(),
            ));
        }
        Ok(Arc::new(SessionLink::new(session, events)))
    }
}

/// Project owners from the Project CR, tokens from the encrypted store `sandbox-proxy` writes.
pub struct StoredTokens {
    client: KubeClient,
    creds: enzarb_sandbox_proxy::store::Credentials,
}

impl StoredTokens {
    pub async fn new(creds: enzarb_sandbox_proxy::store::Credentials) -> anyhow::Result<Self> {
        Ok(Self {
            client: KubeClient::try_default().await?,
            creds,
        })
    }
}

#[async_trait]
impl ClaudeTokens for StoredTokens {
    async fn token_for(&self, owner: &str) -> Result<Option<Secret>, String> {
        self.creds
            .token_for(owner)
            .await
            .map_err(|e| format!("reading the Claude token: {e}"))
    }

    async fn owner_of(&self, project: &str) -> Result<Option<String>, String> {
        let projects: Api<Project> = Api::all(self.client.clone());
        let list = projects
            .list(&Default::default())
            .await
            .map_err(|e| format!("listing Projects: {e}"))?;
        Ok(list
            .items
            .into_iter()
            .find(|p| p.metadata.name.as_deref() == Some(project))
            .map(|p| p.spec.owner))
    }
}
