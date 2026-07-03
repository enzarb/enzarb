//! Owns the single, lazily-spawned `claude-agent-acp` process per project and
//! plays the ACP "client" role against it. All sessions (new or resumed)
//! multiplex over this one JSON-RPC connection via `session/new`/`session/load`.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest,
    PromptRequest, RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionModeState, SessionNotification, SetSessionModeRequest,
    TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell, broadcast};

use super::events::{
    AcpWsClientMsg, AcpWsEvent, from_session_update, permission_request_event, tool_kind_str,
};
use super::permissions::{PermissionRegistry, auto_allow};
use super::session::{SessionIndex, SessionModeInfo, SessionStatus};

fn split_modes(modes: Option<SessionModeState>) -> (Option<String>, Vec<SessionModeInfo>) {
    match modes {
        Some(m) => (
            Some(m.current_mode_id.to_string()),
            m.available_modes
                .into_iter()
                .map(|mode| SessionModeInfo {
                    id: mode.id.to_string(),
                    name: mode.name,
                    description: mode.description,
                })
                .collect(),
        ),
        None => (None, Vec::new()),
    }
}

const CHANNEL_CAPACITY: usize = 8192;
const ACP_COMMAND: &str = "claude-agent-acp";

type Channels = Arc<Mutex<HashMap<String, broadcast::Sender<AcpWsEvent>>>>;
// Per-session accumulated event log; populated by the notification handler so
// reconnecting clients can replay history without a second session/load call.
type History = Arc<Mutex<HashMap<String, Vec<AcpWsEvent>>>>;

#[derive(Clone)]
pub struct AcpStore {
    connection: Arc<OnceCell<ConnectionTo<Agent>>>,
    cwd: PathBuf,
    index: SessionIndex,
    permissions: PermissionRegistry,
    channels: Channels,
    history: History,
}

impl AcpStore {
    pub fn new(cwd: PathBuf, index: SessionIndex) -> Self {
        Self {
            connection: Arc::new(OnceCell::new()),
            cwd,
            index,
            permissions: PermissionRegistry::default(),
            channels: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn connection(&self) -> Result<ConnectionTo<Agent>> {
        self.connection
            .get_or_try_init(|| {
                spawn(
                    self.permissions.clone(),
                    self.channels.clone(),
                    self.history.clone(),
                )
            })
            .await
            .cloned()
    }

    async fn channel(&self, session_id: &str) -> broadcast::Sender<AcpWsEvent> {
        let mut channels = self.channels.lock().await;
        channels
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }

    pub async fn list_sessions(&self) -> Vec<super::session::SessionMeta> {
        self.index.list().await
    }

    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        self.history.lock().await.remove(session_id);
        self.channels.lock().await.remove(session_id);
        self.index.archive(session_id).await
    }

    pub async fn create_session(
        &self,
        label: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<super::session::SessionMeta> {
        let connection = self.connection().await?;
        let cwd = cwd.unwrap_or_else(|| self.cwd.clone());
        let response = connection
            .send_request(NewSessionRequest::new(cwd.clone()))
            .block_task()
            .await
            .map_err(|e| anyhow!("session/new failed: {e}"))?;
        let session_id = response.session_id.to_string();
        let (mode_id, available_modes) = split_modes(response.modes);
        self.index
            .insert(
                session_id,
                label.unwrap_or_else(|| "New session".to_string()),
                cwd,
                mode_id,
                available_modes,
            )
            .await
    }

    /// Attaches to a session for WS streaming.
    ///
    /// On first attach after process start (including after a workspace restart),
    /// calls `session/load` so Claude Code replays the conversation history as
    /// `SessionNotification` events. Those arrive via the broadcast channel and
    /// are also accumulated in the in-memory history log.
    ///
    /// On reconnect (same process lifetime), skips `session/load` and returns
    /// the in-memory history snapshot directly so the caller can replay it
    /// over the new WS connection without re-reading from disk.
    pub async fn attach(
        &self,
        session_id: &str,
    ) -> Result<(Vec<AcpWsEvent>, broadcast::Receiver<AcpWsEvent>)> {
        let meta = self
            .index
            .get(session_id)
            .await
            .ok_or_else(|| anyhow!("unknown session"))?;

        let connection = self.connection().await?;
        let tx = self.channel(session_id).await;

        // Check for in-memory history, subscribing while the lock is held so
        // we don't miss events arriving between the two operations.
        let history_snapshot = {
            let hist = self.history.lock().await;
            let rx = tx.subscribe();
            let snapshot = hist.get(session_id).cloned();
            drop(hist);
            snapshot.map(|s| (s, rx))
        };

        if let Some((snapshot, rx)) = history_snapshot {
            // Reconnect: history is already in memory; no session/load needed.
            self.index.touch(session_id, SessionStatus::Live).await?;
            return Ok((snapshot, rx));
        }

        // First attach in this process: subscribe before session/load so the
        // broadcast channel captures the replayed history notifications.
        let rx = tx.subscribe();

        let response = connection
            .send_request(LoadSessionRequest::new(meta.id.clone(), meta.cwd.clone()))
            .block_task()
            .await
            .map_err(|e| anyhow!("session/load failed: {e}"))?;

        let (mode_id, available_modes) = split_modes(response.modes);
        self.index
            .set_modes(session_id, mode_id, available_modes)
            .await?;
        self.index.touch(session_id, SessionStatus::Live).await?;

        // History events from session/load are queued in rx (and being
        // accumulated in self.history by the notification handler). Return
        // an empty snapshot so attach_ws reads them from rx directly.
        Ok((vec![], rx))
    }

    pub async fn handle_client_msg(&self, session_id: &str, msg: AcpWsClientMsg) -> Result<()> {
        let connection = self.connection().await?;
        match msg {
            AcpWsClientMsg::SendMessage { text } => {
                let connection = connection.clone();
                let session_id = session_id.to_string();
                let index = self.index.clone();
                let channels = self.channels.clone();
                // session/prompt blocks until the agent finishes its turn; run
                // it in the background so the WS input loop stays responsive.
                tokio::spawn(async move {
                    let result = connection
                        .send_request(PromptRequest::new(
                            session_id.clone(),
                            vec![ContentBlock::Text(TextContent::new(text))],
                        ))
                        .block_task()
                        .await;
                    if let Err(e) = result {
                        let tx = channels.lock().await.get(&session_id).cloned();
                        if let Some(tx) = tx {
                            let _ = tx.send(AcpWsEvent::Error {
                                session_id: Some(session_id.clone()),
                                message: e.to_string(),
                            });
                        }
                    }
                    let _ = index.touch(&session_id, SessionStatus::Idle).await;
                });
                Ok(())
            }
            AcpWsClientMsg::PermissionResponse {
                request_id,
                option_id,
            } => {
                let resolved = self.permissions.resolve(&request_id, option_id).await;
                if resolved {
                    let tx = self.channel(session_id).await;
                    let _ = tx.send(AcpWsEvent::PermissionResolved {
                        session_id: session_id.to_string(),
                        request_id,
                    });
                }
                Ok(())
            }
            AcpWsClientMsg::SetPermissionMode { mode_id } => {
                connection
                    .send_request(SetSessionModeRequest::new(
                        session_id.to_string(),
                        mode_id.clone(),
                    ))
                    .block_task()
                    .await
                    .map_err(|e| anyhow!("session/set_mode failed: {e}"))?;
                self.index.set_mode(session_id, mode_id.clone()).await?;
                let tx = self.channel(session_id).await;
                let _ = tx.send(AcpWsEvent::ModeChanged {
                    session_id: session_id.to_string(),
                    mode_id,
                });
                Ok(())
            }
            AcpWsClientMsg::Cancel => {
                connection
                    .send_notification(CancelNotification::new(session_id.to_string()))
                    .map_err(|e| anyhow!("session/cancel failed: {e}"))?;
                Ok(())
            }
        }
    }
}

/// Spawns the `claude-agent-acp` child process and drives the ACP client
/// event loop indefinitely in a background task, returning the connection
/// handle once `initialize` completes. `ConnectionTo` is cheaply cloneable
/// and safe to share across tasks, so callers just hold onto the handle
/// rather than talking to the spawned task directly.
async fn spawn(
    permissions: PermissionRegistry,
    channels: Channels,
    history: History,
) -> Result<ConnectionTo<Agent>> {
    let agent = AcpAgent::from_str(ACP_COMMAND).map_err(|e| anyhow!("invalid ACP command: {e}"))?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let notification_channels = channels.clone();
        let notification_history = history.clone();
        let result = Client
            .builder()
            .on_receive_notification(
                move |notification: SessionNotification, _cx| {
                    let channels = notification_channels.clone();
                    let history = notification_history.clone();
                    async move {
                        let session_id = notification.session_id.to_string();
                        let events = from_session_update(&session_id, notification.update);
                        if !events.is_empty() {
                            history
                                .lock()
                                .await
                                .entry(session_id.clone())
                                .or_default()
                                .extend(events.iter().cloned());
                        }
                        let tx = channels.lock().await.get(&session_id).cloned();
                        if let Some(tx) = tx {
                            for event in events {
                                let _ = tx.send(event);
                            }
                        }
                        Ok(())
                    }
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                move |request: RequestPermissionRequest,
                      responder: agent_client_protocol::Responder<RequestPermissionResponse>,
                      _connection| {
                    let permissions = permissions.clone();
                    let channels = channels.clone();
                    async move {
                        let session_id = request.session_id.to_string();
                        let kind = request
                            .tool_call
                            .fields
                            .kind
                            .map(tool_kind_str)
                            .unwrap_or("other");

                        let chosen_option = if auto_allow(kind) {
                            request.options.first().map(|o| o.option_id.to_string())
                        } else {
                            let (request_id, rx) = permissions.register().await;
                            let tx = channels.lock().await.get(&session_id).cloned();
                            if let Some(tx) = tx {
                                let _ = tx.send(permission_request_event(
                                    &session_id,
                                    &request_id,
                                    &request,
                                ));
                            }
                            rx.await.ok()
                        };

                        match chosen_option {
                            Some(id) => responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                    id,
                                )),
                            )),
                            None => responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            )),
                        }
                    }
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(agent, move |connection: ConnectionTo<Agent>| async move {
                connection
                    .send_request(InitializeRequest::new(ProtocolVersion::V1))
                    .block_task()
                    .await?;
                let _ = ready_tx.send(connection);
                // Keep this future alive for the process lifetime — it drives
                // the JSON-RPC event loop for as long as the child runs.
                std::future::pending::<()>().await;
                Ok(())
            })
            .await;

        if let Err(e) = result {
            tracing::warn!(error = %e, "ACP connection ended");
        }
    });

    ready_rx
        .await
        .map_err(|_| anyhow!("ACP agent process failed to initialize"))
}
