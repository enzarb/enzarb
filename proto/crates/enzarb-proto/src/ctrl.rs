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
    /// ACP's newline-delimited JSON-RPC as a byte stream on `DATA_BIN`.
    /// Senders must only use it after `welcome.features` contained
    /// [`feature::ACP`] (an old proxy closes the socket on an unknown kind).
    Acp,
    /// Any kind this implementation does not know. Decode-only: it is never
    /// sent. A server answers it with `open_error` (`unsupported_kind`).
    #[serde(other)]
    Unknown,
}

/// Feature names advertised in `welcome.features`.
pub mod feature {
    /// The `acp` channel kind is available.
    pub const ACP: &str = "acp";
    /// Client feature (`hello.features`): the client stores per-user Claude
    /// tokens from `credential_captured.token` and never relays them, and acks
    /// `legacy` captures. Only sandbox-proxy sends it.
    pub const CLAUDE_USER_TOKEN: &str = "claude_user_token";
    /// Decoders keep at most this many features...
    pub const MAX_FEATURES: usize = 64;
    /// ...and drop any longer than this many bytes.
    pub const MAX_FEATURE_LEN: usize = 64;
}

/// Standard `open_error.code` values. Unknown codes must be tolerated.
pub mod open_error_code {
    pub const CHAT_DISABLED: &str = "chat_disabled";
    pub const CHAT_LIMIT: &str = "chat_limit";
    pub const NO_CREDENTIAL: &str = "no_credential";
    pub const REPO_NOT_CLONED: &str = "repo_not_cloned";
    /// The `open.kind` is not one this peer implements.
    pub const UNSUPPORTED_KIND: &str = "unsupported_kind";
    /// `open.repo`/`open.resume` exceeds [`MAX_REPO_LEN`]/[`MAX_RESUME_LEN`].
    pub const INVALID_REQUEST: &str = "invalid_request";
}

/// Longest `open.repo` (bytes) senders may send and receivers must accept.
pub const MAX_REPO_LEN: usize = 2048;
/// Longest `open.resume` (bytes) senders may send and receivers must accept.
pub const MAX_RESUME_LEN: usize = 256;

/// Typical `CLOSE` reasons on an `acp` channel (besides `exit:<n>`/`signal:<n>`).
/// Receivers must tolerate any other text.
pub mod close_reason {
    /// No traffic for the idle limit.
    pub const IDLE: &str = "idle";
    /// Replaced by a newer channel ("last tab wins").
    pub const SUPERSEDED: &str = "superseded";
    /// The sandbox is shutting down.
    pub const SHUTDOWN: &str = "shutdown";
    /// A size or lifetime limit was hit.
    pub const LIMIT: &str = "limit";
}

fn features_is_empty(f: &Option<Vec<String>>) -> bool {
    f.as_ref().is_none_or(|v| v.is_empty())
}

/// Drop oversized/excess entries from a peer-supplied feature list.
pub fn sanitize_features(features: Option<Vec<String>>) -> Vec<String> {
    features
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !f.is_empty() && f.len() <= feature::MAX_FEATURE_LEN)
        .take(feature::MAX_FEATURES)
        .collect()
}

/// A secret string (token). Serializes as a plain JSON string; `Debug`
/// redacts it. Read it with [`Secret::expose`]; there is deliberately no
/// `Deref`/`Display`, so it cannot be formatted by accident.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The secret value. Do not log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Self(s.into())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Known `credential_state.scope` values. Unknown values must be tolerated.
pub mod credential_scope {
    /// The credential is the user's own (per-user Claude token).
    pub const USER: &str = "user";
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
///
/// `Debug` is hand-written and redacts every token field.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Optional read-only token (`contents: read`) for fetching private git
    /// dependencies from the same Project. Never used for push. Omitted when
    /// absent; old peers ignore it and old payloads decode without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deps_token: Option<String>,
    /// Optional long-lived token with `read:packages` for installing private
    /// npm packages (exposed to the run process as `NODE_AUTH_TOKEN`). Never
    /// persisted by implementations. Omitted when absent; old peers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_token: Option<String>,
    /// Optional per-user Claude token for the run. Omitted when absent; old
    /// peers ignore it and old payloads decode without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_token: Option<Secret>,
    /// When present the run is not an agent run: the sandbox rebases its own
    /// child branch onto `onto` and pushes with an explicit lease (stacked
    /// change requests). `push`, `branch` and `base_ref` are ignored for
    /// pushing. Omitted when absent; old peers ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retarget: Option<Retarget>,
}

/// Parameters of a retarget run (`RunStart.retarget`): after a parent change
/// request was squash-merged, `git rebase --onto origin/<onto> <old_base>` of
/// the child branch, then a force-with-lease push of exactly that branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retarget {
    /// The child branch to rebase and push (an enzarb-created branch).
    pub head_ref: String,
    /// The branch to rebase onto (the repository's default branch).
    pub onto: String,
    /// Full sha of the parent's head the child was cut from.
    pub old_base: String,
    /// Full sha the child's remote head must have; the push lease.
    pub expected_sha: String,
    /// Branch names that must never be rewritten. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protected: Vec<String>,
}

impl std::fmt::Debug for RunStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const R: &str = "<redacted>";
        f.debug_struct("RunStart")
            .field("run_id", &self.run_id)
            .field("repo_url", &self.repo_url)
            .field("base_ref", &self.base_ref)
            .field("branch", &self.branch)
            .field("push", &self.push)
            .field("clone_token", &R)
            .field("prompt", &self.prompt)
            .field("timeout_secs", &self.timeout_secs)
            .field("callback_url", &self.callback_url)
            .field("callback_token", &R)
            .field("log_channel", &self.log_channel)
            .field("deps_token", &self.deps_token.as_ref().map(|_| R))
            .field("npm_token", &self.npm_token.as_ref().map(|_| R))
            .field("claude_token", &self.claude_token.as_ref().map(|_| R))
            .field("retarget", &self.retarget)
            .finish()
    }
}

/// Token usage and cost of an agent run (or, summed, of a job). Every field is
/// optional and omitted when absent: `None` means unknown, not zero. `cost_usd`
/// is whatever the agent reported; nothing here estimates it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// Models that served the run, when known. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

// Costs are finite (they come from JSON), so equality is total.
impl Eq for Usage {}

impl Usage {
    /// True when no field carries a value.
    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cache_read_input_tokens.is_none()
            && self.cache_creation_input_tokens.is_none()
            && self.models.is_empty()
            && self.cost_usd.is_none()
    }
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
    /// Token usage and cost of the run, when the agent reported any. Omitted
    /// when absent; old receivers ignore it and old senders omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Every CTRL message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ctrl {
    Hello {
        versions: Vec<u16>,
        client: ClientKind,
        /// Optional client feature names (see [`feature`]), e.g.
        /// [`feature::CLAUDE_USER_TOKEN`]. Omitted when empty or absent; old
        /// servers ignore it. Self-declared by the client, like `client`.
        #[serde(default, skip_serializing_if = "features_is_empty")]
        features: Option<Vec<String>>,
    },
    Welcome {
        version: u16,
        /// Optional feature names (see [`feature`]). Omitted when empty or
        /// absent; old peers ignore it and old payloads decode without it.
        #[serde(default, skip_serializing_if = "features_is_empty")]
        features: Option<Vec<String>>,
    },
    Open {
        channel: u32,
        kind: ChannelKind,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<Vec<String>>,
        /// `acp` only: repo URL of the Project to work in (at most
        /// [`MAX_REPO_LEN`] bytes). Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo: Option<String>,
        /// `acp` only: ACP session id, a routing hint (at most
        /// [`MAX_RESUME_LEN`] bytes). Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resume: Option<String>,
        /// Optional per-user Claude token for this channel. Omitted when
        /// absent; old peers ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_token: Option<Secret>,
    },
    Opened {
        channel: u32,
    },
    OpenError {
        channel: u32,
        message: String,
        /// Machine-readable reason (see [`open_error_code`]); unknown values
        /// must be tolerated. Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
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
        /// Token usage and cost of the run, once known. Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
    RunCancel {
        run_id: String,
    },
    CredentialCaptured {
        kind: CredentialKind,
        /// The captured token, when the sender forwards it. Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<Secret>,
        /// Set for a capture made by a legacy (shared-credential) sandbox; the
        /// receiver acks it with `credential_state` (`scope: "user"`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        legacy: Option<bool>,
    },
    /// Client asks whether the sandbox holds a credential; answered by
    /// `credential_state`. No fields.
    CredentialStatus,
    /// Reply to `credential_status` (the sandbox's current view). Also sent
    /// client -> server as the ack of a `legacy` `credential_captured`.
    CredentialState {
        kind: CredentialKind,
        configured: bool,
        /// Whose credential this is (see [`credential_scope`]); open string.
        /// Omitted when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<String>,
    },
    /// Browser -> proxy (client -> server): clear the stored credential.
    CredentialClear {
        kind: CredentialKind,
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
