//! ACP client plumbing shared by the TUI and the headless runner.

use std::path::Path;

use anyhow::{Result, anyhow};
use ez_acp::Connection;
use ez_acp::schema::{self, SessionMode, SessionModeState};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default)]
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AskRequest {
    #[serde(rename = "sessionId", default)]
    pub session_id: String,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub question: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AskResponse {
    pub answers: Vec<Answer>,
    pub cancelled: bool,
}

impl AskResponse {
    pub fn cancelled() -> Self {
        Self {
            answers: vec![],
            cancelled: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionEntry {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub first_prompt: String,
    pub mode: String,
}

pub struct Started {
    pub session_id: String,
    pub modes: Option<SessionModeState>,
}

pub async fn initialize(conn: &Connection) -> Result<()> {
    let req = schema::InitializeRequest::new(ez_acp::PROTOCOL_VERSION)
        .client_capabilities(schema::ez_client_capabilities())
        .client_info(schema::Implementation::new("ez", env!("CARGO_PKG_VERSION")));
    let _: schema::InitializeResponse = conn
        .request_typed(schema::INITIALIZE, &req)
        .await
        .map_err(|e| anyhow!("initialize failed: {e}"))?;
    Ok(())
}

pub async fn new_session(conn: &Connection, cwd: &Path) -> Result<Started> {
    let r: schema::NewSessionResponse = conn
        .request_typed(
            schema::SESSION_NEW,
            &schema::NewSessionRequest::new(cwd.to_path_buf()),
        )
        .await
        .map_err(|e| anyhow!("session/new failed: {e}"))?;
    Ok(Started {
        session_id: r.session_id.to_string(),
        modes: r.modes,
    })
}

pub async fn load_session(conn: &Connection, cwd: &Path, id: &str) -> Result<Started> {
    let r: schema::LoadSessionResponse = conn
        .request_typed(
            schema::SESSION_LOAD,
            &schema::LoadSessionRequest::new(schema::SessionId::new(id), cwd.to_path_buf()),
        )
        .await
        .map_err(|e| anyhow!("session/load failed: {e}"))?;
    Ok(Started {
        session_id: id.to_string(),
        modes: r.modes,
    })
}

pub async fn set_mode(conn: &Connection, session_id: &str, mode_id: &str) -> Result<()> {
    let req = schema::SetSessionModeRequest::new(
        schema::SessionId::new(session_id),
        schema::SessionModeId::new(mode_id),
    );
    let _: schema::SetSessionModeResponse = conn
        .request_typed(schema::SESSION_SET_MODE, &req)
        .await
        .map_err(|e| anyhow!("set_mode failed: {e}"))?;
    Ok(())
}

/// Pin (`Some`) or clear (`None`) the session model. Uses the `_ez/set_model` extension
/// because the standard `session/set_config_option` cannot clear a value.
pub async fn set_model(conn: &Connection, session_id: &str, model: Option<&str>) -> Result<()> {
    let req = schema::SetModelRequest {
        session_id: session_id.to_string(),
        model: model.map(String::from),
    };
    conn.request(schema::SET_MODEL, serde_json::to_value(req)?)
        .await
        .map_err(|e| anyhow!("set_model failed: {e}"))?;
    Ok(())
}

/// Sessions of `cwd` via the standard `session/list`, newest first.
pub async fn list_sessions(conn: &Connection, cwd: &Path) -> Result<Vec<SessionEntry>> {
    let req = schema::ListSessionsRequest::new().cwd(cwd.to_path_buf());
    let r: schema::ListSessionsResponse = conn
        .request_typed(schema::SESSION_LIST, &req)
        .await
        .map_err(|e| anyhow!("list sessions failed: {e}"))?;
    let mut sessions: Vec<SessionEntry> = r
        .sessions
        .into_iter()
        .map(|s| {
            let ext = s.meta.as_ref().and_then(|m| m.get("ez"));
            let field = |k: &str| {
                ext.and_then(|e| e.get(k))
                    .and_then(Value::as_str)
                    .map(String::from)
            };
            let updated_at = s.updated_at.clone().unwrap_or_default();
            SessionEntry {
                id: s.session_id.to_string(),
                created_at: field("createdAt").unwrap_or_else(|| updated_at.clone()),
                updated_at,
                first_prompt: field("firstPrompt").or(s.title).unwrap_or_default(),
                mode: field("mode").unwrap_or_default(),
            }
        })
        .collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(sessions)
}

/// Option id to answer a permission request: first of `kinds` present, else cancelled.
pub fn pick_option(
    req: &schema::RequestPermissionRequest,
    allow: bool,
) -> schema::RequestPermissionOutcome {
    use schema::PermissionOptionKind::*;
    let wanted: &[_] = if allow {
        &[AllowOnce, AllowAlways]
    } else {
        &[RejectOnce, RejectAlways]
    };
    for k in wanted {
        if let Some(o) = req.options.iter().find(|o| o.kind == *k) {
            return schema::RequestPermissionOutcome::Selected(
                schema::SelectedPermissionOutcome::new(o.option_id.clone()),
            );
        }
    }
    schema::RequestPermissionOutcome::Cancelled
}

pub fn mode_list(state: &Option<SessionModeState>) -> (Vec<SessionMode>, String) {
    match state {
        Some(s) => (s.available_modes.clone(), s.current_mode_id.to_string()),
        None => (vec![], String::new()),
    }
}
