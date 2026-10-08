//! Typed CTRL messages (contracts §4). JSON objects tagged by `"type"`.
//!
//! Unknown fields are ignored on decode (additive evolution); unknown `type`
//! values are a decode error and should be answered with `error`
//! (`unknown_message`).

use serde::{Deserialize, Serialize};

/// Who is connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Ui,
    Proxy,
    Executor,
}

/// Channel kinds that can be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Pty,
}

/// Credential kinds reported by `credential_captured` and `credential_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ClaudeOauth,
}

/// Run lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// Final outcome in the run-finished callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// Standard `error.code` values.
pub mod error_code {
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    pub const UNKNOWN_MESSAGE: &str = "unknown_message";
    pub const PROTOCOL_VIOLATION: &str = "protocol_violation";
    pub const FLOW_CONTROL: &str = "flow_control";
    pub const UNAUTHORIZED: &str = "unauthorized";
    pub const UNKNOWN_CHANNEL: &str = "unknown_channel";
    pub const UNKNOWN_RUN: &str = "unknown_run";
    pub const INTERNAL: &str = "internal";
    /// The peer does not implement the request (e.g. `run_*` without a run manager).
    pub const UNIMPLEMENTED: &str = "unimplemented";
}

/// Body of `run_start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStart {
    pub run_id: String,
    pub repo_url: String,
    pub base_ref: String,
    pub branch: String,
    pub push: bool,
    pub clone_token: String,
    pub prompt: String,
    pub timeout_secs: u64,
    pub callback_url: String,
    pub callback_token: String,
    /// Client-allocated channel on which the server streams the transcript as
    /// `DATA_JSON`, or `null` for none. Serialized as `null` when absent.
    #[serde(default)]
    pub log_channel: Option<u32>,
}

/// Body POSTed to `callback_url` when a run ends. Not a CTRL message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFinished {
    pub run_id: String,
    pub outcome: RunOutcome,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub summary: String,
    pub branch: String,
    pub head_sha: String,
    pub pushed: bool,
}

/// Every CTRL message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ctrl {
    Hello {
        versions: Vec<u16>,
        client: ClientKind,
    },
    Welcome {
        version: u16,
    },
    Open {
        channel: u32,
        kind: ChannelKind,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Vec<String>>,
    },
    Opened {
        channel: u32,
    },
    OpenError {
        channel: u32,
        message: String,
    },
    Resize {
        channel: u32,
        cols: u16,
        rows: u16,
    },
    RunStart(RunStart),
    RunAccepted {
        run_id: String,
        state: RunPhase,
        existing: bool,
    },
    RunStatus {
        run_id: String,
    },
    RunState {
        run_id: String,
        state: RunPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    RunCancel {
        run_id: String,
    },
    CredentialCaptured {
        kind: CredentialKind,
    },
    /// Client asks whether the sandbox holds a credential; answered by
    /// `credential_state`. No fields.
    CredentialStatus,
    /// Reply to `credential_status` (the sandbox's current view).
    CredentialState {
        kind: CredentialKind,
        configured: bool,
    },
    Error {
        code: String,
        message: String,
        /// Run this error answers; preferred for attributing it to a pending
        /// `run_start`/`run_status`. Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        /// Channel this error concerns, when known. Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        channel: Option<u32>,
    },
}
