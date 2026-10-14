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
use enzarb_proto::Usage;
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
    /// Receives the usage reported so far when [`run`] returns, whatever the outcome.
    pub usage: UsageSink,
}

/// A finished turn.
#[derive(Debug)]
pub struct AcpOutput {
    /// ACP stop reason: `end_turn`, `max_tokens`, `max_turn_requests`, `refusal`, `cancelled`.
    pub stop_reason: String,
    /// The agent's message text of the turn, verbatim.
    pub text: String,
    /// Token usage and cost the agent reported, if any.
    pub usage: Option<Usage>,
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

/// Where [`run`] leaves the usage collected so far, also when the run ends in
/// an error, a cancel or a timeout (the spend happened either way).
pub type UsageSink = Arc<Mutex<Option<Usage>>>;

/// Token usage and cost as the agent reports them. Nothing is estimated: a
/// field the agent never sent stays `None`.
///
/// Sources, all optional (the adapter's output varies by version):
/// * the `session/prompt` response: `usage` and `_meta` (per turn: summed);
/// * `usage_update` session updates: `cost` and `_meta` (cumulative: the last
///   one wins). Their `used`/`size` are context-window figures, not spend.
///
/// Per field, the per-turn figure wins over the cumulative one.
#[derive(Default)]
struct UsageCollector {
    turns: Usage,
    cumulative: Usage,
}

/// First integer found under any of `keys`.
fn pick_u64(o: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| {
        let v = o.get(*k)?;
        v.as_u64().or_else(|| {
            let f = v.as_f64()?;
            (f >= 0.0 && f.fract() == 0.0).then_some(f as u64)
        })
    })
}

/// The reported cost in USD: a number, or `{amount, currency}` (USD or no currency).
fn pick_cost(o: &Value) -> Option<f64> {
    for k in ["total_cost_usd", "totalCostUsd", "cost_usd", "costUsd"] {
        if let Some(f) = o.get(k).and_then(Value::as_f64) {
            return f.is_finite().then_some(f).filter(|f| *f >= 0.0);
        }
    }
    let c = o.get("cost")?;
    let (amount, currency) = match c {
        Value::Number(n) => (n.as_f64()?, None),
        Value::Object(_) => (c.get("amount")?.as_f64()?, c.get("currency")),
        _ => return None,
    };
    let usd = currency
        .and_then(Value::as_str)
        .is_none_or(|c| c.eq_ignore_ascii_case("usd"));
    (usd && amount.is_finite() && amount >= 0.0).then_some(amount)
}

/// Model names under `model`, `models` or the keys of `modelUsage`.
fn pick_models(o: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(m) = o.get("model").and_then(Value::as_str) {
        out.push(m.to_string());
    }
    if let Some(a) = o.get("models").and_then(Value::as_array) {
        out.extend(a.iter().filter_map(Value::as_str).map(String::from));
    }
    for k in ["modelUsage", "model_usage"] {
        if let Some(m) = o.get(k).and_then(Value::as_object) {
            out.extend(m.keys().cloned());
        }
    }
    out
}

impl UsageCollector {
    /// Read the usage fields of one message (`PromptResponse` or a session
    /// update). The same figures may be repeated in `usage`, `_meta.usage` and
    /// `_meta`; per field the first one found is used, so nothing is counted twice.
    fn read(v: &Value) -> Usage {
        let meta = v.get("_meta");
        let sources: Vec<&Value> = [
            v.get("usage"),
            meta.and_then(|m| m.get("usage")),
            meta,
            Some(v),
        ]
        .into_iter()
        .flatten()
        .filter(|s| s.is_object())
        .collect();
        let first_u64 = |keys: &[&str]| sources.iter().find_map(|s| pick_u64(s, keys));
        let mut models: Vec<String> = Vec::new();
        for m in sources.iter().flat_map(|s| pick_models(s)) {
            if !models.contains(&m) {
                models.push(m);
            }
        }
        Usage {
            input_tokens: first_u64(&["inputTokens", "input_tokens"]),
            output_tokens: first_u64(&["outputTokens", "output_tokens"]),
            cache_read_input_tokens: first_u64(&[
                "cachedReadTokens",
                "cacheReadTokens",
                "cacheReadInputTokens",
                "cache_read_input_tokens",
            ]),
            cache_creation_input_tokens: first_u64(&[
                "cachedWriteTokens",
                "cacheWriteTokens",
                "cacheCreationInputTokens",
                "cache_creation_input_tokens",
            ]),
            models,
            cost_usd: sources.iter().find_map(|s| pick_cost(s)),
        }
    }

    /// A `session/update` payload; only `usage_update` carries usage.
    fn update(&mut self, update: &Value) {
        if update.get("sessionUpdate").and_then(Value::as_str) == Some("usage_update") {
            let u = Self::read(update);
            merge_latest(&mut self.cumulative, u);
        }
    }

    /// The `session/prompt` response (one turn).
    fn response(&mut self, response: &Value) {
        let u = Self::read(response);
        let t = &mut self.turns;
        let add = |a: &mut Option<u64>, b: Option<u64>| {
            *a = match (*a, b) {
                (Some(x), Some(y)) => Some(x.saturating_add(y)),
                (x, y) => x.or(y),
            }
        };
        add(&mut t.input_tokens, u.input_tokens);
        add(&mut t.output_tokens, u.output_tokens);
        add(&mut t.cache_read_input_tokens, u.cache_read_input_tokens);
        add(
            &mut t.cache_creation_input_tokens,
            u.cache_creation_input_tokens,
        );
        for m in u.models {
            if !t.models.contains(&m) {
                t.models.push(m);
            }
        }
        // `total_cost_usd` is the cumulative cost of the session.
        t.cost_usd = u.cost_usd.or(t.cost_usd);
    }

    /// What was reported so far, `None` when nothing was.
    fn snapshot(&self) -> Option<Usage> {
        let (t, c) = (&self.turns, &self.cumulative);
        let mut models = t.models.clone();
        for m in &c.models {
            if !models.contains(m) {
                models.push(m.clone());
            }
        }
        let u = Usage {
            input_tokens: t.input_tokens.or(c.input_tokens),
            output_tokens: t.output_tokens.or(c.output_tokens),
            cache_read_input_tokens: t.cache_read_input_tokens.or(c.cache_read_input_tokens),
            cache_creation_input_tokens: t
                .cache_creation_input_tokens
                .or(c.cache_creation_input_tokens),
            models,
            cost_usd: t.cost_usd.or(c.cost_usd),
        };
        (!u.is_empty()).then_some(u)
    }
}

/// Cumulative figures: a later report replaces the earlier one, field by field.
fn merge_latest(into: &mut Usage, u: Usage) {
    into.input_tokens = u.input_tokens.or(into.input_tokens);
    into.output_tokens = u.output_tokens.or(into.output_tokens);
    into.cache_read_input_tokens = u.cache_read_input_tokens.or(into.cache_read_input_tokens);
    into.cache_creation_input_tokens = u
        .cache_creation_input_tokens
        .or(into.cache_creation_input_tokens);
    into.cost_usd = u.cost_usd.or(into.cost_usd);
    for m in u.models {
        if !into.models.contains(&m) {
            into.models.push(m);
        }
    }
}

/// Collects the agent's message text: the concatenated `agent_message_chunk`
/// text of the turn, verbatim (the engine parses the trailing ```json block).
#[derive(Default)]
struct Collector {
    text: String,
    usage: UsageCollector,
}

impl Collector {
    fn update(&mut self, update: &Value) {
        self.usage.update(update);
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

/// A started agent: pid (= its process group), exit status and stdio pipes.
pub(crate) struct Spawned {
    pub pid: u32,
    pub exit: tokio::sync::oneshot::Receiver<ExitStatus>,
    pub stdin: pipe::Sender,
    pub stdout: pipe::Receiver,
}

fn spawn(reaper: &Reaper, spec: &AcpSpec, stderr: Option<LineSink>) -> Result<Spawned, AcpError> {
    spawn_agent(reaper, &spec.bin, &spec.cwd, &spec.env, false, stderr)
}

/// Start `bin` in `cwd` in its own session (process group) under the reaper,
/// with `env` added to the inherited environment, or as the whole
/// environment when `clear_env`. stderr lines go to `stderr`.
pub(crate) fn spawn_agent(
    reaper: &Reaper,
    bin: &str,
    cwd: &Path,
    env: &[(String, String)],
    clear_env: bool,
    stderr: Option<LineSink>,
) -> Result<Spawned, AcpError> {
    let mut cmd = Command::new(bin);
    if clear_env {
        cmd.env_clear();
    }
    cmd.current_dir(cwd)
        .envs(env.iter().map(|(k, v)| (k, v)))
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
        .map_err(|e| AcpError::Failed(format!("cannot run {bin}: {e}")))?;
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
    let collector2 = collector.clone();
    let conn = Client
        .builder()
        .name("sandbox-init")
        .on_receive_notification(on_update, agent_client_protocol::on_receive_notification!())
        .on_receive_request(on_permission, agent_client_protocol::on_receive_request!())
        .connect_with(
            ByteStreams::new(sp.stdin.compat_write(), sp.stdout.compat()),
            async |cx: ConnectionTo<Agent>| {
                Ok(drive(
                    cx,
                    &prompt,
                    &cwd,
                    ctl,
                    grace,
                    &emit2,
                    &cancelling2,
                    &collector2,
                )
                .await)
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
    let usage = collector.lock().unwrap().usage.snapshot();
    *spec.usage.lock().unwrap() = usage.clone();
    let mut result = result?;
    result.text = std::mem::take(&mut collector.lock().unwrap().text);
    result.usage = usage;
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
    collector: &Arc<Mutex<Collector>>,
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
            if let Ok(v) = serde_json::to_value(&r) {                collector.lock().unwrap().usage.response(&v);
            }
            return Ok(AcpOutput {
                stop_reason: stop_reason(&r),
                text: String::new(),
                usage: None,
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

    fn usage_update(cost: Option<f64>) -> Value {
        let mut u = json!({"sessionUpdate": "usage_update", "used": 5000, "size": 200000});
        if let Some(c) = cost {
            u["cost"] = json!({"amount": c, "currency": "USD"});
        }
        u
    }

    #[test]
    fn usage_unknown_when_nothing_reported() {
        let mut c = UsageCollector::default();
        c.update(&usage_update(None)); // used/size are context figures, not spend
        c.response(&json!({"stopReason": "end_turn"}));
        assert_eq!(c.snapshot(), None);
    }

    #[test]
    fn usage_turn_tokens_and_cumulative_cost() {
        let mut c = UsageCollector::default();
        c.update(&usage_update(Some(0.10)));
        c.update(&usage_update(Some(0.25))); // cumulative: replaces, not 0.35
        c.response(&json!({
            "stopReason": "end_turn",
            "usage": {"inputTokens": 100, "outputTokens": 40, "totalTokens": 140, "cachedReadTokens": 900},
        }));
        let u = c.snapshot().unwrap();
        assert_eq!(u.input_tokens, Some(100));
        assert_eq!(u.output_tokens, Some(40));
        assert_eq!(u.cache_read_input_tokens, Some(900));
        assert_eq!(u.cache_creation_input_tokens, None); // not reported: unknown, not 0
        assert_eq!(u.cost_usd, Some(0.25));
    }

    #[test]
    fn usage_turns_add_and_reported_cost_wins() {
        let mut c = UsageCollector::default();
        c.update(&usage_update(Some(0.25)));
        for (i, o) in [(10, 1), (20, 2)] {
            c.response(&json!({"usage": {"inputTokens": i, "outputTokens": o}}));
        }
        c.response(
            &json!({"_meta": {"total_cost_usd": 0.5, "modelUsage": {"claude-sonnet-5-5": {}}}}),
        );
        let u = c.snapshot().unwrap();
        assert_eq!((u.input_tokens, u.output_tokens), (Some(30), Some(3)));
        assert_eq!(u.cost_usd, Some(0.5));
        assert_eq!(u.models, vec!["claude-sonnet-5-5".to_string()]);
    }

    #[test]
    fn usage_meta_snake_case_and_no_double_count() {
        let mut c = UsageCollector::default();
        // the same figures in `usage` and `_meta.usage` are counted once
        c.response(&json!({
            "usage": {"inputTokens": 7},
            "_meta": {"usage": {"input_tokens": 7, "output_tokens": 3,
                "cache_read_input_tokens": 11, "cache_creation_input_tokens": 2}, "model": "m1"},
        }));
        let u = c.snapshot().unwrap();
        assert_eq!(u.input_tokens, Some(7));
        assert_eq!(u.output_tokens, Some(3));
        assert_eq!(u.cache_read_input_tokens, Some(11));
        assert_eq!(u.cache_creation_input_tokens, Some(2));
        assert_eq!(u.models, vec!["m1".to_string()]);
        assert_eq!(u.cost_usd, None);
    }

    #[test]
    fn usage_ignores_non_usd_cost() {
        let mut c = UsageCollector::default();
        c.update(
            &json!({"sessionUpdate": "usage_update", "cost": {"amount": 3.0, "currency": "EUR"}}),
        );
        assert_eq!(c.snapshot(), None);
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
