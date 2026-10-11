//! One agent run over the Agent Client Protocol (ACP).
//!
//! sandbox-init spawns the adapter (`claude-agent-acp`, see
//! `ENZARB_ACP_AGENT_BIN`) through [`Reaper::spawn_with`] in its own session
//! (so the whole process group can be killed) and acts as the ACP *client*
//! over the child's stdio:
//!
//! 1. `initialize` (protocol v1, no client capabilities: no fs, no terminal);
//! 2. `session/new` with `cwd` = the run worktree and no MCP servers;
//! 3. `session/prompt` with the rendered prompt. While it runs, every
//!    `session/update` is emitted as a transcript record and the agent text is
//!    collected, and every `session/request_permission` is answered by the
//!    [`Policy`] (`allow_once` or `reject_once`, never a persistent option);
//! 4. on cancel or timeout: `session/cancel`, wait up to `cancel_grace` for the
//!    turn to end, then SIGTERM/SIGKILL of the process group.

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, TextContent,
};
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::{Value, json};
use tokio::net::unix::pipe;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use std::io::{self, BufRead};
use std::os::unix::process::CommandExt;

use crate::exec::{Ctl, LineSink, Stop, terminate};
use crate::policy::Policy;
use crate::reaper::{ExitStatus, Reaper};

/// Receives transcript records `(kind, fields)`.
pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// What to run.
pub struct AcpSpec {
    pub bin: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub prompt: String,
    /// How long the agent gets to end its turn after `session/cancel`.
    pub cancel_grace: Duration,
}

/// A finished turn.
#[derive(Debug)]
pub struct AcpOutput {
    /// ACP stop reason: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`.
    pub stop_reason: String,
    /// The agent's message text of the turn, verbatim.
    pub text: String,
}

#[derive(Debug)]
pub enum AcpError {
    /// Cancelled or timed out (the agent was stopped).
    Stopped(Stop),
    /// Anything else: spawn failure, protocol error, agent crash.
    Failed(String),
}

impl From<Stop> for AcpError {
    fn from(s: Stop) -> Self {
        Self::Stopped(s)
    }
}

/// Collects the agent's message text: the concatenated `agent_message_chunk`
/// text of the turn, verbatim (the engine parses the trailing ```json block).
#[derive(Default)]
struct Collector {
    text: String,
}

impl Collector {
    fn update(&mut self, update: &Value) {
        if update.get("sessionUpdate").and_then(Value::as_str) != Some("agent_message_chunk") {
            return;
        }
        let c = update.get("content");
        if c.and_then(|c| c.get("type")).and_then(Value::as_str) == Some("text")
            && let Some(t) = c.and_then(|c| c.get("text")).and_then(Value::as_str)
        {
            self.text.push_str(t);
        }
    }
}

struct Spawned {
    pid: u32,
    exit: tokio::sync::oneshot::Receiver<ExitStatus>,
    stdin: pipe::Sender,
    stdout: pipe::Receiver,
}

fn spawn(reaper: &Reaper, spec: &AcpSpec, stderr: Option<LineSink>) -> Result<Spawned, AcpError> {
    let mut cmd = Command::new(&spec.bin);
    cmd.current_dir(&spec.cwd)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: `setsid` is async-signal-safe and the closure does nothing else.
    unsafe {
        cmd.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
    }
    let (mut child, exit) = reaper
        .spawn_with(|| {
            let c = cmd.spawn()?;
            let pid = c.id();
            Ok((c, pid))
        })
        .map_err(|e| AcpError::Failed(format!("cannot run {}: {e}", spec.bin)))?;
    let pid = child.id();
    let (stdin, stdout, err) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
    drop(child); // the reaper owns waiting
    let (Some(stdin), Some(stdout)) = (stdin, stdout) else {
        return Err(AcpError::Failed("agent pipes missing".into()));
    };
    if let Some(err) = err {
        std::thread::spawn(move || {
            for line in io::BufReader::new(err).split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line);
                if let Some(f) = &stderr {
                    f(line.trim_end_matches('\r'));
                }
            }
        });
    }
    let pipes = || -> io::Result<(pipe::Sender, pipe::Receiver)> {
        Ok((
            pipe::Sender::from_owned_fd(OwnedFd::from(stdin))?,
            pipe::Receiver::from_owned_fd(OwnedFd::from(stdout))?,
        ))
    };
    match pipes() {
        Ok((stdin, stdout)) => Ok(Spawned {
            pid,
            exit,
            stdin,
            stdout,
        }),
        Err(e) => {
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
            Err(AcpError::Failed(format!("agent pipes: {e}")))
        }
    }
}

fn acp_err(what: &str, e: agent_client_protocol::Error) -> AcpError {
    AcpError::Failed(format!("{what}: {e}"))
}

/// Pick the option to answer a permission request with.
fn choose_option(options: &[Value], allow: bool) -> Option<String> {
    let want = if allow { "allow_once" } else { "reject_once" };
    options
        .iter()
        .find(|o| o.get("kind").and_then(Value::as_str) == Some(want))
        .and_then(|o| o.get("optionId").and_then(Value::as_str))
        .map(String::from)
}

/// Run one prompt to completion (or until stopped), then stop the agent.
pub async fn run(
    reaper: &Reaper,
    spec: AcpSpec,
    policy: Policy,
    ctl: &mut Ctl,
    emit: Emit,
    stderr: Option<LineSink>,
) -> Result<AcpOutput, AcpError> {
    let sp = spawn(reaper, &spec, stderr)?;
    let pid = sp.pid;
    let mut exit = sp.exit;
    let cancelling = Arc::new(AtomicBool::new(false));
    let collector = Arc::new(Mutex::new(Collector::default()));
    let worktree = spec.cwd.clone();

    let on_update = {
        let emit = emit.clone();
        let collector = collector.clone();
        async move |n: SessionNotification, _cx: ConnectionTo<Agent>| {
            if let Ok(v) = serde_json::to_value(&n.update) {
                collector.lock().unwrap().update(&v);
                emit("update", json!({"session_id": n.session_id, "update": v}));
            }
            Ok(())
        }
    };
    let on_permission = {
        let emit = emit.clone();
        let cancelling = cancelling.clone();
        async move |req: RequestPermissionRequest,
                    responder: agent_client_protocol::Responder<RequestPermissionResponse>,
                    _cx: ConnectionTo<Agent>| {
            let v = serde_json::to_value(&req).unwrap_or(Value::Null);
            let tool_call = v.get("toolCall").cloned().unwrap_or(Value::Null);
            let options: Vec<Value> = v
                .get("options")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let decision = policy.decide(&tool_call, &worktree);
            let option = choose_option(&options, decision.allow);
            let record = json!({
                "tool_call_id": tool_call.get("toolCallId"),
                "tool_kind": tool_call.get("kind"),
                "title": tool_call.get("title"),
                "input": tool_call.get("rawInput"),
                "decision": if decision.allow { "allow" } else { "deny" },
                "rule": decision.rule,
                "reason": decision.reason,
                "option_id": option,
            });
            emit("permission", record);
            let outcome = match option {
                Some(id) if !cancelling.load(Ordering::Relaxed) => {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id))
                }
                _ => RequestPermissionOutcome::Cancelled,
            };
            responder.respond(RequestPermissionResponse::new(outcome))
        }
    };

    let prompt = spec.prompt.clone();
    let cwd = spec.cwd.clone();
    let grace = spec.cancel_grace;
    let emit2 = emit.clone();
    let cancelling2 = cancelling.clone();
    let conn = Client
        .builder()
        .name("sandbox-init")
        .on_receive_notification(on_update, agent_client_protocol::on_receive_notification!())
        .on_receive_request(on_permission, agent_client_protocol::on_receive_request!())
        .connect_with(
            ByteStreams::new(sp.stdin.compat_write(), sp.stdout.compat()),
            async |cx: ConnectionTo<Agent>| {
                Ok(drive(cx, &prompt, &cwd, ctl, grace, &emit2, &cancelling2).await)
            },
        );
    let mut conn = Box::pin(conn);

    // The exit status once `exit` has resolved (a oneshot must not be polled again).
    let mut exited: Option<Option<ExitStatus>> = None;
    let result = tokio::select! {
        biased;
        r = &mut conn => flatten(r),
        st = &mut exit => {
            let st = st.ok();
            exited = Some(st);
            // The agent ended; it may have answered first.
            match tokio::time::timeout(Duration::from_secs(2), &mut conn).await {
                Ok(r) => flatten(r),
                Err(_) => Err(AcpError::Failed(describe_exit(st))),
            }
        }
    };
    // Closing the connection closes the agent's stdin: give it a moment to
    // exit, then stop whatever is left of its process group.
    drop(conn);
    let own_exit = match exited {
        Some(st) => st,
        None => match tokio::time::timeout(Duration::from_secs(2), &mut exit).await {
            Ok(st) => st.ok(),
            Err(_) => {
                terminate(pid, &mut exit).await;
                None
            }
        },
    };
    // The connection broke because the agent died: say how.
    let result = match (result, own_exit) {
        (Err(AcpError::Failed(m)), Some(st)) if st != ExitStatus::Exited(0) => Err(
            AcpError::Failed(format!("{} ({m})", describe_exit(Some(st)))),
        ),
        (r, _) => r,
    };
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    let mut result = result?;
    result.text = std::mem::take(&mut collector.lock().unwrap().text);
    Ok(result)
}

fn describe_exit(st: Option<ExitStatus>) -> String {
    match st {
        Some(ExitStatus::Exited(c)) => format!("agent exited unexpectedly with code {c}"),
        Some(ExitStatus::Signaled(s)) => format!("agent was killed by signal {s}"),
        None => "agent exited unexpectedly".into(),
    }
}

fn flatten(
    r: Result<Result<AcpOutput, AcpError>, agent_client_protocol::Error>,
) -> Result<AcpOutput, AcpError> {
    match r {
        Ok(inner) => inner,
        Err(e) => Err(AcpError::Failed(format!("agent connection: {e}"))),
    }
}

/// The client side of the conversation.
#[allow(clippy::too_many_arguments)]
async fn drive(
    cx: ConnectionTo<Agent>,
    prompt: &str,
    cwd: &Path,
    ctl: &mut Ctl,
    grace: Duration,
    emit: &Emit,
    cancelling: &Arc<AtomicBool>,
) -> Result<AcpOutput, AcpError> {
    let init = cx
        .send_request(InitializeRequest::new(ProtocolVersion::V1))
        .block_task();
    let init = tokio::select! {
        r = init => r.map_err(|e| acp_err("initialize", e))?,
        s = ctl.stopped() => return Err(s.into()),
    };
    emit(
        "acp",
        json!({
            "event": "initialized",
            "agent_info": serde_json::to_value(&init.agent_info).unwrap_or(Value::Null),
            "protocol_version": serde_json::to_value(init.protocol_version).unwrap_or(Value::Null),
        }),
    );
    let new = cx
        .send_request(NewSessionRequest::new(cwd.to_path_buf()))
        .block_task();
    let new = tokio::select! {
        r = new => r.map_err(|e| acp_err("session/new", e))?,
        s = ctl.stopped() => return Err(s.into()),
    };
    let session_id = new.session_id.clone();
    emit(
        "acp",
        json!({
            "event": "session",
            "session_id": session_id,
            "modes": serde_json::to_value(&new.modes).unwrap_or(Value::Null),
        }),
    );
    let turn = cx
        .send_request(PromptRequest::new(
            session_id.clone(),
            vec![ContentBlock::Text(TextContent::new(prompt.to_string()))],
        ))
        .block_task();
    tokio::pin!(turn);
    let stopped = tokio::select! {
        r = &mut turn => {
            let r = r.map_err(|e| acp_err("session/prompt", e))?;
            return Ok(AcpOutput {
                stop_reason: stop_reason(&r),
                text: String::new(),
            });
        }
        s = ctl.stopped() => s,
    };
    // Ask the agent to stop and give it a moment to end the turn cleanly.
    cancelling.store(true, Ordering::Relaxed);
    emit(
        "acp",
        json!({"event": "cancel", "reason": format!("{stopped:?}")}),
    );
    let _ = cx.send_notification(CancelNotification::new(session_id));
    let _ = tokio::time::timeout(grace, &mut turn).await;
    Err(stopped.into())
}

fn stop_reason(r: &agent_client_protocol::schema::v1::PromptResponse) -> String {
    serde_json::to_value(r.stop_reason)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collector_concatenates_message_chunks_only() {
        let chunk = |t: &str| json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": t}});
        let mut c = Collector::default();
        c.update(&chunk("Hel"));
        c.update(&chunk("lo\n"));
        c.update(&json!({"sessionUpdate": "tool_call", "toolCallId": "1"}));
        c.update(&json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "hmm"}}));
        c.update(&chunk("done\n```json\n{}\n```"));
        c.update(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "image"}}));
        assert_eq!(c.text, "Hello\ndone\n```json\n{}\n```");
    }

    #[test]
    fn chooses_one_shot_options_only() {
        let opts = vec![
            json!({"optionId": "always", "kind": "allow_always"}),
            json!({"optionId": "yes", "kind": "allow_once"}),
            json!({"optionId": "no", "kind": "reject_once"}),
        ];
        assert_eq!(choose_option(&opts, true).as_deref(), Some("yes"));
        assert_eq!(choose_option(&opts, false).as_deref(), Some("no"));
        assert_eq!(choose_option(&opts[..1], true), None);
        assert_eq!(choose_option(&[], false), None);
    }
}
