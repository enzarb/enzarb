//! An agent step: sandbox-init is the ACP client of the runtime's process.
//!
//! `initialize`, then `session/new` (or `session/load` of the saved session
//! when resuming), then the prompt as a user turn. Every `session/update` is
//! emitted as a session event (text chunks of one kind are merged), every
//! permission request is allowed (the sandbox is the boundary), steering
//! texts become further user turns after the current one, and a stop sends
//! `session/cancel` and ends the step `interrupted` once the turn ended (or
//! after the cancel grace).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use enzarb_proto::Usage;
use enzarb_proto::step::{exit, session_kind};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::acp::{self, UsageCollector};
use crate::exec::{Ctl, LineSink, terminate};
use crate::reaper::Reaper;

/// Merged text chunks are flushed at this size.
const CHUNK_FLUSH: usize = 64 * 1024;

pub struct AgentSpec {
    pub bin: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    /// The first user turn.
    pub prompt: String,
    /// Resume this ACP session (`session/load`) instead of `session/new`.
    pub resume: Option<String>,
    /// `mcpServers` of the session.
    pub mcp_servers: Vec<Value>,
    pub cancel_grace: Duration,
}

pub struct AgentEnd {
    pub exit: &'static str,
    pub error: Option<String>,
    pub usage: Option<Usage>,
}

/// Receives `(kind, payload)` session events.
pub type Emit<'a> = &'a (dyn Fn(&str, Value) + Send + Sync);

#[derive(PartialEq)]
enum Wait {
    Init,
    Session,
    Prompt,
}

struct Driver<'a> {
    tx: mpsc::UnboundedSender<String>,
    next_id: u64,
    emit: Emit<'a>,
    chunk: Option<(&'static str, String)>,
    usage: UsageCollector,
}

impl Driver<'_> {
    fn send(&self, v: Value) {
        let _ = self.tx.send(format!("{v}\n"));
    }

    fn request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn flush(&mut self) {
        if let Some((kind, text)) = self.chunk.take() {
            (self.emit)(kind, json!({"text": text}));
        }
    }

    fn event(&mut self, kind: &str, payload: Value) {
        self.flush();
        (self.emit)(kind, payload);
    }

    fn update(&mut self, update: &Value) {
        self.usage.update(update);
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("");
        let text_kind = match kind {
            "agent_message_chunk" => Some(session_kind::ASSISTANT),
            "agent_thought_chunk" => Some(session_kind::THINKING),
            "user_message_chunk" => Some(session_kind::USER),
            _ => None,
        };
        if let Some(k) = text_kind {
            let c = update.get("content");
            if c.and_then(|c| c.get("type")).and_then(Value::as_str) == Some("text")
                && let Some(t) = c.and_then(|c| c.get("text")).and_then(Value::as_str)
            {
                match &mut self.chunk {
                    Some((ck, buf)) if *ck == k => buf.push_str(t),
                    _ => {
                        self.flush();
                        self.chunk = Some((k, t.to_string()));
                    }
                }
                if self
                    .chunk
                    .as_ref()
                    .is_some_and(|(_, b)| b.len() >= CHUNK_FLUSH)
                {
                    self.flush();
                }
                return;
            }
            return self.event(k, update.clone());
        }
        let kind = match kind {
            "tool_call" => session_kind::TOOL_CALL,
            "tool_call_update" => match update.get("status").and_then(Value::as_str) {
                Some("completed" | "failed") => session_kind::TOOL_RESULT,
                _ => session_kind::TOOL_CALL,
            },
            "usage_update" => session_kind::USAGE,
            _ => session_kind::SYSTEM,
        };
        self.event(kind, update.clone());
    }

    fn prompt(&mut self, sid: &str, text: &str) -> u64 {
        self.flush();
        self.request(
            "session/prompt",
            json!({"sessionId": sid, "prompt": [{"type": "text", "text": text}]}),
        )
    }
}

/// Run the agent until its turns are done, it fails, or it is stopped.
/// `on_session` gets the ACP session id as soon as it is known.
pub async fn run(
    reaper: &Reaper,
    spec: AgentSpec,
    emit: Emit<'_>,
    on_session: &(dyn Fn(&str) + Send + Sync),
    steer: &mut mpsc::UnboundedReceiver<String>,
    ctl: &mut Ctl,
    stderr: LineSink,
) -> AgentEnd {
    let sp = match acp::spawn_agent(
        reaper,
        &spec.bin,
        &spec.args,
        &spec.cwd,
        &spec.env,
        false,
        Some(stderr),
    ) {
        Ok(sp) => sp,
        Err(e) => {
            return AgentEnd {
                exit: exit::FAILURE,
                error: Some(format!("cannot start the agent: {e:?}")),
                usage: None,
            };
        }
    };
    let pid = sp.pid;
    let mut exit_rx = sp.exit;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let mut stdin = sp.stdin;
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(sp.stdout).lines();
    let mut d = Driver {
        tx,
        next_id: 1,
        emit,
        chunk: None,
        usage: UsageCollector::default(),
    };

    let init_id = d.request(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
               "clientInfo": {"name": "sandbox-init", "version": env!("CARGO_PKG_VERSION")}}),
    );
    let mut wait = Wait::Init;
    let mut wait_id = init_id;
    let mut sid: Option<String> = None;
    let mut queued: VecDeque<String> = VecDeque::new();
    let mut stopping: Option<Instant> = None;
    let mut exited = false;
    let mut stop_reason = String::new();

    let end: Result<(), String> = loop {
        let grace = stopping.unwrap_or_else(|| Instant::now() + Duration::from_secs(86400 * 365));
        tokio::select! {
            line = lines.next_line() => {
                let line = match line {
                    Ok(Some(l)) => l,
                    _ => {
                        exited = true;
                        break Err("the agent exited".into());
                    }
                };
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    d.event(session_kind::SYSTEM, json!({"invalid_line": line}));
                    continue;
                };
                let method = msg.get("method").and_then(Value::as_str);
                let id = msg.get("id").cloned();
                match (method, id) {
                    (Some("session/update"), None) => {
                        // History replayed by session/load is already in the log.
                        if (wait != Wait::Session || spec.resume.is_none())
                            && let Some(u) = msg.get("params").and_then(|p| p.get("update")) {
                                d.update(u);
                            }
                    }
                    (Some(_), None) => {}
                    (Some("session/request_permission"), Some(id)) => {
                        let params = msg.get("params").cloned().unwrap_or(Value::Null);
                        let options: Vec<Value> = params.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
                        let option = acp::allow_option(&options);
                        d.event(session_kind::SYSTEM, json!({"permission": params.get("toolCall"), "decision": "allow", "option_id": option}));
                        let outcome = match option {
                            Some(o) if stopping.is_none() => json!({"outcome": "selected", "optionId": o}),
                            _ => json!({"outcome": "cancelled"}),
                        };
                        d.send(json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": outcome}}));
                    }
                    (Some(m), Some(id)) => {
                        d.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("{m} is not supported")}}));
                    }
                    (None, Some(id)) => {
                        if id.as_u64() != Some(wait_id) {
                            continue;
                        }
                        if let Some(err) = msg.get("error") {
                            let what = match wait { Wait::Init => "initialize", Wait::Session => if spec.resume.is_some() { "session/load" } else { "session/new" }, Wait::Prompt => "session/prompt" };
                            break Err(format!("{what} failed: {err}"));
                        }
                        let result = msg.get("result").cloned().unwrap_or(Value::Null);
                        match wait {
                            Wait::Init => {
                                d.event(session_kind::SYSTEM, json!({"event": "initialized", "agent_info": result.get("agentInfo")}));
                                let mut params = json!({"cwd": spec.cwd, "mcpServers": spec.mcp_servers});
                                let method = match &spec.resume {
                                    Some(s) => { params["sessionId"] = json!(s); "session/load" }
                                    None => "session/new",
                                };
                                wait_id = d.request(method, params);
                                wait = Wait::Session;
                            }
                            Wait::Session => {
                                let s = spec.resume.clone().or_else(|| result.get("sessionId").and_then(Value::as_str).map(String::from));
                                let Some(s) = s else { break Err("session/new returned no sessionId".into()) };
                                on_session(&s);
                                d.event(session_kind::SYSTEM, json!({"event": if spec.resume.is_some() { "session_loaded" } else { "session" }, "session_id": s}));
                                let kind = if spec.resume.is_some() { session_kind::USER } else { session_kind::PROMPT };
                                d.event(kind, json!({"text": spec.prompt}));
                                wait_id = d.prompt(&s, &spec.prompt);
                                wait = Wait::Prompt;
                                sid = Some(s);
                            }
                            Wait::Prompt => {
                                d.usage.response(&result);
                                stop_reason = result.get("stopReason").and_then(Value::as_str).unwrap_or("unknown").to_string();
                                d.flush();
                                if stopping.is_some() {
                                    break Ok(());
                                }
                                while let Ok(t) = steer.try_recv() { queued.push_back(t); }
                                match queued.pop_front() {
                                    Some(t) => {
                                        let s = sid.clone().unwrap_or_default();
                                        wait_id = d.prompt(&s, &t);
                                    }
                                    None => break Ok(()),
                                }
                            }
                        }
                    }
                    (None, None) => {}
                }
            }
            t = steer.recv(), if stopping.is_none() => {
                if let Some(t) = t { queued.push_back(t); }
            }
            _ = ctl.stopped(), if stopping.is_none() => {
                d.event(session_kind::SYSTEM, json!({"event": "cancel"}));
                match (&sid, &wait) {
                    (Some(s), Wait::Prompt) => {
                        d.send(json!({"jsonrpc": "2.0", "method": "session/cancel", "params": {"sessionId": s}}));
                        stopping = Some(Instant::now() + spec.cancel_grace);
                    }
                    _ => break Ok(()),
                }
            }
            () = tokio::time::sleep_until(grace), if stopping.is_some() => break Ok(()),
            () = tokio::time::sleep(Duration::from_millis(500)), if d.chunk.is_some() => d.flush(),
            st = &mut exit_rx, if !exited => {
                exited = true;
                // Let the reader drain what it wrote; EOF ends the loop next.
                let _ = st;
            }
        }
    };
    d.flush();
    let usage = d.usage.snapshot();
    if let Some(u) = &usage {
        (emit)(
            session_kind::USAGE,
            serde_json::to_value(u).unwrap_or_default(),
        );
    }
    drop(d);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    if !exited
        && tokio::time::timeout(Duration::from_secs(2), &mut exit_rx)
            .await
            .is_err()
    {
        terminate(pid, &mut exit_rx).await;
    }
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    let cancelled = *ctl.cancel.borrow();
    match end {
        _ if cancelled || stopping.is_some() => AgentEnd {
            exit: exit::INTERRUPTED,
            error: None,
            usage,
        },
        Ok(()) if stop_reason == "end_turn" => AgentEnd {
            exit: exit::SUCCESS,
            error: None,
            usage,
        },
        Ok(()) => AgentEnd {
            exit: exit::FAILURE,
            error: Some(format!("the agent stopped: {stop_reason}")),
            usage,
        },
        Err(e) => AgentEnd {
            exit: exit::FAILURE,
            error: Some(e),
            usage,
        },
    }
}
