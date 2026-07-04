//! Simplified WS event schema bridging ACP's JSON-RPC session/update
//! notifications to what the browser needs. Keeps the browser dumb: it never
//! sees raw ACP JSON-RPC, only these tagged events.

use agent_client_protocol::schema::v1::{
    Plan, PlanEntryPriority, PlanEntryStatus, RequestPermissionRequest, SessionConfigKind,
    SessionConfigOption, SessionConfigSelectOptions, SessionUpdate, ToolCallContent,
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
    ToolCallCreated {
        session_id: String,
        tool_call_id: String,
        kind: &'static str,
        title: String,
        status: &'static str,
    },
    ToolCallUpdated {
        session_id: String,
        tool_call_id: String,
        status: Option<&'static str>,
        diff: Option<DiffPayload>,
        output: Option<String>,
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
        SessionUpdate::ToolCall(tc) => vec![AcpWsEvent::ToolCallCreated {
            session_id,
            tool_call_id: tc.tool_call_id.to_string(),
            kind: tool_kind_str(tc.kind),
            title: tc.title,
            status: tool_status_str(tc.status),
        }],
        SessionUpdate::ToolCallUpdate(update) => {
            let content = update.fields.content.as_deref();
            vec![AcpWsEvent::ToolCallUpdated {
                session_id,
                tool_call_id: update.tool_call_id.to_string(),
                status: update.fields.status.map(tool_status_str),
                diff: content.and_then(first_diff),
                output: content.and_then(content_text),
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
        _ => vec![],
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

fn content_block_text(content: &agent_client_protocol::schema::v1::ContentBlock) -> String {
    use agent_client_protocol::schema::v1::ContentBlock;
    match content {
        ContentBlock::Text(t) => t.text.clone(),
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
