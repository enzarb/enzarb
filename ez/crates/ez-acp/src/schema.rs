//! ACP wire types.
//!
//! The protocol types are the official [`agent_client_protocol_schema`] crate (v1, stable
//! surface only, no `unstable_*` features), re-exported here so that `ez_acp::schema::*`
//! is the one import for everything on the wire. On top of that this module holds:
//!
//! - method-name constants usable in `match` arms (the crate only exposes them as struct fields;
//!   a unit test keeps the two in sync),
//! - the `_ez/*` extension types,
//! - small helpers: capability checks and lenient inbound parsing.
//!
//! Note: the official types are `#[non_exhaustive]`; build them with `Type::new(..)` plus the
//! builder methods and always keep a `_ =>` arm when matching enums.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use agent_client_protocol_schema::ProtocolVersion;
pub use agent_client_protocol_schema::v1::*;

// ---- standard method names (mirrors of AGENT_METHOD_NAMES / CLIENT_METHOD_NAMES) -------------

pub const INITIALIZE: &str = "initialize";
pub const AUTHENTICATE: &str = "authenticate";
pub const SESSION_NEW: &str = "session/new";
pub const SESSION_LOAD: &str = "session/load";
pub const SESSION_LIST: &str = "session/list";
pub const SESSION_RESUME: &str = "session/resume";
pub const SESSION_CLOSE: &str = "session/close";
pub const SESSION_PROMPT: &str = "session/prompt";
pub const SESSION_CANCEL: &str = "session/cancel";
pub const SESSION_SET_MODE: &str = "session/set_mode";
pub const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
pub const SESSION_UPDATE: &str = "session/update";
pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
pub const FS_READ_TEXT_FILE: &str = "fs/read_text_file";
pub const FS_WRITE_TEXT_FILE: &str = "fs/write_text_file";

// ---- ez extension method names ---------------------------------------------------------

pub const ASK_USER_QUESTION: &str = "_ez/ask_user_question";
/// Deprecated alias of the standard `session/list`.
pub const LIST_SESSIONS: &str = "_ez/list_sessions";
/// Pins the model for a session; also reachable as `session/set_config_option` with `model`.
pub const SET_MODEL: &str = "_ez/set_model";

/// Id of the `model` session config option.
pub const MODEL_CONFIG_ID: &str = "model";

// ---- extension types --------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AskUserQuestionRequest {
    pub session_id: String,
    pub questions: Vec<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AskUserQuestionResponse {
    pub answers: Vec<Value>,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsExtRequest {
    pub cwd: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsExtEntry {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub first_prompt: String,
    pub mode: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListSessionsExtResponse {
    pub sessions: Vec<ListSessionsExtEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelRequest {
    pub session_id: String,
    /// `None` (or absent) clears the pin.
    #[serde(default)]
    pub model: Option<String>,
}

// ---- helpers ----------------------------------------------------------------------------------

fn ez_meta_flag(caps: &ClientCapabilities, flag: &str) -> bool {
    caps.meta
        .as_ref()
        .and_then(|m| m.get("ez"))
        .and_then(|r| r.get(flag))
        .and_then(Value::as_bool)
        == Some(true)
}

/// Client advertised `_meta.ez.askUserQuestion = true`.
pub fn supports_ask_user_question(caps: &ClientCapabilities) -> bool {
    ez_meta_flag(caps, "askUserQuestion")
}

/// Client can render the standard `notice` session update: it advertised
/// `clientCapabilities.session.notices`, or `_meta.ez.notices = true`.
pub fn supports_notices(caps: &ClientCapabilities) -> bool {
    caps.session.as_ref().is_some_and(|s| s.notices.is_some()) || ez_meta_flag(caps, "notices")
}

/// Capabilities `ez` itself advertises: no fs/terminal, notices, and the ask-user extension.
pub fn ez_client_capabilities() -> ClientCapabilities {
    let Value::Object(meta) = serde_json::json!({"ez": {"askUserQuestion": true, "notices": true}})
    else {
        unreachable!()
    };
    ClientCapabilities::new()
        .terminal(false)
        .session(
            ClientSessionCapabilities::new()
                .notices(NoticeCapabilities::new())
                .config_options(SessionConfigOptionsCapabilities::new()),
        )
        .meta(meta)
}

/// Flatten prompt content blocks into the text sent to the model. Images, audio and binary
/// resources are dropped.
pub fn prompt_to_text(blocks: &[ContentBlock]) -> String {
    let parts: Vec<String> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.text.clone()),
            ContentBlock::ResourceLink(l) => Some(format!("@{}", l.uri)),
            ContentBlock::Resource(r) => match &r.resource {
                EmbeddedResourceResource::TextResourceContents(t) => {
                    Some(format!("<file uri={}>\n{}\n</file>", t.uri, t.text))
                }
                _ => None,
            },
            _ => None,
        })
        .collect();
    parts.join("\n")
}

/// Parse the `prompt` array item by item, skipping blocks of unknown type or shape (the official
/// type would fail the whole request on one unknown block).
pub fn parse_prompt_blocks(prompt: &Value) -> Vec<ContentBlock> {
    let Some(items) = prompt.as_array() else {
        return vec![];
    };
    items
        .iter()
        .filter_map(
            |item| match serde_json::from_value::<ContentBlock>(item.clone()) {
                Ok(b) => Some(b),
                Err(e) => {
                    tracing::debug!("skipping unsupported prompt content block: {e}");
                    None
                }
            },
        )
        .collect()
}

/// Make a request's params tolerant before typed parsing: `mcpServers` defaults to `[]`.
pub fn default_mcp_servers(params: &mut Value) {
    if let Some(m) = params.as_object_mut()
        && m.get("mcpServers").is_none_or(Value::is_null)
    {
        m.insert("mcpServers".into(), Value::Array(vec![]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn method_constants_match_official_names() {
        let a = &AGENT_METHOD_NAMES;
        let c = &CLIENT_METHOD_NAMES;
        assert_eq!(INITIALIZE, a.initialize);
        assert_eq!(AUTHENTICATE, a.authenticate);
        assert_eq!(SESSION_NEW, a.session_new);
        assert_eq!(SESSION_LOAD, a.session_load);
        assert_eq!(SESSION_LIST, a.session_list);
        assert_eq!(SESSION_RESUME, a.session_resume);
        assert_eq!(SESSION_CLOSE, a.session_close);
        assert_eq!(SESSION_PROMPT, a.session_prompt);
        assert_eq!(SESSION_CANCEL, a.session_cancel);
        assert_eq!(SESSION_SET_MODE, a.session_set_mode);
        assert_eq!(SESSION_SET_CONFIG_OPTION, a.session_set_config_option);
        assert_eq!(SESSION_UPDATE, c.session_update);
        assert_eq!(SESSION_REQUEST_PERMISSION, c.session_request_permission);
        assert_eq!(FS_READ_TEXT_FILE, c.fs_read_text_file);
        assert_eq!(FS_WRITE_TEXT_FILE, c.fs_write_text_file);
    }

    #[test]
    fn capability_helpers() {
        let none = ClientCapabilities::new();
        assert!(!supports_ask_user_question(&none));
        assert!(!supports_notices(&none));
        let ours = ez_client_capabilities();
        assert!(supports_ask_user_question(&ours));
        assert!(supports_notices(&ours));
        let i: InitializeRequest = serde_json::from_value(json!({
            "protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":true},"session":{"notices":{}}}
        }))
        .unwrap();
        assert!(i.client_capabilities.fs.read_text_file);
        assert!(supports_notices(&i.client_capabilities));
        assert!(!supports_ask_user_question(&i.client_capabilities));
        let j: InitializeRequest = serde_json::from_value(json!({
            "protocolVersion":1,
            "clientCapabilities":{"_meta":{"ez":{"notices":true}}}
        }))
        .unwrap();
        assert!(supports_notices(&j.client_capabilities));
    }

    #[test]
    fn prompt_parsing_skips_unknown_blocks() {
        let blocks = parse_prompt_blocks(&json!([
            {"type":"text","text":"a"},
            {"type":"resource_link","uri":"file:///x","name":"x"},
            {"type":"resource","resource":{"uri":"file:///y","text":"body","mimeType":"text/plain"}},
            {"type":"audio","data":"..","mimeType":"audio/wav"},
            {"type":"hologram","x":1},
            {"nonsense":true},
            {"type":"resource","resource":{"uri":"file:///z","blob":"AAAA"}}
        ]));
        assert_eq!(blocks.len(), 5);
        assert_eq!(
            prompt_to_text(&blocks),
            "a\n@file:///x\n<file uri=file:///y>\nbody\n</file>"
        );
        assert!(parse_prompt_blocks(&json!(null)).is_empty());
    }

    #[test]
    fn mcp_servers_default() {
        let mut p = json!({"cwd":"/p"});
        default_mcp_servers(&mut p);
        assert_eq!(p["mcpServers"], json!([]));
        let n: NewSessionRequest = serde_json::from_value(p).unwrap();
        assert!(n.mcp_servers.is_empty());
        let mut p = json!({"mcpServers":null});
        default_mcp_servers(&mut p);
        assert_eq!(p["mcpServers"], json!([]));
    }

    #[test]
    fn error_conversion() {
        let e: crate::rpc::RpcError = Error::method_not_found().into();
        assert_eq!(e.code, -32601);
    }
}
