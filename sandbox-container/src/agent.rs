//! The real [`RunManager`]: idempotent agent runs in git worktrees
//! (contracts §3, §4, §12).
//!
//! Lifecycle of a run (`run_start`):
//! 1. `queued`: persisted under `<workspace>/.enzarb/runs/<run_id>/`, waiting for a slot;
//! 2. `running`: ensure the clone, create worktree `run-<run_id>` on `branch`,
//!    run `claude -p` in it, optionally commit leftovers and push `branch`;
//! 3. terminal (`succeeded | failed | cancelled`): `state.json` is final and
//!    the [`RunFinished`] callback is delivered with retries
//!    (`callback.json` exists until a 2xx).
//!
//! On-disk layout:
//! * `<ws>/repos/<name>-<hash>/`: clones (one per repo URL);
//! * `<ws>/worktrees/run-<run_id>/`: the run's worktree;
//! * `<ws>/.enzarb/runs/<run_id>/state.json`: current record;
//! * `.../request.json` (0600): the request without `clone_token` (needed to
//!   finish a run after a restart);
//! * `.../transcript.jsonl`: one JSON object per line (state changes, git and
//!   claude stderr lines, the claude result);
//! * `.../callback.json` (0600): the pending callback, removed once delivered.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine as _;
use enzarb_proto::session::RunInfo;
use enzarb_proto::{RunFinished, RunOutcome, RunPhase, RunStart};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::time::Instant;

use crate::capture::write_atomic;
use crate::config::Config;
use crate::exec::{self, CmdSpec, Ctl, LineSink, Output, Stop};
use crate::reaper::Reaper;
use crate::runs::{RunManager, SessionCtx};

/// Largest `summary` pushed in a `run_state` (the callback carries it whole).
const MAX_STATE_SUMMARY: usize = 256 * 1024;
const MAX_CALLBACK_BACKOFF: Duration = Duration::from_secs(60);
/// Stop retrying a callback after this long (the executor polls as a fallback).
const CALLBACK_GIVE_UP: Duration = Duration::from_secs(24 * 3600);

/// `state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RunRecord {
    run_id: String,
    state: RunPhase,
    repo_url: String,
    base_ref: String,
    branch: String,
    push: bool,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    pushed: bool,
    created_ms: u64,
    #[serde(default)]
    started_ms: Option<u64>,
    #[serde(default)]
    finished_ms: Option<u64>,
}

impl RunRecord {
    fn info(&self) -> RunInfo {
        let summary = self.summary.as_ref().map(|s| {
            if s.len() <= MAX_STATE_SUMMARY {
                s.clone()
            } else {
                let mut cut = MAX_STATE_SUMMARY;
                while !s.is_char_boundary(cut) {
                    cut -= 1;
                }
                s[..cut].to_string()
            }
        });
        RunInfo {
            run_id: self.run_id.clone(),
            state: self.state,
            exit_code: self.exit_code,
            summary,
        }
    }
}

/// `callback.json`.
#[derive(Serialize, Deserialize)]
struct PendingCallback {
    url: String,
    token: String,
    body: RunFinished,
    first_attempt_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---- transcript ----

struct Transcript {
    path: PathBuf,
    inner: Mutex<TranscriptInner>,
}

#[derive(Default)]
struct TranscriptInner {
    subs: Vec<mpsc::UnboundedSender<Value>>,
    closed: bool,
}

impl Transcript {
    fn new(path: PathBuf, closed: bool) -> Self {
        Self {
            path,
            inner: Mutex::new(TranscriptInner {
                subs: Vec::new(),
                closed,
            }),
        }
    }

    fn append(&self, kind: &str, mut v: Value) {
        use std::io::Write;
        v["ts"] = json!(now_ms());
        v["kind"] = json!(kind);
        let mut g = self.inner.lock().unwrap();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{v}");
        }
        g.subs.retain(|s| s.send(v.clone()).is_ok());
    }

    /// Replays the file, then follows live (until [`close`](Self::close)).
    fn subscribe(&self) -> mpsc::UnboundedReceiver<Value> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut g = self.inner.lock().unwrap();
        if let Ok(text) = std::fs::read_to_string(&self.path) {
            for l in text.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(l) {
                    let _ = tx.send(v);
                }
            }
        }
        if !g.closed {
            g.subs.push(tx);
        }
        rx
    }

    fn close(&self) {
        let mut g = self.inner.lock().unwrap();
        g.closed = true;
        g.subs.clear();
    }
}

struct Run {
    id: String,
    dir: PathBuf,
    rec: Mutex<RunRecord>,
    cancel: watch::Sender<bool>,
    transcript: Arc<Transcript>,
}

impl Run {
    fn state(&self) -> RunPhase {
        self.rec.lock().unwrap().state
    }
    fn info(&self) -> RunInfo {
        self.rec.lock().unwrap().info()
    }
    fn persist(&self, rec: &RunRecord) {
        let data = serde_json::to_vec_pretty(rec).unwrap_or_default();
        if let Err(e) = write_atomic(&self.dir.join("state.json"), &data, 0o644) {
            tracing::error!(run = %self.id, error = %e, "cannot write state.json");
        }
    }
}

// ---- manager ----

struct Inner {
    cfg: Config,
    reaper: Arc<Reaper>,
    runs_dir: PathBuf,
    runs: Mutex<HashMap<String, Arc<Run>>>,
    sessions: Mutex<HashMap<u64, SessionCtx>>,
    slots: Semaphore,
    repo_locks: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
    http: reqwest::Client,
}

/// The agent run manager. Cheap to clone.
#[derive(Clone)]
pub struct AgentRunManager {
    inner: Arc<Inner>,
}

impl AgentRunManager {
    pub fn new(cfg: Config, reaper: Arc<Reaper>) -> Self {
        let runs_dir = cfg.workspace.join(".enzarb/runs");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client");
        Self {
            inner: Arc::new(Inner {
                slots: Semaphore::new(cfg.max_runs.max(1)),
                cfg,
                reaper,
                runs_dir,
                runs: Mutex::default(),
                sessions: Mutex::default(),
                repo_locks: Mutex::default(),
                http,
            }),
        }
    }

    /// Load runs from disk: runs that were interrupted by a restart become
    /// `failed`, and pending callbacks are delivered. Call once at startup.
    pub fn recover(&self) {
        let inner = &self.inner;
        let Ok(entries) = std::fs::read_dir(&inner.runs_dir) else {
            return;
        };
        for e in entries.flatten() {
            let dir = e.path();
            let Ok(text) = std::fs::read_to_string(dir.join("state.json")) else {
                continue;
            };
            let Ok(rec) = serde_json::from_str::<RunRecord>(&text) else {
                tracing::warn!(dir = %dir.display(), "unreadable state.json");
                continue;
            };
            let terminal = rec.state.is_terminal();
            let run = Arc::new(Run {
                id: rec.run_id.clone(),
                transcript: Arc::new(Transcript::new(dir.join("transcript.jsonl"), terminal)),
                dir: dir.clone(),
                rec: Mutex::new(rec),
                cancel: watch::channel(false).0,
            });
            inner
                .runs
                .lock()
                .unwrap()
                .insert(run.id.clone(), run.clone());
            if !terminal {
                tracing::warn!(run = %run.id, "run interrupted by restart");
                inner.finalize(
                    &run,
                    Finish::failed("sandbox-init restarted while the run was in progress"),
                );
            } else if dir.join("callback.json").exists() {
                inner.spawn_callback(run.dir.clone(), run.id.clone());
            }
        }
    }
}

#[async_trait]
impl RunManager for AgentRunManager {
    async fn start(&self, ctx: &SessionCtx, req: RunStart) {
        let inner = &self.inner;
        inner.remember(ctx);
        let id = req.run_id.clone();
        if let Err(msg) = validate(&req) {
            let _ = ctx.session.send_run_error(&id, "invalid_request", &msg);
            return;
        }
        let mut runs = inner.runs.lock().unwrap();
        if let Some(run) = runs.get(&id).cloned() {
            drop(runs);
            let state = run.state();
            match ctx.session.accept_run(&id, state, true, req.log_channel) {
                Ok(log) => inner.attach_log(&run, log),
                Err(e) => {
                    let _ = ctx
                        .session
                        .send_run_error(&id, "invalid_request", &e.to_string());
                }
            }
            return;
        }
        let dir = inner.runs_dir.join(&id);
        let rec = RunRecord {
            run_id: id.clone(),
            state: RunPhase::Queued,
            repo_url: req.repo_url.clone(),
            base_ref: req.base_ref.clone(),
            branch: req.branch.clone(),
            push: req.push,
            exit_code: None,
            summary: None,
            head_sha: String::new(),
            pushed: false,
            created_ms: now_ms(),
            started_ms: None,
            finished_ms: None,
        };
        let mut stored = req.clone();
        stored.clone_token = String::new();
        let setup = std::fs::create_dir_all(&dir)
            .and_then(|()| {
                write_atomic(
                    &dir.join("request.json"),
                    &serde_json::to_vec_pretty(&stored).unwrap_or_default(),
                    0o600,
                )
            })
            .and_then(|()| {
                write_atomic(
                    &dir.join("state.json"),
                    &serde_json::to_vec_pretty(&rec).unwrap_or_default(),
                    0o644,
                )
            });
        if let Err(e) = setup {
            let _ = std::fs::remove_dir_all(&dir);
            let _ =
                ctx.session
                    .send_run_error(&id, "internal", &format!("cannot persist run: {e}"));
            return;
        }
        let log = match ctx
            .session
            .accept_run(&id, RunPhase::Queued, false, req.log_channel)
        {
            Ok(l) => l,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                let _ = ctx
                    .session
                    .send_run_error(&id, "invalid_request", &e.to_string());
                return;
            }
        };
        let run = Arc::new(Run {
            id: id.clone(),
            transcript: Arc::new(Transcript::new(dir.join("transcript.jsonl"), false)),
            dir,
            rec: Mutex::new(rec),
            cancel: watch::channel(false).0,
        });
        run.transcript
            .append("state", json!({"state": "queued", "branch": req.branch}));
        runs.insert(id, run.clone());
        drop(runs);
        inner.attach_log(&run, log);
        let inner = inner.clone();
        tokio::spawn(async move { inner.work(run, req).await });
    }

    async fn status(&self, ctx: &SessionCtx, run_id: &str) {
        self.inner.remember(ctx);
        let run = self.inner.runs.lock().unwrap().get(run_id).cloned();
        match run {
            Some(r) => {
                let _ = ctx.session.send_run_state(&r.info());
            }
            None => {
                let _ = ctx.session.send_run_error(
                    run_id,
                    "unknown_run",
                    &format!("unknown run {run_id}"),
                );
            }
        }
    }

    async fn cancel(&self, ctx: &SessionCtx, run_id: &str) {
        self.inner.remember(ctx);
        let run = self.inner.runs.lock().unwrap().get(run_id).cloned();
        match run {
            Some(r) if r.state().is_terminal() => {
                let _ = ctx.session.send_run_state(&r.info());
            }
            Some(r) => {
                let _ = r.cancel.send(true);
            }
            None => {
                let _ = ctx.session.send_run_error(
                    run_id,
                    "unknown_run",
                    &format!("unknown run {run_id}"),
                );
            }
        }
    }

    async fn session_closed(&self, session_id: u64) {
        self.inner.sessions.lock().unwrap().remove(&session_id);
    }
}

fn valid_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('-')
        && !s.starts_with('/')
        && !s.ends_with('/')
        && !s.ends_with(".lock")
        && !s.contains("..")
        && !s.contains("@{")
        && !s
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
}

fn validate(r: &RunStart) -> Result<(), String> {
    let id = &r.run_id;
    if id.is_empty()
        || id.len() > 128
        || id.starts_with('.')
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err("run_id must be 1-128 chars of [A-Za-z0-9._-] and not start with '.'".into());
    }
    if r.repo_url.is_empty() || r.repo_url.starts_with('-') {
        return Err("invalid repo_url".into());
    }
    if !valid_ref(&r.branch) {
        return Err(format!("invalid branch {:?}", r.branch));
    }
    if !valid_ref(&r.base_ref) {
        return Err(format!("invalid base_ref {:?}", r.base_ref));
    }
    Ok(())
}

// ---- the run itself ----

/// Terminal result of a run.
struct Finish {
    outcome: RunOutcome,
    exit_code: Option<i32>,
    summary: String,
    head_sha: String,
    pushed: bool,
}

impl Finish {
    fn failed(msg: impl Into<String>) -> Self {
        Self {
            outcome: RunOutcome::Failed,
            exit_code: None,
            summary: msg.into(),
            head_sha: String::new(),
            pushed: false,
        }
    }
}

/// Why `execute` stopped early.
enum Abort {
    Stop(Stop),
    Fail(String),
}

impl From<Stop> for Abort {
    fn from(s: Stop) -> Self {
        Self::Stop(s)
    }
}

fn redact(s: &str, secrets: &[String]) -> String {
    let mut out = s.to_string();
    for sec in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(sec.as_str(), "***");
    }
    out
}

fn repo_dir_name(url: &str) -> String {
    let tail: String = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let hash = Sha256::digest(url.as_bytes());
    let hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{tail}-{hex}")
}

struct ClaudeResult {
    text: String,
    is_error: bool,
}

/// Extract the final result from `claude -p --output-format json` (a single
/// object), a JSON array of messages, or `stream-json` lines.
fn parse_claude_output(stdout: &[u8]) -> Option<ClaudeResult> {
    fn from_obj(v: &Value) -> Option<ClaudeResult> {
        let text = v.get("result")?.as_str()?.to_string();
        Some(ClaudeResult {
            text,
            is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        })
    }
    let text = String::from_utf8_lossy(stdout);
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        match &v {
            Value::Array(items) => {
                return items
                    .iter()
                    .rev()
                    .find(|i| i.get("type").and_then(Value::as_str) == Some("result"))
                    .and_then(from_obj);
            }
            _ => return from_obj(&v),
        }
    }
    text.lines()
        .rev()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .find(|v| v.get("type").and_then(Value::as_str) == Some("result"))
        .and_then(|v| from_obj(&v))
}

impl Inner {
    fn remember(&self, ctx: &SessionCtx) {
        self.sessions.lock().unwrap().insert(ctx.id, ctx.clone());
    }

    fn publish(&self, run: &Run) {
        let info = run.info();
        let sessions: Vec<SessionCtx> = self.sessions.lock().unwrap().values().cloned().collect();
        for s in sessions {
            let _ = s.session.send_run_state(&info);
        }
    }

    fn attach_log(&self, run: &Run, log: Option<enzarb_proto::session::ChannelHandle>) {
        let Some(log) = log else { return };
        let mut rx = run.transcript.subscribe();
        tokio::spawn(async move {
            let (mut send, _recv) = log.split();
            while let Some(v) = rx.recv().await {
                if send.send_json(&v).await.is_err() {
                    return;
                }
            }
            let _ = send.close(0, "finished");
        });
    }

    fn set_state(&self, run: &Run, f: impl FnOnce(&mut RunRecord)) {
        {
            let mut rec = run.rec.lock().unwrap();
            f(&mut rec);
            run.persist(&rec);
        }
        self.publish(run);
    }

    async fn work(self: Arc<Self>, run: Arc<Run>, req: RunStart) {
        let mut cancel = run.cancel.subscribe();
        let permit = tokio::select! {
            p = self.slots.acquire() => p.ok(),
            _ = async { while !*cancel.borrow() { if cancel.changed().await.is_err() { std::future::pending::<()>().await } } } => None,
        };
        let Some(_permit) = permit else {
            self.finalize(
                &run,
                Finish {
                    outcome: RunOutcome::Cancelled,
                    exit_code: None,
                    summary: "cancelled before start".into(),
                    head_sha: String::new(),
                    pushed: false,
                },
            );
            return;
        };
        self.set_state(&run, |r| {
            r.state = RunPhase::Running;
            r.started_ms = Some(now_ms());
        });
        run.transcript.append("state", json!({"state": "running"}));
        let secs = if req.timeout_secs == 0 {
            self.cfg.default_timeout
        } else {
            Duration::from_secs(req.timeout_secs)
        };
        let mut ctl = Ctl {
            cancel: run.cancel.subscribe(),
            deadline: Instant::now() + secs,
        };
        let finish = match self.execute(&run, &req, &mut ctl).await {
            Ok(f) => f,
            Err(Abort::Stop(Stop::Cancelled)) => Finish {
                outcome: RunOutcome::Cancelled,
                exit_code: None,
                summary: "cancelled".into(),
                head_sha: String::new(),
                pushed: false,
            },
            Err(Abort::Stop(Stop::TimedOut)) => {
                Finish::failed(format!("timed out after {} s", secs.as_secs()))
            }
            Err(Abort::Fail(m)) => Finish::failed(m),
        };
        self.finalize(&run, finish);
    }

    /// Persist the terminal state and the pending callback, then deliver it.
    fn finalize(self: &Arc<Self>, run: &Arc<Run>, f: Finish) {
        let (branch, request) = {
            let rec = run.rec.lock().unwrap();
            (rec.branch.clone(), run.dir.join("request.json"))
        };
        let body = RunFinished {
            run_id: run.id.clone(),
            outcome: f.outcome,
            exit_code: f.exit_code,
            summary: f.summary.clone(),
            branch,
            head_sha: f.head_sha.clone(),
            pushed: f.pushed,
        };
        run.transcript.append(
            "finished",
            json!({"outcome": f.outcome, "exit_code": f.exit_code, "head_sha": f.head_sha, "pushed": f.pushed}),
        );
        let pending = std::fs::read_to_string(request)
            .ok()
            .and_then(|t| serde_json::from_str::<RunStart>(&t).ok())
            .map(|r| PendingCallback {
                url: r.callback_url,
                token: r.callback_token,
                body,
                first_attempt_ms: now_ms(),
            });
        match pending {
            Some(p) => {
                let data = serde_json::to_vec_pretty(&p).unwrap_or_default();
                if let Err(e) = write_atomic(&run.dir.join("callback.json"), &data, 0o600) {
                    tracing::error!(run = %run.id, error = %e, "cannot persist callback");
                }
            }
            None => tracing::error!(run = %run.id, "request.json missing; no callback"),
        }
        self.set_state(run, |r| {
            r.state = match f.outcome {
                RunOutcome::Succeeded => RunPhase::Succeeded,
                RunOutcome::Failed => RunPhase::Failed,
                RunOutcome::Cancelled => RunPhase::Cancelled,
            };
            r.exit_code = f.exit_code;
            r.summary = Some(f.summary);
            r.head_sha = f.head_sha;
            r.pushed = f.pushed;
            r.finished_ms = Some(now_ms());
        });
        run.transcript.close();
        self.spawn_callback(run.dir.clone(), run.id.clone());
    }

    fn spawn_callback(self: &Arc<Self>, dir: PathBuf, run_id: String) {
        let this = self.clone();
        tokio::spawn(async move { this.deliver_callback(dir, run_id).await });
    }

    async fn deliver_callback(&self, dir: PathBuf, run_id: String) {
        let path = dir.join("callback.json");
        let Some(p) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<PendingCallback>(&t).ok())
        else {
            return;
        };
        let mut delay = self.cfg.callback_backoff;
        loop {
            let res = self
                .http
                .post(&p.url)
                .bearer_auth(&p.token)
                .json(&p.body)
                .send()
                .await;
            match res {
                Ok(r) if r.status().is_success() => {
                    let _ = std::fs::remove_file(&path);
                    tracing::info!(run = %run_id, "run-finished callback delivered");
                    return;
                }
                Ok(r) => tracing::warn!(run = %run_id, status = %r.status(), "callback rejected"),
                Err(e) => tracing::warn!(run = %run_id, error = %e, "callback failed"),
            }
            if now_ms().saturating_sub(p.first_attempt_ms) > CALLBACK_GIVE_UP.as_millis() as u64 {
                tracing::error!(run = %run_id, "giving up on callback");
                return;
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_CALLBACK_BACKOFF);
        }
    }

    fn repo_lock(&self, dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.repo_locks
            .lock()
            .unwrap()
            .entry(dir.to_path_buf())
            .or_default()
            .clone()
    }

    fn git_env(&self) -> Vec<(String, String)> {
        let c = &self.cfg;
        vec![
            ("GIT_TERMINAL_PROMPT".into(), "0".into()),
            ("GIT_AUTHOR_NAME".into(), c.git_name.clone()),
            ("GIT_AUTHOR_EMAIL".into(), c.git_email.clone()),
            ("GIT_COMMITTER_NAME".into(), c.git_name.clone()),
            ("GIT_COMMITTER_EMAIL".into(), c.git_email.clone()),
        ]
    }

    /// Run git; a non-zero exit is returned as output, not an error.
    async fn git_raw(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        extra_env: &[(String, String)],
        cwd: Option<&Path>,
        args: &[&str],
    ) -> Result<Output, Abort> {
        let mut spec = CmdSpec::new(&self.cfg.git_bin, args);
        spec.cwd = cwd.map(Path::to_path_buf);
        spec.env = self.git_env();
        spec.env.extend(extra_env.iter().cloned());
        let sink = line_sink(run, "git", secrets);
        let out = exec::run(&self.reaper, spec, ctl, Some(sink))
            .await
            .map_err(|e| Abort::Fail(format!("cannot run git: {e}")))?;
        match out.stop {
            Some(s) => Err(s.into()),
            None => Ok(out),
        }
    }

    /// Run git and require success.
    async fn git(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        extra_env: &[(String, String)],
        cwd: Option<&Path>,
        args: &[&str],
    ) -> Result<Output, Abort> {
        let out = self
            .git_raw(run, ctl, secrets, extra_env, cwd, args)
            .await?;
        if out.success() {
            Ok(out)
        } else {
            Err(Abort::Fail(redact(
                &format!(
                    "git {} failed ({:?}): {}",
                    args.first().unwrap_or(&""),
                    out.code(),
                    out.stderr.trim()
                ),
                secrets,
            )))
        }
    }

    async fn rev_parse(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        cwd: &Path,
        spec: &str,
    ) -> Result<Option<String>, Abort> {
        let out = self
            .git_raw(
                run,
                ctl,
                &[],
                &[],
                Some(cwd),
                &["rev-parse", "--verify", "--quiet", spec],
            )
            .await?;
        Ok(out
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()))
    }

    async fn execute(&self, run: &Run, req: &RunStart, ctl: &mut Ctl) -> Result<Finish, Abort> {
        let ws = &self.cfg.workspace;
        let mut secrets = vec![req.clone_token.clone(), req.callback_token.clone()];
        let mut auth_env = Vec::new();
        if !req.clone_token.is_empty() && !req.repo_url.starts_with("file://") {
            let b64 = base64::engine::general_purpose::STANDARD
                .encode(format!("x-access-token:{}", req.clone_token));
            secrets.push(b64.clone());
            auth_env = vec![
                ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
                (
                    "GIT_CONFIG_KEY_0".to_string(),
                    "http.extraheader".to_string(),
                ),
                (
                    "GIT_CONFIG_VALUE_0".to_string(),
                    format!("Authorization: basic {b64}"),
                ),
            ];
        }

        // 1. clone / fetch, then the worktree.
        let repo = ws.join("repos").join(repo_dir_name(&req.repo_url));
        let worktree = ws.join("worktrees").join(format!("run-{}", req.run_id));
        let lock = self.repo_lock(&repo);
        let (start_sha, base_sha) = {
            let _g = lock.lock().await;
            let _ = std::fs::create_dir_all(ws.join("repos"));
            let _ = std::fs::create_dir_all(ws.join("worktrees"));
            if repo.join(".git").exists() {
                self.git(
                    run,
                    ctl,
                    &secrets,
                    &auth_env,
                    Some(&repo),
                    &["fetch", "--prune", "origin"],
                )
                .await?;
            } else {
                let _ = std::fs::remove_dir_all(&repo);
                let r = repo.to_string_lossy().into_owned();
                let res = self
                    .git(
                        run,
                        ctl,
                        &secrets,
                        &auth_env,
                        None,
                        &["clone", "--no-checkout", "--", &req.repo_url, &r],
                    )
                    .await;
                if let Err(e) = res {
                    let _ = std::fs::remove_dir_all(&repo);
                    return Err(e);
                }
            }
            let base = self.resolve_base(run, ctl, &repo, &req.base_ref).await?;
            let start = match self
                .rev_parse(
                    run,
                    ctl,
                    &repo,
                    &format!("refs/remotes/origin/{}^{{commit}}", req.branch),
                )
                .await?
            {
                Some(s) => s,
                None => base.clone(),
            };
            if worktree.exists() {
                let _ = std::fs::remove_dir_all(&worktree);
            }
            self.git(run, ctl, &secrets, &[], Some(&repo), &["worktree", "prune"])
                .await?;
            let w = worktree.to_string_lossy().into_owned();
            self.git(
                run,
                ctl,
                &secrets,
                &[],
                Some(&repo),
                &["worktree", "add", "-f", "-B", &req.branch, &w, &start],
            )
            .await?;
            (start, base)
        };
        run.transcript.append(
            "worktree",
            json!({"path": worktree, "branch": req.branch, "start": start_sha, "base": base_sha}),
        );

        // 2. claude
        let token = std::fs::read_to_string(&self.cfg.claude_token_file)
            .map(|t| t.trim().to_string())
            .unwrap_or_default();
        if token.is_empty() {
            return Err(Abort::Fail(format!(
                "claude is not logged in: no token in {}",
                self.cfg.claude_token_file.display()
            )));
        }
        secrets.push(token.clone());
        let mut args: Vec<&str> = vec!["-p"];
        args.extend(self.cfg.claude_args.iter().map(String::as_str));
        let mut spec = CmdSpec::new(&self.cfg.claude_bin, &args);
        spec.cwd = Some(worktree.clone());
        spec.env = self.git_env();
        spec.env.extend([
            ("CLAUDE_CODE_OAUTH_TOKEN".to_string(), token),
            ("ENZARB_RUN_ID".to_string(), req.run_id.clone()),
            ("ENZARB_BRANCH".to_string(), req.branch.clone()),
        ]);
        spec.stdin = Some(req.prompt.clone().into_bytes());
        let sink = line_sink(run, "claude", &secrets);
        let out = exec::run(&self.reaper, spec, ctl, Some(sink))
            .await
            .map_err(|e| Abort::Fail(format!("cannot run claude: {e}")))?;
        if let Some(s) = out.stop {
            return Err(s.into());
        }
        let parsed = parse_claude_output(&out.stdout);
        run.transcript.append(
            "claude_result",
            json!({
                "exit_code": out.code(),
                "is_error": parsed.as_ref().map(|p| p.is_error),
                "result": parsed.as_ref().map(|p| redact(&p.text, &secrets)),
            }),
        );
        let exit_code = out.code();
        let head_sha = self
            .rev_parse(run, ctl, &worktree, "HEAD")
            .await?
            .unwrap_or_default();
        let fail = |summary: String, head_sha: String| Finish {
            outcome: RunOutcome::Failed,
            exit_code,
            summary: redact(&summary, &secrets),
            head_sha,
            pushed: false,
        };
        let result = match parsed {
            Some(p) if out.success() && !p.is_error => p.text,
            Some(p) => return Ok(fail(p.text, head_sha)),
            None if out.success() => {
                return Ok(fail("claude produced no parseable result".into(), head_sha));
            }
            None => {
                return Ok(fail(
                    format!("claude exited with {:?}: {}", exit_code, out.stderr.trim()),
                    head_sha,
                ));
            }
        };

        // 3. commit leftovers and push
        let mut head_sha = head_sha;
        let mut pushed = false;
        if req.push {
            let status = self
                .git(
                    run,
                    ctl,
                    &secrets,
                    &[],
                    Some(&worktree),
                    &["status", "--porcelain"],
                )
                .await?;
            if !status.stdout.iter().all(u8::is_ascii_whitespace) {
                let msg = format!("enzarb: uncommitted changes from run {}", req.run_id);
                self.git(run, ctl, &secrets, &[], Some(&worktree), &["add", "-A"])
                    .await?;
                self.git(
                    run,
                    ctl,
                    &secrets,
                    &[],
                    Some(&worktree),
                    &["-c", "commit.gpgsign=false", "commit", "-m", &msg],
                )
                .await?;
                head_sha = self
                    .rev_parse(run, ctl, &worktree, "HEAD")
                    .await?
                    .unwrap_or_default();
            }
            // Nothing new on a fresh branch: do not create it.
            let nothing = req.branch != req.base_ref && head_sha == base_sha;
            if !nothing {
                let refspec = format!("HEAD:refs/heads/{}", req.branch);
                let out = self
                    .git_raw(
                        run,
                        ctl,
                        &secrets,
                        &auth_env,
                        Some(&worktree),
                        &["push", "origin", &refspec],
                    )
                    .await?;
                if !out.success() {
                    return Ok(fail(
                        format!("git push failed ({:?}): {}", out.code(), out.stderr.trim()),
                        head_sha,
                    ));
                }
                pushed = true;
            }
        }
        Ok(Finish {
            outcome: RunOutcome::Succeeded,
            exit_code,
            summary: result,
            head_sha,
            pushed,
        })
    }

    /// Commit of `base_ref`: `origin/<ref>`, else a tag or sha.
    async fn resolve_base(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        repo: &Path,
        base_ref: &str,
    ) -> Result<String, Abort> {
        for spec in [
            format!("refs/remotes/origin/{base_ref}^{{commit}}"),
            format!("{base_ref}^{{commit}}"),
        ] {
            if let Some(s) = self.rev_parse(run, ctl, repo, &spec).await? {
                return Ok(s);
            }
        }
        Err(Abort::Fail(format!("base_ref {base_ref:?} not found")))
    }
}

fn line_sink(run: &Run, source: &'static str, secrets: &[String]) -> LineSink {
    let transcript = run.transcript.clone();
    let secrets = secrets.to_vec();
    Arc::new(move |line: &str| {
        transcript.append(
            "log",
            json!({"source": source, "line": redact(line, &secrets)}),
        );
    })
}
