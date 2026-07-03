//! Owns the single, lazily-spawned `claude-agent-acp` process per project and
//! plays the ACP "client" role against it. All sessions (new or resumed)
//! multiplex over this one JSON-RPC connection via `session/new`/`session/load`.
//!
//! Session metadata is sourced directly from `session/list`; only ephemeral
//! state (live/idle status, per-session mode info, cwd cache) is kept in
//! memory. A minimal `.enzarb/archived_sessions.json` persists the set of
//! session IDs the user has deleted so they stay hidden across pod restarts.

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, RequestPermissionOutcome, RequestPermissionRequest,
    RequestPermissionResponse, SelectedPermissionOutcome, SessionInfo, SessionModeState,
    SessionNotification, SetSessionModeRequest, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell, broadcast};

use crate::init::home_dir;

use super::events::{
    AcpWsClientMsg, AcpWsEvent, from_session_update, permission_request_event, tool_kind_str,
};
use super::permissions::{PermissionRegistry, auto_allow};

const ARCHIVED_FILE: &str = ".enzarb/archived_sessions.json";
const CHANNEL_CAPACITY: usize = 8192;
const ACP_COMMAND: &str = "claude-agent-acp";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Live,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionModeInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

/// The session view returned to the frontend and used in WS events.
/// Derived on-the-fly by merging `session/list` data with in-memory state.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub id: String,
    /// Populated from `SessionInfo.title`; falls back to first 8 chars of the ID.
    pub label: String,
    pub cwd: String,
    pub updated_at: Option<String>,
    pub status: SessionStatus,
    pub mode_id: Option<String>,
    pub available_modes: Vec<SessionModeInfo>,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

type Channels = Arc<Mutex<HashMap<String, broadcast::Sender<AcpWsEvent>>>>;
type History = Arc<Mutex<HashMap<String, Vec<AcpWsEvent>>>>;
type SessionModes = Arc<Mutex<HashMap<String, (Option<String>, Vec<SessionModeInfo>)>>>;

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

fn session_meta_from_info(
    info: SessionInfo,
    live: &HashSet<String>,
    modes: &HashMap<String, (Option<String>, Vec<SessionModeInfo>)>,
) -> SessionMeta {
    let id = info.session_id.to_string();
    let status = if live.contains(&id) {
        SessionStatus::Live
    } else {
        SessionStatus::Idle
    };
    let (mode_id, available_modes) = modes.get(&id).cloned().unwrap_or_default();
    let label = info
        .title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| id[..8.min(id.len())].to_string());
    SessionMeta {
        cwd: info.cwd.to_string_lossy().into_owned(),
        updated_at: info.updated_at,
        id,
        label,
        status,
        mode_id,
        available_modes,
    }
}

// ---------------------------------------------------------------------------
// AcpStore
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AcpStore {
    connection: Arc<OnceCell<ConnectionTo<Agent>>>,
    cwd: PathBuf,
    permissions: PermissionRegistry,
    channels: Channels,
    history: History,
    /// In-memory only: which sessions are currently running a prompt.
    /// Resets on pod restart — correct, since nothing is live after a restart.
    live_sessions: Arc<Mutex<HashSet<String>>>,
    /// Per-session mode info populated from `session/new` and `session/load`.
    session_modes: SessionModes,
    /// cwd per session, populated from `session/new` and `session/list` results.
    session_cwd: Arc<Mutex<HashMap<String, PathBuf>>>,
    /// Persisted set of session IDs hidden from `session/list`.
    archived: Arc<Mutex<HashSet<String>>>,
    archived_path: PathBuf,
}

impl AcpStore {
    pub async fn new(cwd: PathBuf) -> Result<Self> {
        let archived_path = home_dir().join(ARCHIVED_FILE);
        let archived = if archived_path.exists() {
            let data = tokio::fs::read(&archived_path).await?;
            serde_json::from_slice::<HashSet<String>>(&data).unwrap_or_default()
        } else {
            HashSet::new()
        };
        Ok(Self {
            connection: Arc::new(OnceCell::new()),
            cwd,
            permissions: PermissionRegistry::default(),
            channels: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(HashMap::new())),
            live_sessions: Arc::new(Mutex::new(HashSet::new())),
            session_modes: Arc::new(Mutex::new(HashMap::new())),
            session_cwd: Arc::new(Mutex::new(HashMap::new())),
            archived: Arc::new(Mutex::new(archived)),
            archived_path,
        })
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

    /// Fetches all sessions from the ACP agent, following pagination cursors.
    async fn fetch_all_from_acp(&self) -> Result<Vec<SessionInfo>> {
        let connection = self.connection().await?;
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let mut req = ListSessionsRequest::new();
            if let Some(c) = cursor {
                req = req.cursor(c);
            }
            let resp = connection
                .send_request(req)
                .block_task()
                .await
                .map_err(|e| anyhow!("session/list failed: {e}"))?;
            // Populate cwd cache while we have the data.
            let mut cwd_cache = self.session_cwd.lock().await;
            for info in &resp.sessions {
                cwd_cache.insert(info.session_id.to_string(), info.cwd.clone());
            }
            drop(cwd_cache);
            all.extend(resp.sessions);
            cursor = resp.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(all)
    }

    pub async fn list_sessions(&self) -> Vec<SessionMeta> {
        let infos = match self.fetch_all_from_acp().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "failed to list sessions from ACP");
                return vec![];
            }
        };
        let archived = self.archived.lock().await;
        let live = self.live_sessions.lock().await;
        let modes = self.session_modes.lock().await;

        let mut sessions: Vec<SessionMeta> = infos
            .into_iter()
            .filter(|s| !archived.contains(&s.session_id.to_string()))
            .map(|s| session_meta_from_info(s, &live, &modes))
            .collect();

        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        sessions
    }

    /// Returns the count of sessions currently running a prompt.
    /// Cheap (no network) — used by the internal /processes endpoint.
    pub async fn live_session_count(&self) -> usize {
        self.live_sessions.lock().await.len()
    }

    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        self.history.lock().await.remove(session_id);
        self.channels.lock().await.remove(session_id);
        self.live_sessions.lock().await.remove(session_id);
        self.session_modes.lock().await.remove(session_id);
        self.session_cwd.lock().await.remove(session_id);
        self.archived.lock().await.insert(session_id.to_string());
        self.persist_archived().await
    }

    async fn persist_archived(&self) -> Result<()> {
        let set = self.archived.lock().await.clone();
        let data = serde_json::to_vec_pretty(&set)?;
        if let Some(parent) = self.archived_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.archived_path, data).await?;
        Ok(())
    }

    pub async fn create_session(
        &self,
        label: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<SessionMeta> {
        let connection = self.connection().await?;
        let cwd = cwd.unwrap_or_else(|| self.cwd.clone());
        let response = connection
            .send_request(NewSessionRequest::new(cwd.clone()))
            .block_task()
            .await
            .map_err(|e| anyhow!("session/new failed: {e}"))?;
        let id = response.session_id.to_string();
        let (mode_id, available_modes) = split_modes(response.modes);
        self.session_cwd
            .lock()
            .await
            .insert(id.clone(), cwd.clone());
        self.session_modes
            .lock()
            .await
            .insert(id.clone(), (mode_id.clone(), available_modes.clone()));
        self.live_sessions.lock().await.insert(id.clone());
        Ok(SessionMeta {
            label: label.unwrap_or_else(|| "New session".to_string()),
            cwd: cwd.to_string_lossy().into_owned(),
            updated_at: None,
            id,
            status: SessionStatus::Live,
            mode_id,
            available_modes,
        })
    }

    /// Attaches to a session for WS streaming.
    ///
    /// On reconnect within the same process lifetime, returns in-memory history
    /// directly (no `session/load` needed).
    ///
    /// On first attach (e.g. after a pod restart), resolves the session's cwd
    /// via `session/list` then calls `session/load` so the ACP agent replays
    /// the on-disk JSONL transcript.
    pub async fn attach(
        &self,
        session_id: &str,
    ) -> Result<(Vec<AcpWsEvent>, broadcast::Receiver<AcpWsEvent>)> {
        let connection = self.connection().await?;
        let tx = self.channel(session_id).await;

        // Reconnect: history is already in memory; no session/load needed.
        let history_snapshot = {
            let hist = self.history.lock().await;
            let rx = tx.subscribe();
            let snapshot = hist.get(session_id).cloned();
            drop(hist);
            snapshot.map(|s| (s, rx))
        };
        if let Some((snapshot, rx)) = history_snapshot {
            self.live_sessions
                .lock()
                .await
                .insert(session_id.to_string());
            return Ok((snapshot, rx));
        }

        // First attach: resolve cwd from cache (populated by a prior
        // list_sessions call) or fetch fresh via session/list.
        let cwd = {
            let cache = self.session_cwd.lock().await;
            cache.get(session_id).cloned()
        };
        let cwd = match cwd {
            Some(c) => c,
            None => {
                self.fetch_all_from_acp().await?;
                self.session_cwd
                    .lock()
                    .await
                    .get(session_id)
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown session"))?
            }
        };

        // Subscribe before session/load so broadcast captures the replayed history.
        let rx = tx.subscribe();
        let response = connection
            .send_request(LoadSessionRequest::new(session_id.to_string(), cwd))
            .block_task()
            .await
            .map_err(|e| anyhow!("session/load failed: {e}"))?;

        let (mode_id, available_modes) = split_modes(response.modes);
        self.session_modes
            .lock()
            .await
            .insert(session_id.to_string(), (mode_id, available_modes));
        self.live_sessions
            .lock()
            .await
            .insert(session_id.to_string());

        Ok((vec![], rx))
    }

    pub async fn handle_client_msg(&self, session_id: &str, msg: AcpWsClientMsg) -> Result<()> {
        let connection = self.connection().await?;
        match msg {
            AcpWsClientMsg::SendMessage { text } => {
                let connection = connection.clone();
                let session_id = session_id.to_string();
                let live_sessions = self.live_sessions.clone();
                let channels = self.channels.clone();
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
                    live_sessions.lock().await.remove(&session_id);
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
                {
                    let mut modes = self.session_modes.lock().await;
                    if let Some((mid, _)) = modes.get_mut(session_id) {
                        *mid = Some(mode_id.clone());
                    } else {
                        modes.insert(session_id.to_string(), (Some(mode_id.clone()), Vec::new()));
                    }
                }
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

// ---------------------------------------------------------------------------
// ACP process lifecycle
// ---------------------------------------------------------------------------

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
