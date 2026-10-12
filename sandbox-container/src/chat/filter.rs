//! The chat relay's JSON-RPC filter: pure (no I/O), one instance per chat
//! channel.
//!
//! sandbox-init sits between the browser (an ACP *client*) and
//! `claude-agent-acp` (the ACP *agent*). Every NDJSON line from either side
//! is parsed here and turned into [`Out`] actions. Nothing is passed through
//! verbatim from the browser: each allowed request is rebuilt from the fields
//! sandbox-init knows, so unknown fields (`_meta` in particular, which the
//! adapter reads for SDK options, env, system prompt and session resume) never
//! reach the agent.
//!
//! Browser → agent (anything else gets JSON-RPC `-32601`):
//! * `initialize` (once): `clientCapabilities` forced to none (no fs, no terminal);
//! * `session/new`: `cwd` = the chat worktree, `mcpServers: []`;
//! * `session/load`: only when the agent advertised `loadSession`, only for
//!   the session id this chat was resumed with (sandbox-init's registry);
//! * `session/prompt`: `text` and `resource_link` blocks, at most
//!   [`MAX_PROMPT_BYTES`] of text;
//! * `session/cancel` (notification);
//! * `session/set_mode`: `default` and `plan` only;
//! * responses: only to an outstanding forwarded `session/request_permission`,
//!   and only `allow_once` / `reject_once` / `cancelled`. Anything else (an
//!   `*_always` option, an unknown option id, an error, junk) becomes
//!   `reject_once` (or `cancelled` when the request had no such option).
//!
//! Agent → browser:
//! * responses to forwarded requests and `session/update`, with the Claude
//!   token redacted from every string;
//! * `session/request_permission`: [`Policy::classify`]. A hard deny is
//!   answered `reject_once` here and reported to the browser as the
//!   `_enzarb/permission_denied` notification; everything else is forwarded
//!   with the policy's opinion in `_meta.enzarb` (a hint, never a default);
//! * every other request (`fs/*`, `terminal/*`, ...) gets `-32601` back, and
//!   other notifications are dropped.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::policy::{Classification, Policy};

/// Largest total text of one `session/prompt`.
pub const MAX_PROMPT_BYTES: usize = 256 * 1024;
/// Permission requests forwarded to the browser and not yet answered.
pub const MAX_PENDING_PERMISSIONS: usize = 64;
/// Browser requests forwarded to the agent and not yet answered.
pub const MAX_PENDING_REQUESTS: usize = 64;
/// Modes the browser may switch to. `acceptEdits` and `bypassPermissions`
/// would skip per-call approval, so they are refused.
pub const ALLOWED_MODES: &[&str] = &["default", "plan"];
/// The browser notification for a hard deny.
pub const PERMISSION_DENIED: &str = "_enzarb/permission_denied";
/// Longest JSON-RPC string id accepted from the browser.
const MAX_ID_LEN: usize = 128;
/// Longest ACP session id accepted (Claude's are UUIDs).
pub const MAX_SESSION_ID_LEN: usize = 128;

/// JSON-RPC error codes.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
}

/// What to do with a parsed line.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Write this message to the agent.
    Agent(Value),
    /// Write this message to the browser (already redacted).
    Browser(Value),
    /// Append a transcript record `(kind, fields)` (already redacted).
    Record(&'static str, Value),
    /// The chat now has this ACP session (record it in the registry).
    Session(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Pending {
    Initialize,
    New,
    Load(String),
    Prompt,
    Other,
}

#[derive(Debug, Clone)]
struct PermReq {
    /// The agent's request id, echoed in the answer.
    id: Value,
    /// `(optionId, kind)` of the request's options.
    options: Vec<(String, String)>,
    tool_call_id: Value,
}

/// True for an ACP session id sandbox-init accepts (`[A-Za-z0-9._-]`, no
/// leading dot, at most [`MAX_SESSION_ID_LEN`]).
pub fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_SESSION_ID_LEN
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Per-channel filter state.
pub struct Filter {
    policy: Policy,
    worktree: PathBuf,
    secrets: Vec<String>,
    /// The only session id `session/load` may name (from the registry).
    resume: Option<String>,
    init_sent: bool,
    initialized: bool,
    load_session: bool,
    session: Option<String>,
    establishing: bool,
    prompting: usize,
    browser_pending: HashMap<String, Pending>,
    permissions: HashMap<String, PermReq>,
}

fn id_key(id: &Value) -> String {
    id.to_string()
}

fn valid_id(id: &Value) -> bool {
    match id {
        // integers only, exactly representable in JS (the agent echoes ids)
        Value::Number(n) => n.as_i64().is_some_and(|i| i.unsigned_abs() <= (1u64 << 53)),
        Value::String(s) => s.len() <= MAX_ID_LEN,
        _ => false,
    }
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn str_field<'a>(p: Option<&'a Map<String, Value>>, k: &str) -> Option<&'a str> {
    p.and_then(|p| p.get(k)).and_then(Value::as_str)
}

/// The fixed `_meta` sandbox-init sends on `session/new`/`load`: bypass mode
/// is never available to the agent's SDK, whatever its settings say.
fn session_meta() -> Value {
    // `settingSources: ["user"]`: the repo's committed `.claude/settings*.json`
    // (permission allow rules, hooks, modes) is not loaded, so nothing in the
    // repo can approve a tool call; the user settings dir is the chat's own
    // CLAUDE_CONFIG_DIR under `.enzarb/` (hard-denied to the agent).
    json!({"claudeCode": {"options": {
        "allowDangerouslySkipPermissions": false,
        "permissionMode": "default",
        "settingSources": ["user"],
    }}})
}

impl Filter {
    /// `secrets` are redacted from everything sent to the browser and the
    /// transcript; `resume` is the ACP session id the chat may load.
    pub fn new(
        policy: Policy,
        worktree: PathBuf,
        secrets: Vec<String>,
        resume: Option<String>,
    ) -> Self {
        Self {
            policy,
            worktree,
            secrets: secrets.into_iter().filter(|s| s.len() >= 8).collect(),
            resume,
            init_sent: false,
            initialized: false,
            load_session: false,
            session: None,
            establishing: false,
            prompting: 0,
            browser_pending: HashMap::new(),
            permissions: HashMap::new(),
        }
    }

    /// The established ACP session, if any.
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// A `session/prompt` is in flight.
    pub fn prompting(&self) -> bool {
        self.prompting > 0
    }

    /// Permission requests waiting for the browser.
    pub fn pending_permissions(&self) -> usize {
        self.permissions.len()
    }

    /// Redact the secrets from every string (and key) of `v`.
    pub fn redact(&self, v: Value) -> Value {
        redact_value(v, &self.secrets)
    }

    // ---- browser -> agent ----

    /// Handle one line from the browser.
    pub fn from_browser(&mut self, line: &[u8]) -> Vec<Out> {
        let v: Value = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => {
                return vec![Out::Browser(rpc_error(
                    Value::Null,
                    code::PARSE_ERROR,
                    "parse error",
                ))];
            }
        };
        let Value::Object(m) = v else {
            return vec![Out::Browser(rpc_error(
                Value::Null,
                code::INVALID_REQUEST,
                "not a JSON-RPC message",
            ))];
        };
        let id = m.get("id").cloned();
        if m.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return match id {
                Some(id) if valid_id(&id) => vec![Out::Browser(rpc_error(
                    id,
                    code::INVALID_REQUEST,
                    "jsonrpc must be \"2.0\"",
                ))],
                _ => vec![self.dropped("browser", "not JSON-RPC 2.0")],
            };
        }
        match (m.get("method"), id) {
            (Some(Value::String(method)), Some(id)) => {
                if !valid_id(&id) {
                    return vec![Out::Browser(rpc_error(
                        Value::Null,
                        code::INVALID_REQUEST,
                        "invalid id",
                    ))];
                }
                let method = method.clone();
                self.browser_request(&method, id, m.get("params").and_then(Value::as_object))
            }
            (Some(Value::String(method)), None) => {
                let method = method.clone();
                self.browser_notification(&method, m.get("params").and_then(Value::as_object))
            }
            (None, Some(id)) => self.browser_response(id, &m),
            _ => vec![self.dropped("browser", "invalid message")],
        }
    }

    fn dropped(&self, from: &str, reason: &str) -> Out {
        Out::Record("dropped", json!({"from": from, "reason": reason}))
    }

    fn refuse(&self, id: Value, method: &str, code: i64, message: &str) -> Vec<Out> {
        vec![
            Out::Browser(rpc_error(id, code, message)),
            Out::Record(
                "refused",
                self.redact(json!({"method": method, "code": code, "reason": message})),
            ),
        ]
    }

    fn forward(&mut self, id: Value, method: &str, params: Value, pending: Pending) -> Vec<Out> {
        self.browser_pending.insert(id_key(&id), pending);
        let rec = if method == "session/prompt" {
            Out::Record("prompt", self.redact(params.clone()))
        } else {
            Out::Record(
                "browser",
                self.redact(json!({"method": method, "params": params})),
            )
        };
        vec![
            rec,
            Out::Agent(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})),
        ]
    }

    fn check_session(&self, p: Option<&Map<String, Value>>) -> Result<String, &'static str> {
        match (str_field(p, "sessionId"), &self.session) {
            (Some(s), Some(cur)) if s == cur => Ok(s.to_string()),
            (_, None) => Err("no session: call session/new or session/load first"),
            _ => Err("unknown session"),
        }
    }

    fn browser_request(
        &mut self,
        method: &str,
        id: Value,
        p: Option<&Map<String, Value>>,
    ) -> Vec<Out> {
        if self.browser_pending.contains_key(&id_key(&id)) {
            return self.refuse(id, method, code::INVALID_REQUEST, "duplicate request id");
        }
        if self.browser_pending.len() >= MAX_PENDING_REQUESTS {
            return self.refuse(
                id,
                method,
                code::INVALID_REQUEST,
                "too many requests in flight",
            );
        }
        match method {
            "initialize" => {
                if self.init_sent {
                    return self.refuse(id, method, code::INVALID_REQUEST, "already initialized");
                }
                let version = p
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_u64)
                    .filter(|v| *v <= u64::from(u16::MAX))
                    .unwrap_or(1);
                let mut params = json!({
                    "protocolVersion": version,
                    "clientCapabilities": {
                        "fs": {"readTextFile": false, "writeTextFile": false},
                        "terminal": false,
                    },
                });
                if let Some(info) = p
                    .and_then(|p| p.get("clientInfo"))
                    .and_then(Value::as_object)
                {
                    let mut ci = Map::new();
                    for k in ["name", "title", "version"] {
                        if let Some(s) = info.get(k).and_then(Value::as_str) {
                            ci.insert(k.into(), Value::String(s.chars().take(128).collect()));
                        }
                    }
                    if ci.contains_key("name") && ci.contains_key("version") {
                        params["clientInfo"] = Value::Object(ci);
                    }
                }
                self.init_sent = true;
                self.forward(id, method, params, Pending::Initialize)
            }
            "session/new" => {
                if !self.init_sent {
                    return self.refuse(id, method, code::INVALID_REQUEST, "initialize first");
                }
                if self.session.is_some() || self.establishing {
                    return self.refuse(
                        id,
                        method,
                        code::INVALID_REQUEST,
                        "this chat already has a session",
                    );
                }
                self.establishing = true;
                let params = json!({
                    "cwd": self.worktree,
                    "mcpServers": [],
                    "_meta": session_meta(),
                });
                self.forward(id, method, params, Pending::New)
            }
            "session/load" => {
                if !self.initialized || !self.load_session {
                    return self.refuse(
                        id,
                        method,
                        code::METHOD_NOT_FOUND,
                        "session/load is not supported",
                    );
                }
                if self.session.is_some() || self.establishing {
                    return self.refuse(
                        id,
                        method,
                        code::INVALID_REQUEST,
                        "this chat already has a session",
                    );
                }
                let sid = str_field(p, "sessionId").unwrap_or_default();
                if !valid_session_id(sid) || self.resume.as_deref() != Some(sid) {
                    return self.refuse(id, method, code::INVALID_PARAMS, "unknown session");
                }
                let sid = sid.to_string();
                self.establishing = true;
                let params = json!({
                    "sessionId": sid,
                    "cwd": self.worktree,
                    "mcpServers": [],
                    "_meta": session_meta(),
                });
                self.forward(id, method, params, Pending::Load(sid))
            }
            "session/prompt" => {
                let sid = match self.check_session(p) {
                    Ok(s) => s,
                    Err(e) => return self.refuse(id, method, code::INVALID_PARAMS, e),
                };
                let Some(blocks) = p.and_then(|p| p.get("prompt")).and_then(Value::as_array) else {
                    return self.refuse(
                        id,
                        method,
                        code::INVALID_PARAMS,
                        "prompt must be an array",
                    );
                };
                let prompt = match rebuild_prompt(blocks) {
                    Ok(b) => b,
                    Err(e) => return self.refuse(id, method, code::INVALID_PARAMS, e),
                };
                self.prompting += 1;
                self.forward(
                    id,
                    method,
                    json!({"sessionId": sid, "prompt": prompt}),
                    Pending::Prompt,
                )
            }
            "session/set_mode" => {
                let sid = match self.check_session(p) {
                    Ok(s) => s,
                    Err(e) => return self.refuse(id, method, code::INVALID_PARAMS, e),
                };
                let mode = str_field(p, "modeId").unwrap_or_default();
                if !ALLOWED_MODES.contains(&mode) {
                    return self.refuse(
                        id,
                        method,
                        code::INVALID_PARAMS,
                        "mode not allowed (default or plan only)",
                    );
                }
                let params = json!({"sessionId": sid, "modeId": mode});
                self.forward(id, method, params, Pending::Other)
            }
            "session/cancel" => self.refuse(
                id,
                method,
                code::INVALID_REQUEST,
                "session/cancel is a notification",
            ),
            _ => self.refuse(id, method, code::METHOD_NOT_FOUND, "method not allowed"),
        }
    }

    fn browser_notification(&mut self, method: &str, p: Option<&Map<String, Value>>) -> Vec<Out> {
        if method != "session/cancel" {
            return vec![self.dropped(
                "browser",
                &format!("notification {} not allowed", truncate(method)),
            )];
        }
        let Ok(sid) = self.check_session(p) else {
            return vec![self.dropped("browser", "session/cancel for an unknown session")];
        };
        // ACP: after session/cancel the client answers every pending
        // permission request `cancelled`; do it here, whatever the browser does.
        let mut out = self.cancel_permissions();
        out.push(Out::Record("browser", json!({"method": method})));
        out.push(Out::Agent(json!({
            "jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": sid}
        })));
        out
    }

    fn browser_response(&mut self, id: Value, m: &Map<String, Value>) -> Vec<Out> {
        let Some(req) = self.permissions.remove(&id_key(&id)) else {
            return vec![self.dropped("browser", "response to no outstanding permission request")];
        };
        let outcome = m.get("result").and_then(|r| r.get("outcome"));
        let chosen = match outcome
            .and_then(|o| o.get("outcome"))
            .and_then(Value::as_str)
        {
            Some("cancelled") => Some(None),
            Some("selected") => {
                let oid = outcome
                    .and_then(|o| o.get("optionId"))
                    .and_then(Value::as_str);
                req.options
                    .iter()
                    .find(|(o, k)| {
                        Some(o.as_str()) == oid && (k == "allow_once" || k == "reject_once")
                    })
                    .map(|(o, k)| Some((o.clone(), k.clone())))
            }
            _ => None,
        };
        let replaced = chosen.is_none();
        let chosen = chosen
            .unwrap_or_else(|| reject_option(&req.options).map(|o| (o, "reject_once".into())));
        let (outcome, answer) = match &chosen {
            Some((o, k)) => (json!({"outcome": "selected", "optionId": o}), k.as_str()),
            None => (json!({"outcome": "cancelled"}), "cancelled"),
        };
        vec![
            Out::Record(
                "permission",
                self.redact(json!({
                    "tool_call_id": req.tool_call_id,
                    "decision": "user",
                    "answer": answer,
                    "replaced": replaced,
                })),
            ),
            Out::Agent(json!({"jsonrpc": "2.0", "id": req.id, "result": {"outcome": outcome}})),
        ]
    }

    // ---- agent -> browser ----

    /// Handle one line from the agent.
    pub fn from_agent(&mut self, line: &[u8]) -> Vec<Out> {
        let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(line) else {
            return vec![self.dropped("agent", "invalid JSON-RPC")];
        };
        let id = m.get("id").cloned().filter(valid_id);
        match (m.get("method").and_then(Value::as_str), id) {
            (Some(method), Some(id)) => {
                let method = method.to_string();
                self.agent_request(&method, id, &m)
            }
            (Some(method), None) => {
                if method == "session/update" {
                    let msg = self.redact(Value::Object(m.clone()));
                    let rec = msg.get("params").cloned().unwrap_or(Value::Null);
                    vec![Out::Record("update", rec), Out::Browser(msg)]
                } else {
                    vec![self.dropped(
                        "agent",
                        &format!("notification {} not relayed", truncate(method)),
                    )]
                }
            }
            (None, Some(id)) => self.agent_response(id, m),
            _ => vec![self.dropped("agent", "invalid message")],
        }
    }

    fn agent_response(&mut self, id: Value, m: Map<String, Value>) -> Vec<Out> {
        let Some(pending) = self.browser_pending.remove(&id_key(&id)) else {
            return vec![self.dropped("agent", "response to no forwarded request")];
        };
        let result = m.get("result").filter(|r| r.is_object());
        let mut out = Vec::new();
        match pending {
            Pending::Initialize => {
                self.initialized = true;
                self.load_session = result
                    .and_then(|r| r.get("agentCapabilities"))
                    .and_then(|c| c.get("loadSession"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            }
            Pending::New => {
                self.establishing = false;
                match result
                    .and_then(|r| r.get("sessionId"))
                    .and_then(Value::as_str)
                {
                    Some(s) if valid_session_id(s) => {
                        self.session = Some(s.to_string());
                        out.push(Out::Session(s.to_string()));
                    }
                    Some(_) => {
                        out.push(self.dropped("agent", "invalid session id from session/new"))
                    }
                    None => {}
                }
            }
            Pending::Load(sid) => {
                self.establishing = false;
                if result.is_some() {
                    self.session = Some(sid.clone());
                    out.push(Out::Session(sid));
                }
            }
            Pending::Prompt => self.prompting = self.prompting.saturating_sub(1),
            Pending::Other => {}
        }
        let msg = self.redact(Value::Object(m));
        out.push(Out::Record(
            "agent",
            json!({"response": msg.get("result").map(|_| "result").unwrap_or("error")}),
        ));
        out.push(Out::Browser(msg));
        out
    }

    fn agent_request(&mut self, method: &str, id: Value, m: &Map<String, Value>) -> Vec<Out> {
        if method != "session/request_permission" {
            return vec![
                Out::Agent(rpc_error(
                    id,
                    code::METHOD_NOT_FOUND,
                    "method not available",
                )),
                self.dropped("agent", &format!("request {} refused", truncate(method))),
            ];
        }
        let p = m.get("params").and_then(Value::as_object);
        let tool_call = p
            .and_then(|p| p.get("toolCall"))
            .cloned()
            .unwrap_or(Value::Null);
        let options: Vec<(String, String)> = p
            .and_then(|p| p.get("options"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|o| {
                        Some((
                            o.get("optionId")?.as_str()?.to_string(),
                            o.get("kind")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let session_id = p
            .and_then(|p| p.get("sessionId"))
            .cloned()
            .unwrap_or(Value::Null);
        let key = id_key(&id);
        let tool_call_id = tool_call.get("toolCallId").cloned().unwrap_or(Value::Null);
        let mut ids: Vec<&str> = options.iter().map(|(o, _)| o.as_str()).collect();
        ids.sort_unstable();
        let dup_options = ids.windows(2).any(|w| w[0] == w[1]);
        if dup_options
            || self.permissions.contains_key(&key)
            || self.permissions.len() >= MAX_PENDING_PERMISSIONS
        {
            return vec![
                Out::Agent(
                    json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": {"outcome": "cancelled"}}}),
                ),
                self.dropped("agent", "permission request id in use or too many pending"),
            ];
        }
        match self.policy.classify(&tool_call, &self.worktree) {
            Classification::HardDeny(d) => {
                let outcome = match reject_option(&options) {
                    Some(o) => json!({"outcome": "selected", "optionId": o}),
                    None => json!({"outcome": "cancelled"}),
                };
                let note = self.redact(json!({
                    "jsonrpc": "2.0",
                    "method": PERMISSION_DENIED,
                    "params": {
                        "sessionId": session_id,
                        "toolCall": tool_call,
                        "rule": d.rule,
                        "reason": d.reason,
                    },
                }));
                vec![
                    Out::Agent(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": outcome}})),
                    Out::Record(
                        "permission",
                        self.redact(json!({
                            "tool_call_id": tool_call_id,
                            "tool_kind": tool_call.get("kind"),
                            "title": tool_call.get("title"),
                            "input": tool_call.get("rawInput"),
                            "decision": "hard_deny",
                            "rule": d.rule,
                            "reason": d.reason,
                        })),
                    ),
                    Out::Browser(note),
                ]
            }
            Classification::Ask(hint) => {
                self.permissions.insert(
                    key,
                    PermReq {
                        id: id.clone(),
                        options,
                        tool_call_id: tool_call_id.clone(),
                    },
                );
                let mut msg = Value::Object(m.clone());
                let hint = serde_json::to_value(&hint).unwrap_or(Value::Null);
                if let Some(params) = msg.get_mut("params").and_then(Value::as_object_mut) {
                    let meta = params.entry("_meta").or_insert_with(|| json!({}));
                    if !meta.is_object() {
                        *meta = json!({});
                    }
                    meta["enzarb"] = hint.clone();
                }
                vec![
                    Out::Record(
                        "permission",
                        self.redact(json!({
                            "tool_call_id": tool_call_id,
                            "tool_kind": tool_call.get("kind"),
                            "title": tool_call.get("title"),
                            "input": tool_call.get("rawInput"),
                            "decision": "ask",
                            "hint": hint,
                        })),
                    ),
                    Out::Browser(self.redact(msg)),
                ]
            }
        }
    }

    // ---- shutdown ----

    /// Answer every outstanding permission request `cancelled` (close, idle,
    /// cancel). Late browser answers are then dropped.
    pub fn cancel_permissions(&mut self) -> Vec<Out> {
        let mut reqs: Vec<PermReq> = self.permissions.drain().map(|(_, r)| r).collect();
        reqs.sort_by_key(|r| r.id.to_string());
        reqs.into_iter()
            .flat_map(|r| {
                [
                    Out::Record(
                        "permission",
                        json!({"tool_call_id": r.tool_call_id, "decision": "cancelled"}),
                    ),
                    Out::Agent(json!({
                        "jsonrpc": "2.0", "id": r.id, "result": {"outcome": {"outcome": "cancelled"}}
                    })),
                ]
            })
            .collect()
    }

    /// `session/cancel` for the established session.
    pub fn session_cancel(&self) -> Option<Value> {
        self.session.as_ref().map(
            |s| json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": s}}),
        )
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(64).collect()
}

fn reject_option(options: &[(String, String)]) -> Option<String> {
    options
        .iter()
        .find(|(_, k)| k == "reject_once")
        .map(|(o, _)| o.clone())
}

/// Rebuild prompt blocks: `text` and `resource_link` only, known fields only.
fn rebuild_prompt(blocks: &[Value]) -> Result<Vec<Value>, &'static str> {
    let mut total = 0usize;
    let mut out = Vec::with_capacity(blocks.len());
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                let t = b
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("text block without text")?;
                total += t.len();
                out.push(json!({"type": "text", "text": t}));
            }
            Some("resource_link") => {
                let uri = b
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or("resource_link without uri")?;
                let name = b
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or("resource_link without name")?;
                total += uri.len() + name.len();
                let mut r = json!({"type": "resource_link", "uri": uri, "name": name});
                for k in ["title", "description", "mimeType"] {
                    if let Some(s) = b.get(k).and_then(Value::as_str) {
                        total += s.len();
                        r[k] = Value::String(s.to_string());
                    }
                }
                if let Some(n) = b.get("size").and_then(Value::as_u64) {
                    r["size"] = n.into();
                }
                out.push(r);
            }
            _ => return Err("only text and resource_link blocks are allowed"),
        }
        if total > MAX_PROMPT_BYTES {
            return Err("prompt too large (256 KiB max)");
        }
    }
    if out.is_empty() {
        return Err("empty prompt");
    }
    Ok(out)
}

// ---- redaction ----

/// Prefix of Anthropic credentials; any such token is redacted, not only the
/// configured one.
const TOKEN_PREFIX: &str = "sk-ant-";
const REDACTED: &str = "[redacted]";

/// Redact `secrets` (exact) and anything that looks like an Anthropic token.
pub fn redact_str<'a>(s: &'a str, secrets: &[String]) -> Cow<'a, str> {
    let mut out: Cow<'a, str> = Cow::Borrowed(s);
    for sec in secrets {
        if out.contains(sec.as_str()) {
            out = Cow::Owned(out.replace(sec.as_str(), REDACTED));
        }
    }
    if out.contains(TOKEN_PREFIX) {
        let mut r = String::with_capacity(out.len());
        let mut rest: &str = &out;
        while let Some(i) = rest.find(TOKEN_PREFIX) {
            r.push_str(&rest[..i]);
            let tail = &rest[i + TOKEN_PREFIX.len()..];
            let n = tail
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
                .count();
            if n >= 8 {
                r.push_str(REDACTED);
            } else {
                r.push_str(&rest[i..i + TOKEN_PREFIX.len() + n]);
            }
            rest = &tail[n..];
        }
        r.push_str(rest);
        out = Cow::Owned(r);
    }
    out
}

/// [`redact_str`] over every string and key of a JSON value.
pub fn redact_value(v: Value, secrets: &[String]) -> Value {
    match v {
        Value::String(s) => match redact_str(&s, secrets) {
            Cow::Borrowed(_) => Value::String(s),
            Cow::Owned(o) => Value::String(o),
        },
        Value::Array(a) => Value::Array(a.into_iter().map(|x| redact_value(x, secrets)).collect()),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, x)| {
                    (
                        redact_str(&k, secrets).into_owned(),
                        redact_value(x, secrets),
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

/// The forms of a token that are redacted: the token, and its standard and
/// URL-safe base64 encodings (alone and as `x:token` basic auth).
pub fn token_forms(token: &str) -> Vec<String> {
    use base64::Engine as _;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    let mut v = vec![token.to_string()];
    for raw in [
        token.to_string(),
        format!("x-access-token:{token}"),
        format!("x:{token}"),
    ] {
        for e in [
            STANDARD.encode(&raw),
            STANDARD_NO_PAD.encode(&raw),
            URL_SAFE_NO_PAD.encode(&raw),
        ] {
            // drop the last chars, which depend on what follows the token
            let keep = e.len().saturating_sub(4);
            v.push(e[..keep].to_string());
        }
    }
    v.retain(|s| s.len() >= 8);
    v.sort();
    v.dedup();
    v
}
