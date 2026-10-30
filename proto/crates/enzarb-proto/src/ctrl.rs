//! Typed CTRL messages (contracts §4). JSON objects tagged by `"type"`.
//!
//! Unknown fields are ignored on decode (additive evolution); unknown `type`
//! values are a decode error and should be answered with `error`
//! (`unknown_message`).

use serde::{Deserialize, Serialize};

/// Who is connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS, schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Ui,
    Proxy,
    Executor,
}

/// Channel kinds that can be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "codegen", derive(schemars::JsonSchema))]
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
    /// The server answers `stats_request` with `stats`. Clients must only
    /// send `stats_request` after seeing it (an old server ignores the request).
    pub const STATS: &str = "stats";
    /// The server answers `file_read` with `file`. Clients must only send
    /// `file_read` after seeing it (an old server ignores the request).
    pub const FILE_READ: &str = "file_read";
    /// `file_read` on the mux session of a chat resolves a relative path
    /// against that chat's worktree (not `/workspace`). Send a relative path
    /// meant for a chat's worktree only after seeing it; implies [`FILE_READ`].
    pub const FILE_READ_CHAT: &str = "file_read_chat";
    /// The server runs ADR-0022 steps (`step_*` and `session_event`). Clients
    /// must only send `step_*` after seeing it.
    pub const STEPS: &str = "steps";
    /// Client feature (`hello.features`): the client stores per-user Claude
    /// tokens from `credential_captured.token` and never relays them, and acks
    /// `legacy` captures. Only sandbox-proxy sends it.
    pub const CLAUDE_USER_TOKEN: &str = "claude_user_token";
    /// Decoders keep at most this many features...
    pub const MAX_FEATURES: usize = 64;
    /// ...and drop any longer than this many bytes.
    pub const MAX_FEATURE_LEN: usize = 64;
}

/// Largest file `file_read` returns (bytes of the raw file). Chosen so the
/// JSON-escaped reply stays under the 1 MiB frame cap even in the worst case.
pub const MAX_FILE_READ_BYTES: u64 = 128 * 1024;

/// Standard `file.code` values for a failed `file_read`. Open set.
pub mod file_error_code {
    pub const NOT_FOUND: &str = "not_found";
    pub const OUTSIDE_WORKSPACE: &str = "outside_workspace";
    pub const NOT_A_FILE: &str = "not_a_file";
    pub const TOO_LARGE: &str = "too_large";
    pub const NOT_UTF8: &str = "not_utf8";
    pub const IO: &str = "io";
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

// Codegen: a `Secret` is a plain string on the wire; TS wraps it (`SecretLike`, secret.ts). The
// `x-secret` schema keyword is how the generator finds every secret field (`examples/gen_ts.rs`).
#[cfg(feature = "codegen")]
impl ts_rs::TS for Secret {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn name(_: &ts_rs::Config) -> String {
        "SecretLike".into()
    }

    fn inline(cfg: &ts_rs::Config) -> String {
        Self::name(cfg)
    }

    fn decl(_: &ts_rs::Config) -> String {
        panic!("SecretLike is declared in secret.ts")
    }

    fn decl_concrete(_: &ts_rs::Config) -> String {
        panic!("SecretLike is declared in secret.ts")
    }
}

#[cfg(feature = "codegen")]
impl schemars::JsonSchema for Secret {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Secret".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "x-secret": true })
    }

    fn inline_schema() -> bool {
        true
    }
}

/// Known `credential_state.scope` values. Unknown values must be tolerated.
pub mod credential_scope {
    /// The credential is the user's own (per-user Claude token).
    pub const USER: &str = "user";
}

/// Credential kinds reported by `credential_captured` and `credential_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS, schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    ClaudeOauth,
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

/// Token usage and cost of an agent run (or, summed, of a job). Every field is
/// optional and omitted when absent: `None` means unknown, not zero. `cost_usd`
/// is whatever the agent reported; nothing here estimates it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS, schemars::JsonSchema), ts(optional_fields = nullable))]
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
    #[cfg_attr(feature = "codegen", ts(as = "Option<Vec<String>>", optional))]
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

/// Raw cumulative resource counters of a sandbox, read on demand from cgroup
/// v2 and statfs (`stats` reply). Every field but `ts_ms` is omitted when it
/// could not be read: absent means unknown, never 0. Consumers compute rates
/// from two snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS, schemars::JsonSchema), ts(optional_fields = nullable))]
pub struct SandboxStats {
    /// Unix time of the snapshot in milliseconds.
    pub ts_ms: u64,
    /// `cpu.stat` `usage_usec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    /// `cpu.stat` `user_usec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_user_usec: Option<u64>,
    /// `cpu.stat` `system_usec`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_system_usec: Option<u64>,
    /// `memory.current` in bytes (a gauge, not cumulative).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Cgroup `memory.max` in bytes. Omitted when the cgroup is unlimited (`max`) or unreadable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
    /// `memory.stat` `inactive_file` in bytes: reclaimable page cache that `memory.current`
    /// includes. `memory_bytes - memory_inactive_file_bytes` is the working set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_inactive_file_bytes: Option<u64>,
    /// `io.stat` `rbytes` summed over devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_read_bytes: Option<u64>,
    /// `io.stat` `wbytes` summed over devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_write_bytes: Option<u64>,
    /// Used bytes of the workspace filesystem (statfs; a gauge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_used_bytes: Option<u64>,
    /// Total bytes of the workspace filesystem (statfs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_total_bytes: Option<u64>,
}

/// Every CTRL message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, strum::IntoStaticStr)]
#[cfg_attr(feature = "codegen", derive(ts_rs::TS, schemars::JsonSchema), ts(optional_fields = nullable))]
#[strum(serialize_all = "snake_case")]
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
        #[cfg_attr(feature = "codegen", ts(type = "\"pty\" | \"acp\" | (string & {})"))]
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
    /// Client -> server: run a step (ADR-0022). Answered by `step_accepted`.
    /// Repeating a `run_id` is idempotent (`existing: true`).
    StepStart(Box<crate::step::StepStart>),
    StepAccepted {
        run_id: String,
        state: crate::step::StepPhase,
        existing: bool,
    },
    /// Client -> server: deliver a user turn into the running agent session.
    StepSteer {
        run_id: String,
        text: String,
    },
    /// Client -> server: stop the step at the next checkpoint; it finishes with exit `interrupted`.
    StepStop {
        run_id: String,
    },
    /// Client -> server: continue an interrupted step (after a sandbox restart) by
    /// re-attaching to its saved agent session (ACP `session/load`).
    StepResume {
        run_id: String,
    },
    /// Client -> server: replay `session_event`s with `seq > after_seq` and answer
    /// `step_state`. Also subscribes this session to the run's further events.
    StepStatus {
        run_id: String,
        #[serde(default)]
        #[cfg_attr(feature = "codegen", ts(as = "Option<u64>", optional))]
        after_seq: u64,
    },
    StepState {
        run_id: String,
        state: crate::step::StepPhase,
        /// Set once the step is terminal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<crate::step::StepFinished>,
    },
    /// Server -> client: one captured, redacted session event.
    SessionEvent(crate::step::SessionEvent),
    /// Server -> client: a step ended. May arrive more than once.
    StepFinished(crate::step::StepFinished),
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
    /// Client asks for a stats snapshot; answered by `stats`. No fields. Only
    /// send it after `welcome.features` contained [`feature::STATS`].
    StatsRequest,
    /// Reply to `stats_request`.
    Stats(SandboxStats),
    /// Client asks for one UTF-8 text file of the sandbox workspace; answered
    /// by `file` with the same `id`. `path` is relative to `/workspace` (or
    /// absolute under it); anything that resolves outside is refused. Only
    /// send it after `welcome.features` contained [`feature::FILE_READ`].
    FileRead {
        id: String,
        path: String,
    },
    /// Reply to `file_read`: `content` on success, else `code`
    /// (see [`file_error_code`]) and `message`.
    File {
        id: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
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

/// Who may send a CTRL message, and in which direction it travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Only in the handshake (`hello`, `welcome`); never afterwards.
    Handshake,
    /// Client -> server, from these client kinds.
    ClientToServer(&'static [ClientKind]),
    /// Server -> client only; no client may send it.
    ServerToClient,
}

const ANY_CLIENT: &[ClientKind] = &[ClientKind::Ui, ClientKind::Proxy, ClientKind::Executor];

impl Ctrl {
    /// The wire `type` of this message (`"step_start"`, ...), as serialized.
    pub fn tag(&self) -> &'static str {
        self.into()
    }

    /// Who sends this message. The one direction table: the session core and
    /// sandbox-proxy's browser guard both derive their rules from it. The
    /// `match` is exhaustive on purpose, so a new variant does not compile
    /// until it is classified here.
    ///
    /// A variant that also travels as a reply (`error`, `credential_state`)
    /// is listed by its client -> server senders.
    pub fn direction(&self) -> Direction {
        use ClientKind::{Executor, Proxy, Ui};
        match self {
            Ctrl::Hello { .. } | Ctrl::Welcome { .. } => Direction::Handshake,
            Ctrl::Open { .. } | Ctrl::Resize { .. } | Ctrl::Error { .. } => {
                Direction::ClientToServer(ANY_CLIENT)
            }
            Ctrl::CredentialStatus | Ctrl::CredentialClear { .. } | Ctrl::FileRead { .. } => {
                Direction::ClientToServer(&[Ui, Proxy])
            }
            // The ack of a legacy `credential_captured`, sent by sandbox-proxy.
            Ctrl::CredentialState { .. } | Ctrl::StatsRequest => {
                Direction::ClientToServer(&[Proxy])
            }
            // Steps are engine-only (ADR-0022).
            Ctrl::StepStart(_)
            | Ctrl::StepSteer { .. }
            | Ctrl::StepStop { .. }
            | Ctrl::StepResume { .. }
            | Ctrl::StepStatus { .. } => Direction::ClientToServer(&[Executor]),
            Ctrl::Opened { .. }
            | Ctrl::OpenError { .. }
            | Ctrl::StepAccepted { .. }
            | Ctrl::StepState { .. }
            | Ctrl::SessionEvent(_)
            | Ctrl::StepFinished(_)
            | Ctrl::CredentialCaptured { .. }
            | Ctrl::Stats(_)
            | Ctrl::File { .. } => Direction::ServerToClient,
        }
    }

    /// True when a client of this kind may send the message after the
    /// handshake. Default-deny: handshake and server -> client messages are
    /// allowed from nobody.
    pub fn allowed_from(&self, kind: ClientKind) -> bool {
        matches!(self.direction(), Direction::ClientToServer(kinds) if kinds.contains(&kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One message of every `type`, from the CTRL vectors.
    fn vectors() -> Vec<(String, Ctrl)> {
        let all: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../vectors/ctrl.json")).unwrap();
        all.into_iter()
            .map(|v| {
                let ty = v["message"]["type"].as_str().unwrap().to_string();
                (ty, serde_json::from_value(v["message"].clone()).unwrap())
            })
            .collect()
    }

    fn allowed(kind: ClientKind) -> Vec<&'static str> {
        let mut v: Vec<_> = vectors()
            .iter()
            .filter(|(_, m)| m.allowed_from(kind))
            .map(|(_, m)| m.tag())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    #[test]
    fn tag_is_the_serialized_type() {
        let vectors = vectors();
        assert!(!vectors.is_empty());
        for (ty, m) in &vectors {
            assert_eq!(m.tag(), ty);
            assert_eq!(serde_json::to_value(m).unwrap()["type"], *ty);
        }
    }

    #[test]
    fn vectors_cover_every_variant() {
        // `tag()` is exhaustive by construction; this pins that the vectors cover them all.
        let mut tags: Vec<_> = vectors().into_iter().map(|(ty, _)| ty).collect();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), 24, "{tags:?}");
    }

    #[test]
    fn browser_may_send_six_messages() {
        assert_eq!(
            allowed(ClientKind::Ui),
            [
                "credential_clear",
                "credential_status",
                "error",
                "file_read",
                "open",
                "resize"
            ]
        );
    }

    #[test]
    fn steps_are_executor_only() {
        let steps = [
            "step_resume",
            "step_start",
            "step_status",
            "step_steer",
            "step_stop",
        ];
        let exec = allowed(ClientKind::Executor);
        for s in steps {
            assert!(exec.contains(&s), "{s}");
            assert!(!allowed(ClientKind::Ui).contains(&s), "{s}");
            assert!(!allowed(ClientKind::Proxy).contains(&s), "{s}");
        }
    }

    #[test]
    fn handshake_and_replies_are_allowed_from_nobody() {
        for kind in [ClientKind::Ui, ClientKind::Proxy, ClientKind::Executor] {
            let a = allowed(kind);
            for t in [
                "hello",
                "welcome",
                "opened",
                "open_error",
                "step_accepted",
                "step_state",
                "session_event",
                "step_finished",
                "credential_captured",
                "stats",
                "file",
            ] {
                assert!(!a.contains(&t), "{kind:?} {t}");
            }
        }
    }

    /// The session core documents which messages each side receives
    /// (`Event::Ctrl`); the server-side list must be exactly what some client
    /// may send, bar `open` (its own `Event::Open`).
    #[test]
    fn server_side_events_match_the_table() {
        let mut server: Vec<_> = vectors()
            .iter()
            .filter(|(_, m)| matches!(m.direction(), Direction::ClientToServer(_)))
            .map(|(_, m)| m.tag())
            .filter(|t| *t != "open")
            .collect();
        server.sort_unstable();
        server.dedup();
        assert_eq!(
            server,
            [
                "credential_clear",
                "credential_state",
                "credential_status",
                "error",
                "file_read",
                "resize",
                "stats_request",
                "step_resume",
                "step_start",
                "step_status",
                "step_steer",
                "step_stop",
            ]
        );
    }
}
