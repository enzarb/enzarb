//! Shared ACP (Agent Client Protocol) helpers: starting an agent process
//! under the reaper, the permission answer, and usage accounting.
//!
//! Agents are started through [`Reaper::spawn_with`] in their own session (so
//! the whole process group can be killed) and spoken to over stdio as
//! newline-delimited JSON-RPC by [`crate::chat`] (a filtering relay) and
//! [`crate::steps::agent`] (sandbox-init as the ACP client).

use std::io::{self, BufRead};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use enzarb_proto::Usage;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde_json::Value;
use tokio::net::unix::pipe;

use crate::exec::{LineSink, Stop};
use crate::reaper::{ExitStatus, Reaper};

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
pub(crate) struct UsageCollector {
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
    pub(crate) fn update(&mut self, update: &Value) {
        if update.get("sessionUpdate").and_then(Value::as_str) == Some("usage_update") {
            let u = Self::read(update);
            merge_latest(&mut self.cumulative, u);
        }
    }

    /// The `session/prompt` response (one turn).
    pub(crate) fn response(&mut self, response: &Value) {
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
    pub(crate) fn snapshot(&self) -> Option<Usage> {
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

/// A started agent: pid (= its process group), exit status and stdio pipes.
pub(crate) struct Spawned {
    pub pid: u32,
    pub exit: tokio::sync::oneshot::Receiver<ExitStatus>,
    pub stdin: pipe::Sender,
    pub stdout: pipe::Receiver,
}

/// Start `bin args..` in `cwd` in its own session (process group) under the
/// reaper, with `env` added to the inherited environment, or as the whole
/// environment when `clear_env`. stderr lines go to `stderr`.
pub(crate) fn spawn_agent(
    reaper: &Reaper,
    bin: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    clear_env: bool,
    stderr: Option<LineSink>,
) -> Result<Spawned, AcpError> {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    if clear_env {
        cmd.env_clear();
    }
    crate::privs::as_sandbox_user(&mut cmd);
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

/// Pick the allow option to answer a permission request with: the sandbox
/// (gVisor, network policy, scoped tokens) is the security boundary, so every
/// request is allowed (`allow_always` when offered, else `allow_once`).
pub(crate) fn allow_option(options: &[Value]) -> Option<String> {
    ["allow_always", "allow_once"].iter().find_map(|want| {
        options
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(*want))
            .and_then(|o| o.get("optionId").and_then(Value::as_str))
            .map(String::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn usage_cache_figures_add_over_turns() {
        let mut c = UsageCollector::default();
        for (i, r, w) in [(3, 18_000, 1_200), (4, 21_000, 0)] {
            c.response(&json!({"usage": {"inputTokens": i, "outputTokens": 50,
                "cachedReadTokens": r, "cachedWriteTokens": w}}));
        }
        let u = c.snapshot().unwrap();
        assert_eq!(u.input_tokens, Some(7));
        assert_eq!(u.cache_read_input_tokens, Some(39_000));
        assert_eq!(u.cache_creation_input_tokens, Some(1_200));
    }

    #[test]
    fn usage_read_takes_first_source_per_field() {
        let u = UsageCollector::read(&json!({
            "usage": {"inputTokens": 5},
            "_meta": {"usage": {"input_tokens": 9, "cache_read_input_tokens": 40}, "output_tokens": 2},
        }));
        assert_eq!(u.input_tokens, Some(5)); // `usage` wins over `_meta.usage`
        assert_eq!(u.cache_read_input_tokens, Some(40)); // falls through when absent
        assert_eq!(u.output_tokens, Some(2));
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
    fn always_chooses_an_allow_option() {
        let opts = vec![
            json!({"optionId": "always", "kind": "allow_always"}),
            json!({"optionId": "yes", "kind": "allow_once"}),
            json!({"optionId": "no", "kind": "reject_once"}),
        ];
        assert_eq!(allow_option(&opts).as_deref(), Some("always"));
        assert_eq!(allow_option(&opts[1..]).as_deref(), Some("yes"));
        assert_eq!(allow_option(&opts[2..]), None);
        assert_eq!(allow_option(&[]), None);
    }
}
