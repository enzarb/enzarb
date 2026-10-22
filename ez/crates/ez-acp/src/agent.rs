//! Serves a [`ez_core::Agent`] over ACP.
//!
//! # Extension: `_ez/ask_user_question` (agent -> client request)
//!
//! Sent only when the client advertised `clientCapabilities._meta.ez.askUserQuestion = true`.
//!
//! ```json
//! // params
//! {"sessionId": "...", "questions": [
//!   {"question": "Which DB?", "header": "DB", "multiSelect": false,
//!    "options": [{"label": "sqlite", "description": "..."}, {"label": "postgres", "description": ""}]}]}
//! // result
//! {"answers": [{"question": "Which DB?", "selected": ["sqlite"], "other": null}], "cancelled": false}
//! ```
//!
//! `other` carries free text when the user chose "Other". `cancelled: true` aborts the tool call.
//! Without the capability, each question is posed through `session/request_permission` instead.
//!
//! # Extension: `_ez/list_sessions` (client -> agent request, deprecated)
//!
//! `{"cwd": "/path"}` -> `{"sessions": [{"id", "createdAt", "updatedAt", "firstPrompt", "mode"}]}`.
//! Deprecated alias of the standard `session/list`.
//!
//! # Extension: `_ez/set_model` (client -> agent request)
//!
//! `{"sessionId": "...", "model": "claude-haiku-5-5"}` -> `{}`. Pins the model for this session,
//! overriding the mode's and profile's model (recorded in the transcript as `model_change`).
//! `model: null` (or absent) clears the pin. The standard `session/set_config_option` with the
//! `model` config option does the same (without the ability to clear).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use ez_core::{
    Agent, Client, ModeInfo, PermissionDecision, PermissionRequest, SessionEvent,
    SessionInfo as CoreSessionInfo, StopReason as CoreStop, ToolStatus,
};
use ez_mcp::McpServerConfig;
use ez_tools::{Answer, Question, ToolKind as CoreKind, UserBridge};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::rpc::{self, Connection, Handler, RpcError};
use crate::schema::{self as acp, *};
use agent_client_protocol as sdk;

struct Shared {
    agent: Arc<Agent>,
    conn: OnceLock<Connection>,
    caps: Mutex<ClientCapabilities>,
    /// Models offered per session (for `config_option_update`), filled when a session is
    /// created, loaded or resumed.
    models: Mutex<HashMap<String, Vec<String>>>,
}

impl Shared {
    fn conn(&self) -> anyhow::Result<&Connection> {
        self.conn
            .get()
            .ok_or_else(|| anyhow::anyhow!("connection not ready"))
    }

    fn update(&self, session_id: &str, update: SessionUpdate) {
        let Ok(conn) = self.conn() else { return };
        let n = SessionNotification::new(SessionId::new(session_id), update);
        if let Ok(v) = serde_json::to_value(&n) {
            // `notify` enqueues on an unbounded channel drained by a single writer task,
            // so notifications keep the order in which they were sent.
            let _ = conn.notify(SESSION_UPDATE, v);
        }
    }

    async fn select(
        &self,
        session_id: &str,
        tool_call: ToolCallUpdate,
        options: Vec<PermissionOption>,
    ) -> Option<String> {
        let conn = self.conn().ok()?;
        let req = RequestPermissionRequest::new(SessionId::new(session_id), tool_call, options);
        let resp: RequestPermissionResponse = conn
            .request_typed(SESSION_REQUEST_PERMISSION, &req)
            .await
            .ok()?;
        match resp.outcome {
            RequestPermissionOutcome::Selected(s) => Some(s.option_id.to_string()),
            _ => None,
        }
    }

    fn notices_supported(&self) -> bool {
        supports_notices(&self.caps.lock().unwrap())
    }

    fn model_option_for(&self, session_id: &str, current: &str) -> Option<SessionConfigOption> {
        let models = self
            .models
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .unwrap_or_default();
        model_option(current, &models)
    }

    fn convert_event(&self, session_id: &str, ev: SessionEvent) -> Vec<SessionUpdate> {
        match ev {
            SessionEvent::Notice(n) if self.notices_supported() => {
                vec![SessionUpdate::Notice(Notice::new(NoticeSeverity::Info, n))]
            }
            SessionEvent::ModelChanged(m) => self
                .model_option_for(session_id, &m)
                .map(|o| {
                    vec![SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(
                        vec![o],
                    ))]
                })
                .unwrap_or_default(),
            ev => vec![convert_event(ev)],
        }
    }
}

fn kind(k: CoreKind) -> ToolKind {
    match k {
        CoreKind::Read => ToolKind::Read,
        CoreKind::Edit => ToolKind::Edit,
        CoreKind::Delete => ToolKind::Delete,
        CoreKind::Move => ToolKind::Move,
        CoreKind::Search => ToolKind::Search,
        CoreKind::Execute => ToolKind::Execute,
        CoreKind::Think => ToolKind::Think,
        CoreKind::Fetch => ToolKind::Fetch,
        CoreKind::SwitchMode => ToolKind::SwitchMode,
        CoreKind::Other => ToolKind::Other,
    }
}

fn text(s: impl Into<String>) -> ContentBlock {
    ContentBlock::from(s.into())
}

/// The `model` select option: `current` first if it is not among `models`.
fn model_option(current: &str, models: &[String]) -> Option<SessionConfigOption> {
    if current.is_empty() {
        return None;
    }
    let mut ids: Vec<&str> = models.iter().map(String::as_str).collect();
    if !ids.contains(&current) {
        ids.insert(0, current);
    }
    let options: Vec<SessionConfigSelectOption> = ids
        .into_iter()
        .map(|m| SessionConfigSelectOption::new(m.to_string(), m))
        .collect();
    Some(
        SessionConfigOption::select(MODEL_CONFIG_ID, "Model", current.to_string(), options)
            .description("Model used for this session")
            .category(SessionConfigOptionCategory::Model),
    )
}

/// Event -> update for events that map one-to-one and need no connection state. (Notices are
/// sent as message chunks here; `Shared::convert_event` upgrades them for capable clients.)
fn convert_event(ev: SessionEvent) -> SessionUpdate {
    match ev {
        SessionEvent::UserText(t) => SessionUpdate::UserMessageChunk(ContentChunk::new(text(t))),
        SessionEvent::AgentText(t) => SessionUpdate::AgentMessageChunk(ContentChunk::new(text(t))),
        SessionEvent::AgentThought(t) => {
            SessionUpdate::AgentThoughtChunk(ContentChunk::new(text(t)))
        }
        SessionEvent::ToolCall {
            id,
            title,
            kind: k,
            locations,
            input,
        } => SessionUpdate::ToolCall(
            ToolCall::new(id, title)
                .kind(kind(k))
                .status(ToolCallStatus::Pending)
                .locations(
                    locations
                        .into_iter()
                        .map(|p| ToolCallLocation::new(PathBuf::from(p)))
                        .collect::<Vec<_>>(),
                )
                .raw_input(input),
        ),
        SessionEvent::ToolCallUpdate {
            id,
            status,
            output,
            edits,
        } => {
            let mut content = vec![];
            if let Some(o) = output.filter(|o| !o.is_empty()) {
                content.push(ToolCallContent::Content(Content::new(text(o))));
            }
            content.extend(edits.into_iter().map(|e| {
                ToolCallContent::Diff(Diff::new(e.path, e.new_text).old_text(e.old_text))
            }));
            let mut fields = ToolCallUpdateFields::new().status(match status {
                ToolStatus::Pending => ToolCallStatus::Pending,
                ToolStatus::InProgress => ToolCallStatus::InProgress,
                ToolStatus::Completed => ToolCallStatus::Completed,
                ToolStatus::Failed => ToolCallStatus::Failed,
            });
            // `content` replaces what the client shows, so an empty list must be omitted.
            if !content.is_empty() {
                fields = fields.content(content);
            }
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(id, fields))
        }
        SessionEvent::Plan(items) => SessionUpdate::Plan(Plan::new(
            items
                .into_iter()
                .map(|i| {
                    PlanEntry::new(
                        i.content,
                        PlanEntryPriority::Medium,
                        match i.status {
                            ez_tools::TodoStatus::Pending => PlanEntryStatus::Pending,
                            ez_tools::TodoStatus::InProgress => PlanEntryStatus::InProgress,
                            ez_tools::TodoStatus::Completed => PlanEntryStatus::Completed,
                        },
                    )
                })
                .collect::<Vec<_>>(),
        )),
        SessionEvent::ModeChanged(m) => SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(m)),
        SessionEvent::Notice(n) => {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(text(format!("\n> {n}\n"))))
        }
        SessionEvent::Usage {
            input_tokens,
            output_tokens,
            context_window,
        } => SessionUpdate::UsageUpdate(UsageUpdate::new(
            input_tokens + output_tokens,
            context_window,
        )),
        // Handled by `Shared::convert_event`; never reaches here.
        SessionEvent::ModelChanged(m) => {
            SessionUpdate::AgentMessageChunk(ContentChunk::new(text(format!("\n> model: {m}\n"))))
        }
    }
}

/// The ACP-facing implementation of the core `Client` / `UserBridge` traits.
struct Bridge(Arc<Shared>);

#[async_trait]
impl UserBridge for Bridge {
    async fn ask_user(
        &self,
        session_id: &str,
        questions: Vec<Question>,
    ) -> anyhow::Result<Vec<Answer>> {
        let sh = &self.0;
        if supports_ask_user_question(&sh.caps.lock().unwrap()) {
            let req = AskUserQuestionRequest {
                session_id: session_id.to_string(),
                questions: questions
                    .iter()
                    .map(serde_json::to_value)
                    .collect::<Result<_, _>>()?,
            };
            let resp: AskUserQuestionResponse = sh
                .conn()?
                .request_typed(ASK_USER_QUESTION, &req)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if resp.cancelled {
                anyhow::bail!("user cancelled");
            }
            return resp
                .answers
                .into_iter()
                .map(|a| serde_json::from_value(a).map_err(Into::into))
                .collect();
        }
        // Fallback: one permission prompt per question.
        let mut answers = vec![];
        for (qi, q) in questions.into_iter().enumerate() {
            let options = q
                .options
                .iter()
                .enumerate()
                .map(|(i, o)| {
                    PermissionOption::new(
                        format!("opt-{i}"),
                        o.label.clone(),
                        PermissionOptionKind::AllowOnce,
                    )
                })
                .collect();
            let tc = ToolCallUpdate::new(
                format!("ask-{session_id}-{qi}"),
                ToolCallUpdateFields::new()
                    .title(q.question.clone())
                    .kind(ToolKind::Other)
                    .status(ToolCallStatus::Pending),
            );
            let chosen = sh
                .select(session_id, tc, options)
                .await
                .and_then(|id| id.strip_prefix("opt-")?.parse::<usize>().ok())
                .and_then(|i| q.options.get(i))
                .ok_or_else(|| anyhow::anyhow!("user cancelled"))?;
            answers.push(Answer {
                question: q.question.clone(),
                selected: vec![chosen.label.clone()],
                other: None,
            });
        }
        Ok(answers)
    }
}

#[async_trait]
impl Client for Bridge {
    async fn request_permission(
        &self,
        session_id: &str,
        req: PermissionRequest,
    ) -> PermissionDecision {
        let mut options = vec![PermissionOption::new(
            "allow_once",
            "Allow",
            PermissionOptionKind::AllowOnce,
        )];
        if req.allow_always {
            options.push(PermissionOption::new(
                "allow_always",
                "Always allow",
                PermissionOptionKind::AllowAlways,
            ));
        }
        options.push(PermissionOption::new(
            "reject_once",
            "Reject",
            PermissionOptionKind::RejectOnce,
        ));
        let tc = ToolCallUpdate::new(
            req.tool_call_id,
            ToolCallUpdateFields::new()
                .title(req.title)
                .kind(kind(req.kind))
                .status(ToolCallStatus::Pending)
                .raw_input(req.input),
        );
        match self.0.select(session_id, tc, options).await.as_deref() {
            Some("allow_once") => PermissionDecision::AllowOnce,
            Some("allow_always") => PermissionDecision::AllowAlways,
            Some(id) if id.starts_with("reject") => PermissionDecision::Reject,
            _ => PermissionDecision::Cancelled,
        }
    }

    fn send_event(&self, session_id: &str, event: SessionEvent) {
        for u in self.0.convert_event(session_id, event) {
            self.0.update(session_id, u);
        }
    }
}

/// ACP request handler wrapping a core [`Agent`].
pub struct AcpAgent {
    shared: Arc<Shared>,
}

fn parse<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))
}

fn mode_state(info: &CoreSessionInfo) -> SessionModeState {
    SessionModeState::new(
        info.current_mode.clone(),
        info.modes
            .iter()
            .map(
                |ModeInfo {
                     id,
                     name,
                     description,
                 }| {
                    SessionMode::new(id.clone(), name.clone())
                        .description(Some(description.clone()).filter(|d| !d.is_empty()))
                },
            )
            .collect::<Vec<_>>(),
    )
}

/// ACP `mcpServers` entries as session MCP servers; unsupported ones (SSE) are skipped.
fn acp_mcp_servers(servers: &[Value]) -> BTreeMap<String, McpServerConfig> {
    servers
        .iter()
        .filter_map(|v| {
            let r = ez_mcp::from_acp(v);
            if r.is_none() {
                tracing::warn!("ignoring unsupported ACP MCP server entry: {v}");
            }
            r
        })
        .collect()
}

/// Take the raw `mcpServers` entries out of the params (leaving `[]` for the typed parse), so a
/// malformed or unsupported entry is skipped on its own instead of failing the request.
fn take_mcp_servers(params: &mut Value) -> Vec<Value> {
    let Some(m) = params.as_object_mut() else {
        return vec![];
    };
    match m.insert("mcpServers".into(), Value::Array(vec![])) {
        Some(Value::Array(a)) => a,
        _ => vec![],
    }
}

fn to_value<T: serde::Serialize>(v: &T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::internal(e.to_string()))
}

fn session_capabilities() -> SessionCapabilities {
    SessionCapabilities::new()
        .list(SessionListCapabilities::new())
        .resume(SessionResumeCapabilities::new())
        .close(SessionCloseCapabilities::new())
}

impl AcpAgent {
    pub fn new(agent: Arc<Agent>) -> Self {
        Self {
            shared: Arc::new(Shared {
                agent,
                conn: OnceLock::new(),
                caps: Mutex::default(),
                models: Mutex::default(),
            }),
        }
    }

    /// Provide the connection used for calls back to the client. Must be set once.
    pub fn set_connection(&self, conn: Connection) {
        let _ = self.shared.conn.set(conn);
    }

    fn client(&self) -> Arc<dyn Client> {
        Arc::new(Bridge(self.shared.clone()))
    }

    fn agent(&self) -> &Agent {
        &self.shared.agent
    }

    /// Remember the session's models and build its `configOptions`.
    fn config_options(&self, info: &CoreSessionInfo) -> Option<Vec<SessionConfigOption>> {
        self.shared
            .models
            .lock()
            .unwrap()
            .insert(info.id.clone(), info.models.clone());
        model_option(&info.model, &info.models).map(|o| vec![o])
    }

    fn list_sessions(&self, req: ListSessionsRequest) -> Result<Value, RpcError> {
        let cwd = match req.cwd {
            Some(c) => c,
            None => std::env::current_dir().map_err(|e| RpcError::internal(e.to_string()))?,
        };
        let sessions: Vec<acp::SessionInfo> = self
            .agent()
            .list_sessions(&cwd)?
            .into_iter()
            .map(|s| {
                let Value::Object(meta) = json!({"ez": {
                    "createdAt": s.created_at, "mode": s.mode, "firstPrompt": s.first_prompt}})
                else {
                    unreachable!()
                };
                acp::SessionInfo::new(SessionId::new(s.id), cwd.clone())
                    .title(session_title(&s.first_prompt))
                    .updated_at(s.updated_at)
                    .meta(meta)
            })
            .collect();
        to_value(&ListSessionsResponse::new(sessions))
    }
}

/// One-line title from the first prompt.
fn session_title(first_prompt: &str) -> Option<String> {
    let line = first_prompt.lines().find(|l| !l.trim().is_empty())?.trim();
    let mut t: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        t.push('…');
    }
    Some(t)
}

#[async_trait]
impl Handler for AcpAgent {
    async fn request(&self, method: &str, mut params: Value) -> Result<Value, RpcError> {
        match method {
            INITIALIZE => {
                // Lenient: a missing version means v1.
                if let Some(m) = params.as_object_mut() {
                    m.entry("protocolVersion").or_insert(json!(1));
                }
                let req: InitializeRequest = parse(params)?;
                if let Some(info) = &req.client_info {
                    tracing::debug!("ACP client: {} {}", info.name, info.version);
                }
                *self.shared.caps.lock().unwrap() = req.client_capabilities;
                to_value(
                    &InitializeResponse::new(req.protocol_version.min(crate::PROTOCOL_VERSION))
                        .agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(true)
                                .prompt_capabilities(
                                    PromptCapabilities::new()
                                        .image(false)
                                        .audio(false)
                                        .embedded_context(true),
                                )
                                .mcp_capabilities(McpCapabilities::new().http(true).sse(false))
                                .session_capabilities(session_capabilities()),
                        )
                        .agent_info(Implementation::new("ez", env!("CARGO_PKG_VERSION"))),
                )
            }
            AUTHENTICATE => Ok(json!({})),
            SESSION_NEW => {
                let mcp = take_mcp_servers(&mut params);
                let req: NewSessionRequest = parse(params)?;
                let info = self
                    .agent()
                    .new_session_with(req.cwd, None, acp_mcp_servers(&mcp))
                    .await?;
                let mut resp = NewSessionResponse::new(SessionId::new(info.id.as_str()))
                    .modes(mode_state(&info));
                if let Some(o) = self.config_options(&info) {
                    resp = resp.config_options(o);
                }
                to_value(&resp)
            }
            SESSION_LOAD => {
                let mcp = take_mcp_servers(&mut params);
                let req: LoadSessionRequest = parse(params)?;
                let info = self
                    .agent()
                    .load_session_with(
                        &req.session_id.0,
                        req.cwd,
                        self.client(),
                        acp_mcp_servers(&mcp),
                    )
                    .await?;
                let mut resp = LoadSessionResponse::new().modes(mode_state(&info));
                if let Some(o) = self.config_options(&info) {
                    resp = resp.config_options(o);
                }
                to_value(&resp)
            }
            SESSION_RESUME => {
                let mcp = take_mcp_servers(&mut params);
                let req: ResumeSessionRequest = parse(params)?;
                let info = self
                    .agent()
                    .resume_session_with(&req.session_id.0, req.cwd, acp_mcp_servers(&mcp))
                    .await?;
                let mut resp = ResumeSessionResponse::new().modes(mode_state(&info));
                if let Some(o) = self.config_options(&info) {
                    resp = resp.config_options(o);
                }
                to_value(&resp)
            }
            SESSION_LIST => self.list_sessions(parse(params)?),
            SESSION_CLOSE => {
                let req: CloseSessionRequest = parse(params)?;
                self.agent().close_session(&req.session_id.0)?;
                self.shared
                    .models
                    .lock()
                    .unwrap()
                    .remove(&*req.session_id.0);
                to_value(&CloseSessionResponse::new())
            }
            SESSION_PROMPT => {
                // Parsed by hand so that unknown content blocks are skipped, not fatal.
                let session_id: SessionId = parse(params["sessionId"].clone())?;
                let blocks = parse_prompt_blocks(&params["prompt"]);
                let stop = self
                    .agent()
                    .prompt(&session_id.0, prompt_to_text(&blocks), self.client())
                    .await?;
                to_value(&PromptResponse::new(match stop {
                    CoreStop::EndTurn => StopReason::EndTurn,
                    CoreStop::MaxTokens => StopReason::MaxTokens,
                    CoreStop::MaxTurnRequests => StopReason::MaxTurnRequests,
                    CoreStop::Cancelled => StopReason::Cancelled,
                    CoreStop::Refusal => StopReason::Refusal,
                }))
            }
            SESSION_SET_MODE => {
                let req: SetSessionModeRequest = parse(params)?;
                // The agent emits `ModeChanged`, which the bridge forwards as current_mode_update.
                self.agent()
                    .set_mode(&req.session_id.0, &req.mode_id.0, self.client())
                    .await?;
                to_value(&SetSessionModeResponse::new())
            }
            SESSION_SET_CONFIG_OPTION => {
                let req: SetSessionConfigOptionRequest = parse(params)?;
                if &*req.config_id.0 != MODEL_CONFIG_ID {
                    return Err(RpcError::invalid_params(format!(
                        "unknown config option `{}`",
                        req.config_id
                    )));
                }
                let model = req
                    .value
                    .as_value_id()
                    .map(|v| v.to_string())
                    .ok_or_else(|| RpcError::invalid_params("`model` takes a model id"))?;
                self.agent()
                    .set_session_model(&req.session_id.0, Some(model.clone()), self.client())
                    .await?;
                let options = self
                    .shared
                    .model_option_for(&req.session_id.0, &model)
                    .into_iter()
                    .collect();
                to_value(&SetSessionConfigOptionResponse::new(options))
            }
            SET_MODEL => {
                let req: SetModelRequest = parse(params)?;
                self.agent()
                    .set_session_model(&req.session_id, req.model, self.client())
                    .await?;
                Ok(json!({}))
            }
            LIST_SESSIONS => {
                let req: ListSessionsExtRequest = parse(params)?;
                let sessions = self
                    .agent()
                    .list_sessions(&req.cwd)?
                    .into_iter()
                    .map(|s| ListSessionsExtEntry {
                        id: s.id,
                        created_at: s.created_at,
                        updated_at: s.updated_at,
                        first_prompt: s.first_prompt,
                        mode: s.mode,
                    })
                    .collect();
                to_value(&ListSessionsExtResponse { sessions })
            }
            m => Err(RpcError::method_not_found(m)),
        }
    }

    async fn notification(&self, method: &str, params: Value) {
        if method == SESSION_CANCEL
            && let Ok(n) = serde_json::from_value::<CancelNotification>(params)
        {
            self.agent().cancel(&n.session_id.0);
        }
    }
}

/// Serve over arbitrary streams; returns when the peer disconnects.
pub async fn serve<R, W>(agent: Arc<Agent>, reader: R, writer: W) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    serve_transport(agent, rpc::lines_transport(reader, writer)).await
}

async fn serve_transport(
    agent: Arc<Agent>,
    transport: impl sdk::ConnectTo<sdk::Agent> + 'static,
) -> anyhow::Result<()> {
    let acp = Arc::new(AcpAgent::new(agent));
    let (link, conn) = Connection::link();
    acp.set_connection(conn);
    rpc::run(sdk::Agent, transport, acp, link, None)
        .await
        .map_err(|e| anyhow::anyhow!("acp connection: {e}"))
}

pub async fn serve_stdio(agent: Arc<Agent>) -> anyhow::Result<()> {
    serve(agent, tokio::io::stdin(), tokio::io::stdout()).await
}

/// Wire an agent to a client-side handler in process (over an in-memory byte pipe, so the wire
/// is the same as over stdio); returns the client-side connection.
pub fn spawn_in_process(agent: Arc<Agent>, client_handler: Arc<dyn Handler>) -> Connection {
    let (a, b) = tokio::io::duplex(1024 * 1024);
    let (ar, aw) = tokio::io::split(a);
    let (br, bw) = tokio::io::split(b);
    tokio::spawn(async move {
        let _ = serve(agent, ar, aw).await;
    });
    rpc::spawn_client(rpc::lines_transport(br, bw), client_handler)
}

/// Wire an agent to a raw client end in process: the returned sender takes ACP JSON-RPC lines
/// (no trailing newline) destined for the agent, and the receiver yields the agent's lines.
/// For hosts that relay the protocol verbatim (e.g. a webview). Dropping the sender ends the
/// connection; the receiver then closes.
pub fn spawn_in_process_raw(
    agent: Arc<Agent>,
) -> (
    tokio::sync::mpsc::UnboundedSender<String>,
    tokio::sync::mpsc::UnboundedReceiver<String>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let (a, b) = tokio::io::duplex(1024 * 1024);
    let (ar, aw) = tokio::io::split(a);
    let (br, mut bw) = tokio::io::split(b);
    tokio::spawn(async move {
        let _ = serve(agent, ar, aw).await;
    });
    let (to_agent, mut from_host) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (to_host, from_agent) = tokio::sync::mpsc::unbounded_channel::<String>();
    tokio::spawn(async move {
        while let Some(line) = from_host.recv().await {
            let mut buf = line.into_bytes();
            buf.push(b'\n');
            if bw.write_all(&buf).await.is_err() || bw.flush().await.is_err() {
                break;
            }
        }
        let _ = bw.shutdown().await;
    });
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(br).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() && to_host.send(line).is_err() {
                break;
            }
        }
    });
    (to_agent, from_agent)
}
