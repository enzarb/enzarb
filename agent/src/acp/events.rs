//! Simplified WS event schema bridging ACP's JSON-RPC session/update
//! notifications to what the browser needs. Keeps the browser dumb: it never
//! sees raw ACP JSON-RPC, only these tagged events.

use agent_client_protocol::schema::v1::{
    Plan, PlanEntryPriority, PlanEntryStatus, RequestPermissionRequest, SessionConfigKind,
    SessionConfigOption, SessionConfigSelectOptions, SessionUpdate, StopReason, ToolCallContent,
    ToolCallStatus, ToolKind,
};
use serde::{Deserialize, Serialize};

use super::store::{SessionMeta, SessionModeInfo};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpWsEvent {
    SessionList {
        sessions: Vec<SessionMeta>,
    },
    SessionCreated {
        session: SessionMeta,
    },
    MessageChunk {
        session_id: String,
        role: &'static str,
        text: String,
    },
    /// The model's extended-thinking output, streamed the same way as
    /// MessageChunk. Kept as a separate event (rather than a role) so the UI
    /// can render it distinctly (e.g. collapsed/muted) instead of mixing it
    /// into the assistant's reply.
    ThoughtChunk {
        session_id: String,
        text: String,
    },
    ToolCallCreated {
        session_id: String,
        tool_call_id: String,
        kind: &'static str,
        title: String,
        status: &'static str,
        path: Option<String>,
        plan: Option<String>,
        command: Option<String>,
        /// Raw tool-call input (e.g. a search query, grep pattern, url) so the
        /// UI can show generically what any tool is doing, not just the
        /// cherry-picked plan/command/path fields above.
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    ToolCallUpdated {
        session_id: String,
        tool_call_id: String,
        status: Option<&'static str>,
        diff: Option<DiffPayload>,
        output: Option<String>,
        path: Option<String>,
        plan: Option<String>,
        command: Option<String>,
        /// Raw tool-call input, if the update carries a refreshed one. See
        /// `ToolCallCreated::input`.
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
    },
    PlanUpdate {
        session_id: String,
        entries: Vec<PlanEntryPayload>,
    },
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_call_id: String,
        title: String,
        options: Vec<PermissionOptionPayload>,
        plan: Option<String>,
    },
    PermissionResolved {
        session_id: String,
        request_id: String,
    },
    ModeChanged {
        session_id: String,
        mode_id: String,
    },
    ConfigOptionsChanged {
        session_id: String,
        config_options: Vec<ConfigOptionPayload>,
    },
    /// Full mode/config snapshot pushed after attach completes. The WS opens
    /// before `session/load` finishes, so a client fetching meta on connect
    /// races an empty mode cache — this event closes that gap.
    SessionState {
        session_id: String,
        mode_id: Option<String>,
        available_modes: Vec<SessionModeInfo>,
        config_options: Vec<ConfigOptionPayload>,
    },
    Error {
        session_id: Option<String>,
        message: String,
    },
    /// Whether a prompt turn is currently in flight, so the browser can swap
    /// its send button for a stop button.
    TurnStatus {
        session_id: String,
        running: bool,
    },
    /// Why the just-finished turn stopped (end_turn, max_tokens, refusal,
    /// cancelled, ...), sent alongside the `TurnStatus{running:false}` that
    /// already announces completion.
    TurnEnded {
        session_id: String,
        stop_reason: &'static str,
    },
    /// Context-window usage pushed periodically during a turn, plus the
    /// running session cost when the agent reports one. This is
    /// context-window fullness (`used`/`size`), not per-turn input/output
    /// token counts — ACP only exposes those behind an unstable feature this
    /// crate doesn't enable.
    UsageUpdate {
        session_id: String,
        used: u64,
        size: u64,
        cost_amount: Option<f64>,
        cost_currency: Option<String>,
    },
    /// Slash commands the agent currently supports (e.g. `/compact`), so the
    /// UI can offer them instead of the user having to know them by heart.
    AvailableCommandsUpdate {
        session_id: String,
        commands: Vec<AvailableCommandPayload>,
    },
    /// The agent renamed/retitled the session (e.g. after summarizing the
    /// first user message into a short title).
    SessionInfoUpdate {
        session_id: String,
        title: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct AvailableCommandPayload {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffPayload {
    pub path: String,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanEntryPayload {
    pub content: String,
    pub priority: &'static str,
    pub status: &'static str,
}

/// Simplified view of an ACP `SessionConfigOption` (select kind only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigOptionPayload {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub current_value: String,
    pub options: Vec<ConfigValuePayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValuePayload {
    pub value: String,
    pub name: String,
}

/// Flattens ACP config options to the select-kind payloads the browser renders.
pub fn config_payloads(options: &[SessionConfigOption]) -> Vec<ConfigOptionPayload> {
    options
        .iter()
        .filter_map(|opt| match &opt.kind {
            SessionConfigKind::Select(select) => Some(ConfigOptionPayload {
                id: opt.id.to_string(),
                name: opt.name.clone(),
                category: opt
                    .category
                    .as_ref()
                    .and_then(|c| serde_json::to_value(c).ok())
                    .and_then(|v| v.as_str().map(str::to_string)),
                current_value: select.current_value.to_string(),
                options: match &select.options {
                    SessionConfigSelectOptions::Ungrouped(opts) => opts
                        .iter()
                        .map(|o| ConfigValuePayload {
                            value: o.value.to_string(),
                            name: o.name.clone(),
                        })
                        .collect(),
                    SessionConfigSelectOptions::Grouped(groups) => groups
                        .iter()
                        .flat_map(|g| g.options.iter())
                        .map(|o| ConfigValuePayload {
                            value: o.value.to_string(),
                            name: o.name.clone(),
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            }),
            #[allow(unreachable_patterns)]
            _ => None,
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct PermissionOptionPayload {
    pub option_id: String,
    pub label: String,
    pub kind: &'static str,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpWsClientMsg {
    SendMessage {
        text: String,
    },
    PermissionResponse {
        request_id: String,
        option_id: String,
    },
    SetPermissionMode {
        mode_id: String,
    },
    SetConfigOption {
        config_id: String,
        value: String,
    },
    Cancel,
}

pub fn tool_kind_str(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit | ToolKind::Delete | ToolKind::Move => "edit",
        ToolKind::Execute => "execute",
        _ => "other",
    }
}

fn tool_status_str(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "pending",
        ToolCallStatus::InProgress => "running",
        ToolCallStatus::Completed => "completed",
        ToolCallStatus::Failed => "failed",
        _ => "pending",
    }
}

fn plan_priority_str(p: &PlanEntryPriority) -> &'static str {
    match p {
        PlanEntryPriority::High => "high",
        PlanEntryPriority::Medium => "medium",
        PlanEntryPriority::Low => "low",
        _ => "medium",
    }
}

fn plan_status_str(s: &PlanEntryStatus) -> &'static str {
    match s {
        PlanEntryStatus::Pending => "pending",
        PlanEntryStatus::InProgress => "in_progress",
        PlanEntryStatus::Completed => "completed",
        _ => "pending",
    }
}

fn first_diff(content: &[ToolCallContent]) -> Option<DiffPayload> {
    content.iter().find_map(|c| match c {
        ToolCallContent::Diff(d) => Some(DiffPayload {
            path: d.path.to_string_lossy().into_owned(),
            old_text: d.old_text.clone(),
            new_text: d.new_text.clone(),
        }),
        _ => None,
    })
}

/// Concatenated text of all `Content` blocks (terminal/tool output). `None`
/// when the update carries no text so the browser keeps what it already has.
fn content_text(content: &[ToolCallContent]) -> Option<String> {
    let text: String = content
        .iter()
        .filter_map(|c| match c {
            ToolCallContent::Content(block) => Some(content_block_text(&block.content)),
            _ => None,
        })
        .collect();
    (!text.is_empty()).then_some(text)
}

/// Extracts plan-mode plan markdown from a tool call's raw input. Claude Code
/// delivers the ExitPlanMode plan as `{"plan": "..."}` in `raw_input` rather
/// than as a content block, so it would otherwise never reach the browser.
fn plan_text(raw_input: Option<&serde_json::Value>) -> Option<String> {
    raw_input?.get("plan")?.as_str().map(str::to_string)
}

/// The shell command an "execute" tool call is running, so the UI can show
/// it live instead of waiting for output once the command finishes.
fn command_text(raw_input: Option<&serde_json::Value>) -> Option<String> {
    raw_input?.get("command")?.as_str().map(str::to_string)
}

/// The file path a read/edit tool call is acting on, so the UI can show
/// which file was touched instead of just a generic "Read"/"Edit" title.
fn first_location_path(
    locations: &[agent_client_protocol::schema::v1::ToolCallLocation],
) -> Option<String> {
    locations.first().map(|l| l.path.display().to_string())
}

/// Translates one ACP `SessionUpdate` notification into zero-or-more simplified
/// WS events. Variants not yet surfaced in the UI (usage, available commands,
/// mode changes, etc.) are intentionally dropped here rather than forwarded raw.
pub fn from_session_update(session_id: &str, update: SessionUpdate) -> Vec<AcpWsEvent> {
    let session_id = session_id.to_string();
    match update {
        SessionUpdate::UserMessageChunk(chunk) => vec![AcpWsEvent::MessageChunk {
            session_id,
            role: "user",
            text: content_block_text(&chunk.content),
        }],
        SessionUpdate::AgentMessageChunk(chunk) => vec![AcpWsEvent::MessageChunk {
            session_id,
            role: "assistant",
            text: content_block_text(&chunk.content),
        }],
        SessionUpdate::AgentThoughtChunk(chunk) => vec![AcpWsEvent::ThoughtChunk {
            session_id,
            text: content_block_text(&chunk.content),
        }],
        SessionUpdate::ToolCall(tc) => vec![AcpWsEvent::ToolCallCreated {
            session_id,
            tool_call_id: tc.tool_call_id.to_string(),
            kind: tool_kind_str(tc.kind),
            title: tc.title,
            status: tool_status_str(tc.status),
            path: first_location_path(&tc.locations),
            plan: plan_text(tc.raw_input.as_ref()),
            command: command_text(tc.raw_input.as_ref()),
            input: tc.raw_input.clone(),
        }],
        SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.as_deref();
            vec![AcpWsEvent::ToolCallUpdated {
                session_id,
                tool_call_id: update.tool_call_id.to_string(),
                status: update.fields.status.map(tool_status_str),
                diff: content.and_then(first_diff),
                output: content.and_then(content_text),
                path: update
                    .fields
                    .locations
                    .as_deref()
                    .and_then(first_location_path),
                plan: plan_text(update.fields.raw_input.as_ref()),
                command: command_text(update.fields.raw_input.as_ref()),
                input: update.fields.raw_input.clone(),
            }]
        }
        SessionUpdate::Plan(plan) => vec![AcpWsEvent::PlanUpdate {
            session_id,
            entries: plan_entries(&plan),
        }],
        SessionUpdate::CurrentModeUpdate(update) => vec![AcpWsEvent::ModeChanged {
            session_id,
            mode_id: update.current_mode_id.to_string(),
        }],
        SessionUpdate::ConfigOptionUpdate(update) => vec![AcpWsEvent::ConfigOptionsChanged {
            session_id,
            config_options: config_payloads(&update.config_options),
        }],
        SessionUpdate::UsageUpdate(usage) => vec![AcpWsEvent::UsageUpdate {
            session_id,
            used: usage.used,
            size: usage.size,
            cost_amount: usage.cost.as_ref().map(|c| c.amount),
            cost_currency: usage.cost.map(|c| c.currency),
        }],
        SessionUpdate::AvailableCommandsUpdate(update) => {
            vec![AcpWsEvent::AvailableCommandsUpdate {
                session_id,
                commands: update
                    .available_commands
                    .into_iter()
                    .map(|c| AvailableCommandPayload {
                        name: c.name,
                        description: c.description,
                    })
                    .collect(),
            }]
        }
        SessionUpdate::SessionInfoUpdate(update) => vec![AcpWsEvent::SessionInfoUpdate {
            session_id,
            title: update.title.take(),
        }],
        _ => vec![],
    }
}

pub fn stop_reason_str(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::MaxTurnRequests => "max_turn_requests",
        StopReason::Refusal => "refusal",
        StopReason::Cancelled => "cancelled",
        _ => "end_turn",
    }
}

fn plan_entries(plan: &Plan) -> Vec<PlanEntryPayload> {
    plan.entries
        .iter()
        .map(|e| PlanEntryPayload {
            content: e.content.clone(),
            priority: plan_priority_str(&e.priority),
            status: plan_status_str(&e.status),
        })
        .collect()
}

/// Renders a content block to displayable text. Non-text blocks (images,
/// audio, embedded/linked resources) have no inline representation in the
/// chat transcript yet, so they render as a bracketed placeholder rather than
/// silently vanishing.
fn content_block_text(content: &agent_client_protocol::schema::v1::ContentBlock) -> String {
    use agent_client_protocol::schema::v1::{ContentBlock, EmbeddedResourceResource};
    match content {
        ContentBlock::Text(t) => t.text.clone(),
        ContentBlock::Image(img) => format!("[image: {}]", img.mime_type),
        ContentBlock::Audio(audio) => format!("[audio: {}]", audio.mime_type),
        ContentBlock::ResourceLink(link) => format!("[resource: {}]", link.name),
        ContentBlock::Resource(res) => match &res.resource {
            EmbeddedResourceResource::TextResourceContents(t) => t.text.clone(),
            EmbeddedResourceResource::BlobResourceContents(b) => {
                format!("[resource: {}]", b.uri)
            }
            _ => "[resource]".to_string(),
        },
        _ => String::new(),
    }
}

pub fn permission_request_event(
    session_id: &str,
    request_id: &str,
    request: &RequestPermissionRequest,
) -> AcpWsEvent {
    AcpWsEvent::PermissionRequest {
        session_id: session_id.to_string(),
        request_id: request_id.to_string(),
        tool_call_id: request.tool_call.tool_call_id.to_string(),
        title: request.tool_call.fields.title.clone().unwrap_or_default(),
        plan: plan_text(request.tool_call.fields.raw_input.as_ref()),
        options: request
            .options
            .iter()
            .map(|o| PermissionOptionPayload {
                option_id: o.option_id.to_string(),
                label: o.name.clone(),
                kind: match o.kind {
                    agent_client_protocol::schema::v1::PermissionOptionKind::AllowOnce => {
                        "allow_once"
                    }
                    agent_client_protocol::schema::v1::PermissionOptionKind::AllowAlways => {
                        "allow_always"
                    }
                    agent_client_protocol::schema::v1::PermissionOptionKind::RejectOnce => {
                        "reject_once"
                    }
                    agent_client_protocol::schema::v1::PermissionOptionKind::RejectAlways => {
                        "reject_always"
                    }
                    _ => "reject_once",
                },
            })
            .collect(),
    }
}
