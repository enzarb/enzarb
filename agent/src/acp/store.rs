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
    SessionNotification, SetSessionConfigOptionRequest, SetSessionModeRequest, TextContent,
};
use agent_client_protocol::{AcpAgent, Agent, Client, ConnectionTo};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Map as JsonMap;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, broadcast};

use crate::init::home_dir;

use super::events::{
    AcpWsClientMsg, AcpWsEvent, ConfigOptionPayload, config_payloads, from_session_update,
    permission_request_event, tool_kind_str,
};
use super::permissions::{PermissionRegistry, auto_allow};

const ARCHIVED_FILE: &str = ".enzarb/archived_sessions.json";
const PREFS_FILE: &str = ".enzarb/session_prefs.json";
const CHANNEL_CAPACITY: usize = 8192;
const ACP_COMMAND: &str = "claude-agent-acp";
/// Upper bound on metadata-style ACP requests (session/list, session/load).
/// A wedged ACP process otherwise hangs these forever, and every /agent
/// endpoint surfaces as a gateway 504. Prompt requests are NOT capped — they
/// legitimately run for minutes.
const ACP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
    pub config_options: Vec<ConfigOptionPayload>,
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<JsonMap<String, serde_json::Value>>,
}

/// User-chosen per-session settings persisted across pod restarts so they can
/// be reapplied after `session/load` (claude-agent-acp resets them otherwise).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionPrefs {
    mode_id: Option<String>,
    /// config option id -> selected value id (e.g. "model" -> "claude-opus-4-8")
    #[serde(default)]
    config: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

type Channels = Arc<Mutex<HashMap<String, broadcast::Sender<AcpWsEvent>>>>;
type History = Arc<Mutex<HashMap<String, Vec<AcpWsEvent>>>>;
type SessionModes = Arc<Mutex<HashMap<String, (Option<String>, Vec<SessionModeInfo>)>>>;
type SessionConfigs = Arc<Mutex<HashMap<String, Vec<ConfigOptionPayload>>>>;
type Prefs = Arc<Mutex<HashMap<String, SessionPrefs>>>;

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
    configs: &HashMap<String, Vec<ConfigOptionPayload>>,
) -> SessionMeta {
    let id = info.session_id.to_string();
    let status = if live.contains(&id) {
        SessionStatus::Live
    } else {
        SessionStatus::Idle
    };
    let (mode_id, available_modes) = modes.get(&id).cloned().unwrap_or_default();
    let config_options = configs.get(&id).cloned().unwrap_or_default();
    let label = info
        .title
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| id[..8.min(id.len())].to_string());
    SessionMeta {
        cwd: info.cwd.to_string_lossy().into_owned(),
        updated_at: info.updated_at,
        meta: info.meta,
        id,
        label,
        status,
        mode_id,
        available_modes,
        config_options,
    }
}

// ---------------------------------------------------------------------------
// AcpStore
// ---------------------------------------------------------------------------

/// A spawned ACP connection plus a liveness flag flipped to `false` once the
/// background task driving it exits (process died, pipe closed, etc.) — lets
/// `connection()` detect a dead connection and transparently respawn.
#[derive(Clone)]
struct ConnState {
    conn: ConnectionTo<Agent>,
    alive: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct AcpStore {
    connection: Arc<Mutex<Option<ConnState>>>,
    cwd: PathBuf,
    permissions: PermissionRegistry,
    channels: Channels,
    history: History,
    /// In-memory only: which sessions are currently running a prompt.
    /// Resets on pod restart — correct, since nothing is live after a restart.
    live_sessions: Arc<Mutex<HashSet<String>>>,
    /// Per-session mode info populated from `session/new` and `session/load`.
    session_modes: SessionModes,
    /// Per-session config options (model, etc.) from `session/new`/`session/load`.
    session_configs: SessionConfigs,
    /// Persisted per-session user prefs (mode, config values) reapplied after load.
    prefs: Prefs,
    prefs_path: PathBuf,
    /// cwd per session, populated from `session/new` and `session/list` results.
    session_cwd: Arc<Mutex<HashMap<String, PathBuf>>>,
    /// Persisted set of session IDs hidden from `session/list`.
    archived: Arc<Mutex<HashSet<String>>>,
    archived_path: PathBuf,
    /// Per-session locks serializing first attach. Two WS clients reconnecting
    /// after a pod restart must not both issue `session/load` for the same
    /// session: claude-agent-acp spawns a duplicate `claude --resume` and
    /// wedges, hanging every subsequent request.
    attach_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
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
        let prefs_path = home_dir().join(PREFS_FILE);
        let prefs = if prefs_path.exists() {
            let data = tokio::fs::read(&prefs_path).await?;
            serde_json::from_slice::<HashMap<String, SessionPrefs>>(&data).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self {
            connection: Arc::new(Mutex::new(None)),
            cwd,
            permissions: PermissionRegistry::default(),
            channels: Arc::new(Mutex::new(HashMap::new())),
            history: Arc::new(Mutex::new(HashMap::new())),
            live_sessions: Arc::new(Mutex::new(HashSet::new())),
            session_modes: Arc::new(Mutex::new(HashMap::new())),
            session_configs: Arc::new(Mutex::new(HashMap::new())),
            prefs: Arc::new(Mutex::new(prefs)),
            prefs_path,
            session_cwd: Arc::new(Mutex::new(HashMap::new())),
            archived: Arc::new(Mutex::new(archived)),
            archived_path,
            attach_locks: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Returns the current ACP connection, respawning `claude-agent-acp` if
    /// none exists yet or the previous one has died (process killed/crashed).
    /// Held behind a single mutex so concurrent callers racing a respawn
    /// single-flight onto the same new process rather than each spawning
    /// their own.
    async fn connection(&self) -> Result<ConnectionTo<Agent>> {
        let mut guard = self.connection.lock().await;
        if let Some(state) = guard.as_ref() {
            if state.alive.load(Ordering::Relaxed) {
                return Ok(state.conn.clone());
            }
            tracing::warn!("ACP connection is dead, respawning claude-agent-acp");
        }
        let (conn, alive) = spawn(
            self.permissions.clone(),
            self.channels.clone(),
            self.history.clone(),
            self.session_modes.clone(),
            self.session_configs.clone(),
        )
        .await?;
        *guard = Some(ConnState {
            conn: conn.clone(),
            alive,
        });
        Ok(conn)
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
            let resp = tokio::time::timeout(
                ACP_REQUEST_TIMEOUT,
                connection.send_request(req).block_task(),
            )
            .await
            .map_err(|_| anyhow!("session/list timed out"))?
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
        let configs = self.session_configs.lock().await;

        let mut sessions: Vec<SessionMeta> = infos
            .into_iter()
            .filter(|s| !archived.contains(&s.session_id.to_string()))
            .map(|s| session_meta_from_info(s, &live, &modes, &configs))
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
        self.session_configs.lock().await.remove(session_id);
        self.session_cwd.lock().await.remove(session_id);
        self.archived.lock().await.insert(session_id.to_string());
        if self.prefs.lock().await.remove(session_id).is_some() {
            let _ = self.persist_prefs().await;
        }
        self.persist_archived().await
    }

    async fn persist_prefs(&self) -> Result<()> {
        let map = self.prefs.lock().await.clone();
        let data = serde_json::to_vec_pretty(&map)?;
        if let Some(parent) = self.prefs_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&self.prefs_path, data).await?;
        Ok(())
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
        let config_options = config_payloads(response.config_options.as_deref().unwrap_or(&[]));
        self.session_cwd
            .lock()
            .await
            .insert(id.clone(), cwd.clone());
        self.session_modes
            .lock()
            .await
            .insert(id.clone(), (mode_id.clone(), available_modes.clone()));
        self.session_configs
            .lock()
            .await
            .insert(id.clone(), config_options.clone());
        self.live_sessions.lock().await.insert(id.clone());
        Ok(SessionMeta {
            label: label.unwrap_or_else(|| "New session".to_string()),
            cwd: cwd.to_string_lossy().into_owned(),
            updated_at: None,
            meta: None,
            id,
            status: SessionStatus::Live,
            mode_id,
            available_modes,
            config_options,
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

        // Serialize attaches per session: the first one performs the
        // session/load; concurrent ones wait here and then take the in-memory
        // history path below.
        let attach_lock = {
            let mut locks = self.attach_locks.lock().await;
            locks.entry(session_id.to_string()).or_default().clone()
        };
        let _attach_guard = attach_lock.lock().await;

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
        let response = tokio::time::timeout(
            ACP_REQUEST_TIMEOUT,
            connection
                .send_request(LoadSessionRequest::new(session_id.to_string(), cwd))
                .block_task(),
        )
        .await
        .map_err(|_| anyhow!("session/load timed out"))?
        .map_err(|e| anyhow!("session/load failed: {e}"))?;

        let (mode_id, available_modes) = split_modes(response.modes);
        let config_options = config_payloads(response.config_options.as_deref().unwrap_or(&[]));
        self.session_modes
            .lock()
            .await
            .insert(session_id.to_string(), (mode_id.clone(), available_modes));
        self.session_configs
            .lock()
            .await
            .insert(session_id.to_string(), config_options.clone());
        self.live_sessions
            .lock()
            .await
            .insert(session_id.to_string());

        // Reapply persisted user prefs: claude-agent-acp loads sessions with
        // its own defaults, dropping the mode/model the user picked earlier.
        self.reapply_prefs(session_id, mode_id, config_options)
            .await;

        Ok((vec![], rx))
    }

    /// Reapplies saved mode/config prefs after `session/load` when they differ
    /// from the loaded state, then broadcasts the resulting state so attached
    /// clients (which may have fetched meta before the load) sync up.
    async fn reapply_prefs(
        &self,
        session_id: &str,
        loaded_mode: Option<String>,
        loaded_configs: Vec<ConfigOptionPayload>,
    ) {
        let saved = self.prefs.lock().await.get(session_id).cloned();
        let mut mode_id = loaded_mode;
        let mut config_options = loaded_configs;

        if let Some(saved) = saved {
            let connection = match self.connection().await {
                Ok(c) => c,
                Err(_) => return,
            };
            if let Some(want_mode) = saved.mode_id
                && mode_id.as_deref() != Some(want_mode.as_str())
            {
                match connection
                    .send_request(SetSessionModeRequest::new(
                        session_id.to_string(),
                        want_mode.clone(),
                    ))
                    .block_task()
                    .await
                {
                    Ok(_) => {
                        mode_id = Some(want_mode.clone());
                        let mut modes = self.session_modes.lock().await;
                        if let Some((mid, _)) = modes.get_mut(session_id) {
                            *mid = Some(want_mode);
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, session_id, "failed to reapply session mode")
                    }
                }
            }
            for (config_id, want_value) in saved.config {
                let current = config_options
                    .iter()
                    .find(|o| o.id == config_id)
                    .map(|o| o.current_value.clone());
                if current.as_deref() == Some(want_value.as_str()) {
                    continue;
                }
                match connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        session_id.to_string(),
                        config_id.clone(),
                        want_value,
                    ))
                    .block_task()
                    .await
                {
                    Ok(resp) => {
                        config_options = config_payloads(&resp.config_options);
                        self.session_configs
                            .lock()
                            .await
                            .insert(session_id.to_string(), config_options.clone());
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, session_id, config_id, "failed to reapply session config option")
                    }
                }
            }
        }

        // Push the complete snapshot (including available_modes, which the
        // per-change events don't carry): the client's meta fetch on connect
        // races session/load and may have seen an empty mode list.
        let available_modes = self
            .session_modes
            .lock()
            .await
            .get(session_id)
            .map(|(_, modes)| modes.clone())
            .unwrap_or_default();
        let tx = self.channel(session_id).await;
        let _ = tx.send(AcpWsEvent::SessionState {
            session_id: session_id.to_string(),
            mode_id,
            available_modes,
            config_options,
        });
    }

    pub async fn handle_client_msg(&self, session_id: &str, msg: AcpWsClientMsg) -> Result<()> {
        let connection = self.connection().await?;
        match msg {
            AcpWsClientMsg::SendMessage { text } => {
                let connection = connection.clone();
                let session_id = session_id.to_string();
                let live_sessions = self.live_sessions.clone();
                let channels = self.channels.clone();
                let tx = self.channel(&session_id).await;
                let _ = tx.send(AcpWsEvent::TurnStatus {
                    session_id: session_id.clone(),
                    running: true,
                });
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
                    let tx = channels.lock().await.get(&session_id).cloned();
                    if let Some(tx) = tx {
                        let _ = tx.send(AcpWsEvent::TurnStatus {
                            session_id: session_id.clone(),
                            running: false,
                        });
                    }
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
                self.prefs
                    .lock()
                    .await
                    .entry(session_id.to_string())
                    .or_default()
                    .mode_id = Some(mode_id.clone());
                if let Err(e) = self.persist_prefs().await {
                    tracing::warn!(error = %e, "failed to persist session prefs");
                }
                let tx = self.channel(session_id).await;
                let _ = tx.send(AcpWsEvent::ModeChanged {
                    session_id: session_id.to_string(),
                    mode_id,
                });
                Ok(())
            }
            AcpWsClientMsg::SetConfigOption { config_id, value } => {
                let response = connection
                    .send_request(SetSessionConfigOptionRequest::new(
                        session_id.to_string(),
                        config_id.clone(),
                        value.clone(),
                    ))
                    .block_task()
                    .await
                    .map_err(|e| anyhow!("session/set_config_option failed: {e}"))?;
                let config_options = config_payloads(&response.config_options);
                self.session_configs
                    .lock()
                    .await
                    .insert(session_id.to_string(), config_options.clone());
                self.prefs
                    .lock()
                    .await
                    .entry(session_id.to_string())
                    .or_default()
                    .config
                    .insert(config_id, value);
                if let Err(e) = self.persist_prefs().await {
                    tracing::warn!(error = %e, "failed to persist session prefs");
                }
                let tx = self.channel(session_id).await;
                let _ = tx.send(AcpWsEvent::ConfigOptionsChanged {
                    session_id: session_id.to_string(),
                    config_options,
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
    session_modes: SessionModes,
    session_configs: SessionConfigs,
) -> Result<(ConnectionTo<Agent>, Arc<AtomicBool>)> {
    let agent = AcpAgent::from_str(ACP_COMMAND).map_err(|e| anyhow!("invalid ACP command: {e}"))?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let alive = Arc::new(AtomicBool::new(true));
    let alive_task = alive.clone();

    tokio::spawn(async move {
        let notification_channels = channels.clone();
        let notification_history = history.clone();
        let result = Client
            .builder()
            .on_receive_notification(
                move |notification: SessionNotification, _cx| {
                    let channels = notification_channels.clone();
                    let history = notification_history.clone();
                    let session_modes = session_modes.clone();
                    let session_configs = session_configs.clone();
                    async move {
                        let session_id = notification.session_id.to_string();
                        let events = from_session_update(&session_id, notification.update);
                        // Keep the meta caches current so REST fetches agree
                        // with what the agent pushed over the session channel.
                        for event in &events {
                            match event {
                                AcpWsEvent::ModeChanged { mode_id, .. } => {
                                    let mut modes = session_modes.lock().await;
                                    if let Some((mid, _)) = modes.get_mut(&session_id) {
                                        *mid = Some(mode_id.clone());
                                    }
                                }
                                AcpWsEvent::ConfigOptionsChanged { config_options, .. } => {
                                    session_configs
                                        .lock()
                                        .await
                                        .insert(session_id.clone(), config_options.clone());
                                }
                                _ => {}
                            }
                        }
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
                        // The ACP connection's incoming-message loop awaits each
                        // request handler inline before reading the next message
                        // (including responses to our own outgoing requests like
                        // session/list) — so a permission prompt that never gets
                        // answered (client disconnect, dropped WS send, user just
                        // never clicks) would otherwise wedge the whole connection
                        // for every session in this project. Spawn the wait off
                        // the dispatch loop so it can only ever block itself.
                        tokio::spawn(async move {
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

                            let result = match chosen_option {
                                Some(id) => responder.respond(RequestPermissionResponse::new(
                                    RequestPermissionOutcome::Selected(
                                        SelectedPermissionOutcome::new(id),
                                    ),
                                )),
                                None => responder.respond(RequestPermissionResponse::new(
                                    RequestPermissionOutcome::Cancelled,
                                )),
                            };
                            if let Err(e) = result {
                                tracing::warn!(error = %e, "failed to send permission response");
                            }
                        });
                        Ok(())
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

        alive_task.store(false, Ordering::Relaxed);
        if let Err(e) = result {
            tracing::warn!(error = %e, "ACP connection ended");
        }
    });

    let conn = ready_rx
        .await
        .map_err(|_| anyhow!("ACP agent process failed to initialize"))?;
    Ok((conn, alive))
}
