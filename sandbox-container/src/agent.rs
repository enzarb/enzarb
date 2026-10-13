//! The real [`RunManager`]: idempotent agent runs in git worktrees
//! (contracts §3, §4, §12).
//!
//! Lifecycle of a run (`run_start`):
//! 1. `queued`: persisted under `<workspace>/.enzarb/runs/<run_id>/`, waiting for a slot;
//! 2. `running`: ensure the clone, create worktree `run-<run_id>` on `branch`,
//!    run the agent in it over ACP (`acp`), optionally commit leftovers and push `branch`;
//! 3. terminal (`succeeded | failed | cancelled`): `state.json` is final and
//!    the [`RunFinished`] callback is delivered with retries
//!    (`callback.json` exists until a 2xx or until the callback is abandoned:
//!    a permanent 4xx, or 24 h of failures; `state.json` then says `abandoned`).
//!
//! On-disk layout:
//! * `<ws>/repos/<name>-<hash>/`: clones (one per repo URL);
//! * `<ws>/worktrees/run-<run_id>/`: the run's worktree. It is removed when the
//!   run ends (`target/` dirs are GBs); at startup and before each run any
//!   `run-*` dir that belongs to no active run is pruned too;
//! * `<ws>/.enzarb/runs/<run_id>/state.json`: current record;
//! * `.../request.json` (0600): the request without `clone_token` (needed to
//!   finish a run after a restart);
//! * `.../transcript.jsonl`: one JSON object per line (state changes, git and
//!   agent stderr lines, ACP updates, permission decisions and the agent result);
//! * `.../callback.json` (0600): the pending callback, removed once delivered or abandoned.

use std::collections::{HashMap, HashSet};
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

use crate::acp::{self, AcpError, AcpSpec};
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
/// Before a run, only prune stale worktrees untouched for this long.
const PRUNE_MIN_AGE: Duration = Duration::from_secs(60);
const ENOSPC_HINT: &str =
    " (the workspace volume is full: free space or grow the Project's sandbox storage)";

/// Delete `run-*` directories under `root` that do not belong to an active
/// run and were last modified at least `min_age` ago. Only real directories
/// directly inside `root` are touched (never symlinks or other files).
/// `active` is read after the directory listing, so a run that creates its
/// worktree concurrently is always seen as active. Returns the count removed.
fn prune_worktrees(root: &Path, min_age: Duration, active: impl Fn() -> HashSet<String>) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let candidates: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_string();
            if !name.starts_with("run-") {
                return None;
            }
            let md = std::fs::symlink_metadata(e.path()).ok()?;
            if !md.is_dir() {
                return None;
            }
            let age = md.modified().ok().and_then(|m| m.elapsed().ok());
            (age.unwrap_or_default() >= min_age).then(|| (name, e.path()))
        })
        .collect();
    if candidates.is_empty() {
        return 0;
    }
    let active = active();
    let mut n = 0;
    for (name, path) in candidates {
        if active.contains(&name) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => n += 1,
            Err(e) => tracing::warn!(dir = %path.display(), error = %e, "cannot prune worktree"),
        }
    }
    n
}

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
    /// Set when delivery of the callback was given up on (never retried again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    callback: Option<CallbackState>,
}

/// Terminal outcome of callback delivery, when it is not a success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CallbackState {
    Abandoned,
}

/// A rejected callback that can never succeed on retry: 4xx except 408/429.
fn callback_status_is_permanent(s: reqwest::StatusCode) -> bool {
    s.is_client_error() && s != reqwest::StatusCode::REQUEST_TIMEOUT && s.as_u16() != 429
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
    repo_locks: RepoLocks,
    http: reqwest::Client,
}

/// Per-clone async locks: every git operation that changes a clone's
/// worktrees or refs (runs and chats) holds the clone's lock.
#[derive(Clone, Default)]
pub struct RepoLocks(Arc<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>);

impl RepoLocks {
    /// The lock of the clone at `dir`.
    pub fn get(&self, dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.0
            .lock()
            .unwrap()
            .entry(dir.to_path_buf())
            .or_default()
            .clone()
    }
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
                repo_locks: RepoLocks::default(),
                http,
            }),
        }
    }

    /// The clone locks, to share with the chat manager.
    pub fn repo_locks(&self) -> RepoLocks {
        self.inner.repo_locks.clone()
    }

    /// Load runs from disk: runs that were interrupted by a restart become
    /// `failed`, and pending callbacks are delivered. Call once at startup.
    pub fn recover(&self) {
        let inner = &self.inner;
        // Runs before this start that were interrupted are failed below, and
        // the rest are over: reclaim their worktrees (concurrent new runs are
        // protected by the active-run check).
        {
            let inner = inner.clone();
            tokio::spawn(async move { inner.sweep_worktrees(Duration::ZERO).await });
        }
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
            } else if run.rec.lock().unwrap().callback != Some(CallbackState::Abandoned)
                && dir.join("callback.json").exists()
            {
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
            callback: None,
        };
        let mut stored = req.clone();
        stored.clone_token = String::new();
        stored.deps_token = None;
        stored.npm_token = None;
        stored.claude_token = None;
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

fn basic_b64(token: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"))
}

/// Environment for the agent process only so cargo can fetch private git
/// dependencies from the Project's other repos. The token lives in the process
/// env and is read by an in-memory credential helper scoped to github.com;
/// nothing is written to disk. Push keeps using the run's own clone token,
/// which sandbox-init applies to its own git calls, not the agent's.
fn deps_env(token: &str) -> Vec<(String, String)> {
    let kv = |k: &str, v: &str| (k.to_string(), v.to_string());
    vec![
        kv("CARGO_NET_GIT_FETCH_WITH_CLI", "true"),
        kv("ENZARB_DEPS_TOKEN", token),
        // Fallback for npm.pkg.github.com when no `npm_token` was supplied;
        // `npm_env` replaces it. Same value as the deps token, so it is
        // already in the redaction set.
        kv("NODE_AUTH_TOKEN", token),
        kv("GIT_CONFIG_COUNT", "3"),
        kv("GIT_CONFIG_KEY_0", "url.https://github.com/.insteadOf"),
        kv("GIT_CONFIG_VALUE_0", "ssh://git@github.com/"),
        // An empty helper resets any inherited list; then ours.
        kv("GIT_CONFIG_KEY_1", "credential.https://github.com.helper"),
        kv("GIT_CONFIG_VALUE_1", ""),
        kv("GIT_CONFIG_KEY_2", "credential.https://github.com.helper"),
        kv(
            "GIT_CONFIG_VALUE_2",
            "!f() { echo username=x-access-token; echo \"password=$ENZARB_DEPS_TOKEN\"; }; f",
        ),
    ]
}

/// `NODE_AUTH_TOKEN` for the agent process only (read by enzarb-ui/.npmrc for
/// npm.pkg.github.com). The token is a long-lived PAT with `read:packages`:
/// env only, no .npmrc is written, never stored in request.json/state.
fn npm_env(token: &str) -> Vec<(String, String)> {
    vec![("NODE_AUTH_TOKEN".to_string(), token.to_string())]
}

/// Apply `npm_token` over whatever `deps_env` put in `NODE_AUTH_TOKEN`.
fn apply_npm_env(env: &mut Vec<(String, String)>, token: &str) {
    env.retain(|(k, _)| k != "NODE_AUTH_TOKEN");
    env.extend(npm_env(token));
}

fn redact(s: &str, secrets: &[String]) -> String {
    let mut out = s.to_string();
    for sec in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(sec.as_str(), "***");
    }
    out
}

/// Redact `secrets` in every string of a JSON value.
fn redact_value(v: Value, secrets: &[String]) -> Value {
    match v {
        Value::String(s) => Value::String(redact(&s, secrets)),
        Value::Array(a) => Value::Array(a.into_iter().map(|x| redact_value(x, secrets)).collect()),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, x)| (k, redact_value(x, secrets)))
                .collect(),
        ),
        other => other,
    }
}

/// Directory name of the clone of `url` under `<workspace>/repos/`.
pub fn repo_dir_name(url: &str) -> String {
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
        // Make room first: stale worktrees of finished or crashed runs.
        self.sweep_worktrees(PRUNE_MIN_AGE).await;
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
            Err(Abort::Fail(m)) => Finish::failed(with_enospc_hint(m)),
        };
        // The run is over: its worktree (and target/) is no longer needed.
        // Logs and transcripts live under .enzarb/runs and are kept.
        self.remove_worktree(&req).await;
        self.finalize(&run, finish);
    }

    fn worktrees_dir(&self) -> PathBuf {
        self.cfg.workspace.join("worktrees")
    }

    /// Names (`run-<id>`) of the worktrees of runs that are not finished.
    fn active_worktrees(&self) -> HashSet<String> {
        self.runs
            .lock()
            .unwrap()
            .values()
            .filter(|r| !r.state().is_terminal())
            .map(|r| format!("run-{}", r.id))
            .collect()
    }

    /// Prune worktrees of no active run (see [`prune_worktrees`]).
    async fn sweep_worktrees(self: &Arc<Self>, min_age: Duration) {
        let this = self.clone();
        let res = tokio::task::spawn_blocking(move || {
            prune_worktrees(&this.worktrees_dir(), min_age, || this.active_worktrees())
        })
        .await;
        if let Ok(n) = res
            && n > 0
        {
            tracing::info!(removed = n, "pruned stale run worktrees");
        }
    }

    /// Remove the run's worktree and the git metadata that points at it.
    async fn remove_worktree(&self, req: &RunStart) {
        let ws = &self.cfg.workspace;
        let repo = ws.join("repos").join(repo_dir_name(&req.repo_url));
        let worktree = ws.join("worktrees").join(format!("run-{}", req.run_id));
        let lock = self.repo_lock(&repo);
        let _g = lock.lock().await;
        let _ = tokio::task::spawn_blocking(move || {
            if std::fs::symlink_metadata(&worktree).is_ok_and(|m| m.is_dir()) {
                let _ = std::fs::remove_dir_all(&worktree);
            }
        })
        .await;
        if repo.join(".git").exists() {
            // drop git's metadata for the removed worktree (through the
            // reaper, like every other child)
            let mut spec = CmdSpec::new(&self.cfg.git_bin, &["worktree", "prune"]);
            spec.cwd = Some(repo);
            spec.env = self.git_env();
            let (_keep, cancel) = watch::channel(false);
            let mut ctl = Ctl {
                cancel,
                deadline: Instant::now() + Duration::from_secs(30),
            };
            let _ = exec::run(&self.reaper, spec, &mut ctl, None).await;
        }
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
                Ok(r) if callback_status_is_permanent(r.status()) => {
                    self.abandon_callback(&dir, &run_id, &format!("rejected with {}", r.status()));
                    return;
                }
                Ok(r) => tracing::warn!(run = %run_id, status = %r.status(), "callback rejected"),
                Err(e) => tracing::warn!(run = %run_id, error = %e, "callback failed"),
            }
            if now_ms().saturating_sub(p.first_attempt_ms) > CALLBACK_GIVE_UP.as_millis() as u64 {
                self.abandon_callback(&dir, &run_id, "giving up after 24h of failures");
                return;
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_CALLBACK_BACKOFF);
        }
    }

    /// Stop delivering a run's callback for good: mark it in `state.json`
    /// (the result and transcript stay), drop `callback.json` (it holds the
    /// bearer token) so a restart does not resurrect the retries, and log once.
    fn abandon_callback(&self, dir: &Path, run_id: &str, reason: &str) {
        tracing::warn!(run = %run_id, reason, "abandoning run-finished callback");
        let run = self.runs.lock().unwrap().get(run_id).cloned();
        if let Some(run) = run {
            let mut rec = run.rec.lock().unwrap();
            rec.callback = Some(CallbackState::Abandoned);
            run.persist(&rec);
        }
        let _ = std::fs::remove_file(dir.join("callback.json"));
    }

    fn repo_lock(&self, dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.repo_locks.get(dir)
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
        let mut spec = CmdSpec::new(&self.cfg.git_bin, &crate::netgit::local_args(args));
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

    /// `mise install` in the worktree (tools persist on the workspace volume).
    /// Best effort: a failure is recorded in the transcript and the run goes
    /// on, since the repo may pin nothing and the agent can retry by hand.
    async fn install_toolchain(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        worktree: &Path,
    ) -> Result<(), Abort> {
        if self.cfg.mise_bin.is_empty() {
            return Ok(());
        }
        let mut spec = CmdSpec::new(&self.cfg.mise_bin, &["install"]);
        spec.cwd = Some(worktree.to_path_buf());
        let sink = line_sink(run, "mise", secrets);
        match exec::run(&self.reaper, spec, ctl, Some(sink)).await {
            Ok(out) => {
                if let Some(s) = out.stop {
                    return Err(s.into());
                }
                run.transcript.append(
                    "toolchain",
                    json!({"ok": out.success(), "code": out.code(),
                           "stderr": redact(out.stderr.trim(), secrets)}),
                );
            }
            Err(e) => run
                .transcript
                .append("toolchain", json!({"ok": false, "error": e.to_string()})),
        }
        Ok(())
    }

    /// Remove other worktrees that have `branch` checked out. Fails if the
    /// holder belongs to a run that is still running in this process.
    async fn free_branch(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        repo: &Path,
        branch: &str,
    ) -> Result<(), Abort> {
        let list = self
            .git(
                run,
                ctl,
                secrets,
                &[],
                Some(repo),
                &["worktree", "list", "--porcelain"],
            )
            .await?;
        let want = format!("refs/heads/{branch}");
        let mut holders = Vec::new();
        let mut path: Option<PathBuf> = None;
        for line in String::from_utf8_lossy(&list.stdout).lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if line.strip_prefix("branch ") == Some(want.as_str())
                && let Some(p) = path.clone()
            {
                holders.push(p);
            }
        }
        for p in holders {
            let owner = p
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix("run-"))
                .map(str::to_string);
            let Some(owner) = owner else {
                return Err(Abort::Fail(format!(
                    "branch {branch:?} is checked out at {}, not a run worktree",
                    p.display()
                )));
            };
            if let Some(o) = Some(owner).filter(|o| *o != run.id) {
                let active = self
                    .runs
                    .lock()
                    .unwrap()
                    .get(&o)
                    .is_some_and(|r| !r.state().is_terminal());
                if active {
                    return Err(Abort::Fail(format!(
                        "branch {branch:?} is in use by active run {o:?}"
                    )));
                }
            }
            let ps = p.to_string_lossy().into_owned();
            let out = self
                .git_raw(
                    run,
                    ctl,
                    secrets,
                    &[],
                    Some(repo),
                    &["worktree", "remove", "--force", &ps],
                )
                .await?;
            if !out.success() {
                let _ = std::fs::remove_dir_all(&p);
                self.git(run, ctl, secrets, &[], Some(repo), &["worktree", "prune"])
                    .await?;
            }
        }
        Ok(())
    }

    /// An authenticated git call: `net_env` only (see [`crate::netgit`]).
    async fn net_git(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        net_env: &[(String, String)],
        cwd: &Path,
        args: &[&str],
    ) -> Result<Output, Abort> {
        let mut spec = CmdSpec::new(&self.cfg.git_bin, args);
        spec.cwd = Some(cwd.to_path_buf());
        spec.env = net_env.to_vec();
        spec.env_clear = true;
        let sink = line_sink(run, "git", secrets);
        let out = exec::run(&self.reaper, spec, ctl, Some(sink))
            .await
            .map_err(|e| Abort::Fail(format!("cannot run git: {e}")))?;
        match out.stop {
            Some(s) => Err(s.into()),
            None => Ok(out),
        }
    }

    /// A fresh, empty bare repo for one network call, borrowing `repo`'s
    /// objects. Removed when the guard drops.
    async fn net_repo(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        net_env: &[(String, String)],
        tag: &str,
        repo: &Path,
    ) -> Result<crate::netgit::NetDir, Abort> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = crate::netgit::net_dir(&self.cfg.workspace, tag)
            .map_err(|e| Abort::Fail(format!("cannot name the fetch repo: {e}")))?;
        let parent = dir.parent().unwrap_or(&self.cfg.workspace).to_path_buf();
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&parent);
        let _ = std::fs::remove_dir_all(&dir);
        let guard = crate::netgit::NetDir(dir.clone());
        let d = dir.to_string_lossy().into_owned();
        let out = self
            .net_git(
                run,
                ctl,
                secrets,
                net_env,
                &parent,
                &["init", "-q", "--bare", "--template=", &d],
            )
            .await?;
        require(out, "init", secrets)?;
        let objects = repo.join(".git").join("objects");
        std::fs::create_dir_all(dir.join("objects/info"))
            .and_then(|_| {
                std::fs::write(
                    dir.join("objects/info/alternates"),
                    format!("{}\n", objects.display()),
                )
            })
            .map_err(|e| Abort::Fail(format!("cannot prepare the fetch repo: {e}")))?;
        Ok(guard)
    }

    /// Fail if the scratch repo's config is no longer what `git init` wrote
    /// (another process raced into it). Checked before and after the call.
    fn net_untouched(net: &crate::netgit::NetDir, snap: &[u8]) -> Result<(), Abort> {
        match std::fs::read(net.0.join("config")) {
            Ok(c) if c == snap => Ok(()),
            _ => Err(Abort::Fail(
                "the scratch git repo was modified during the operation".into(),
            )),
        }
    }

    /// Fetch `url` into `repo` without running anything of `repo`'s config
    /// with the token: network into a scratch repo, then a local fetch.
    #[allow(clippy::too_many_arguments)]
    async fn net_fetch(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        net_env: &[(String, String)],
        run_id: &str,
        repo: &Path,
        url: &str,
    ) -> Result<(), Abort> {
        let net = self
            .net_repo(run, ctl, secrets, net_env, &format!("fetch-{run_id}"), repo)
            .await?;
        let snap = std::fs::read(net.0.join("config")).unwrap_or_default();
        Self::net_untouched(&net, &snap)?;
        let out = self
            .net_git(
                run,
                ctl,
                secrets,
                net_env,
                &net.0,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--",
                    url,
                    "+refs/heads/*:refs/remotes/origin/*",
                    "+refs/tags/*:refs/tags/*",
                ],
            )
            .await?;
        Self::net_untouched(&net, &snap)?;
        require(out, "fetch", secrets)?;
        // Local and token-free: the clone's own config may apply here.
        let n = net.0.to_string_lossy().into_owned();
        self.git(
            run,
            ctl,
            secrets,
            &[],
            Some(repo),
            &[
                "fetch",
                "--prune",
                "--no-tags",
                "--",
                &n,
                "+refs/remotes/origin/*:refs/remotes/origin/*",
                "+refs/tags/*:refs/tags/*",
            ],
        )
        .await?;
        Ok(())
    }

    /// Push `refspec` (`<sha>:refs/heads/<branch>`) to `url` from a scratch
    /// repo that borrows `repo`'s objects. A non-zero exit is returned.
    #[allow(clippy::too_many_arguments)]
    async fn net_push(
        &self,
        run: &Run,
        ctl: &mut Ctl,
        secrets: &[String],
        net_env: &[(String, String)],
        run_id: &str,
        repo: &Path,
        url: &str,
        refspec: &str,
    ) -> Result<Output, Abort> {
        let net = self
            .net_repo(run, ctl, secrets, net_env, &format!("push-{run_id}"), repo)
            .await?;
        let snap = std::fs::read(net.0.join("config")).unwrap_or_default();
        Self::net_untouched(&net, &snap)?;
        let out = self
            .net_git(
                run,
                ctl,
                secrets,
                net_env,
                &net.0,
                &["push", "--no-verify", "--", url, refspec],
            )
            .await?;
        Self::net_untouched(&net, &snap)?;
        Ok(out)
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
        let deps_token = req.deps_token.clone().filter(|t| !t.is_empty());
        if let Some(t) = &deps_token {
            secrets.push(t.clone());
            secrets.push(basic_b64(t));
        }
        let npm_token = req.npm_token.clone().filter(|t| !t.is_empty());
        if let Some(t) = &npm_token {
            secrets.push(t.clone());
            secrets.push(basic_b64(t));
            // npm basic auth uses the bare `_auth` form, `user:token` base64.
            secrets.push(
                base64::engine::general_purpose::STANDARD.encode(format!("{t}:x-oauth-basic")),
            );
        }
        // The auth header lives only in the hardened env of the network calls
        // (crate::netgit), never in a config file or a clone's own config.
        let header = (!req.clone_token.is_empty()).then(|| {
            let b64 = base64::engine::general_purpose::STANDARD
                .encode(format!("x-access-token:{}", req.clone_token));
            secrets.push(b64.clone());
            format!("Authorization: basic {b64}")
        });
        let net_env = crate::netgit::env(&req.repo_url, header.as_deref(), &self.git_env())
            .map_err(Abort::Fail)?;

        // 1. clone / fetch, then the worktree.
        let repo = ws.join("repos").join(repo_dir_name(&req.repo_url));
        let worktree = ws.join("worktrees").join(format!("run-{}", req.run_id));
        let lock = self.repo_lock(&repo);
        let (start_sha, base_sha) = {
            let _g = lock.lock().await;
            let _ = std::fs::create_dir_all(ws.join("repos"));
            let _ = std::fs::create_dir_all(ws.join("worktrees"));
            if repo.join(".git").exists() {
                self.net_fetch(
                    run,
                    ctl,
                    &secrets,
                    &net_env,
                    &req.run_id,
                    &repo,
                    &req.repo_url,
                )
                .await?;
            } else {
                let _ = std::fs::remove_dir_all(&repo);
                // Into a fresh, unpredictably named directory (nothing planted
                // to read), then moved into place.
                let tmp = crate::netgit::net_dir(ws, &format!("clone-{}", req.run_id))
                    .map_err(|e| Abort::Fail(format!("cannot name the clone dir: {e}")))?;
                let _ = std::fs::create_dir_all(tmp.parent().unwrap_or(ws));
                let tmp = crate::netgit::NetDir(tmp);
                let t = tmp.0.to_string_lossy().into_owned();
                let res = self
                    .net_git(
                        run,
                        ctl,
                        &secrets,
                        &net_env,
                        &ws.join("repos"),
                        &[
                            "clone",
                            "--no-checkout",
                            "--template=",
                            "--",
                            &req.repo_url,
                            &t,
                        ],
                    )
                    .await
                    .and_then(|o| require(o, "clone", &secrets))
                    .and_then(|o| {
                        std::fs::rename(&tmp.0, &repo)
                            .map(|_| o)
                            .map_err(|e| Abort::Fail(format!("cannot move the clone: {e}")))
                    });
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
            // A finished job's worktree may still hold `branch` (retries reuse
            // branch names), and git refuses `-B` on a checked-out branch.
            self.free_branch(run, ctl, &secrets, &repo, &req.branch)
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

        // 2. the agent, over ACP
        // Per-run token from `run_start`; the legacy file only when it is absent.
        let Some(token) = crate::capture::resolve_token(req.claude_token.as_ref(), || {
            crate::capture::read_legacy(&self.cfg.claude_token_file)
        }) else {
            return Err(Abort::Fail(
                "claude is not logged in: no_credential (no claude_token and no legacy token)"
                    .into(),
            ));
        };
        secrets.push(token.clone());
        // and its base64 encodings
        secrets.extend(crate::chat::filter::token_forms(&token));
        // The repo's pinned toolchain, so the agent can run its checks. After
        // the token joins `secrets` so any output is redacted.
        self.install_toolchain(run, ctl, &secrets, &worktree)
            .await?;
        let mut env = self.git_env();
        env.extend([
            ("CLAUDE_CODE_OAUTH_TOKEN".to_string(), token),
            ("ENZARB_RUN_ID".to_string(), req.run_id.clone()),
            ("ENZARB_BRANCH".to_string(), req.branch.clone()),
        ]);
        if let Some(t) = &deps_token {
            env.extend(deps_env(t));
        }
        if let Some(t) = &npm_token {
            apply_npm_env(&mut env, t);
        }
        let spec = AcpSpec {
            bin: self.cfg.acp_bin.clone(),
            cwd: worktree.clone(),
            env,
            prompt: req.prompt.clone(),
            cancel_grace: self.cfg.cancel_grace,
        };
        let emit: acp::Emit = {
            let transcript = run.transcript.clone();
            let secrets = secrets.clone();
            Arc::new(move |kind: &str, fields: Value| {
                transcript.append(kind, redact_value(fields, &secrets));
            })
        };
        let policy = self.cfg.policy.clone();
        let sink = line_sink(run, "acp", &secrets);
        let out = match acp::run(&self.reaper, spec, policy, ctl, emit, Some(sink)).await {
            Ok(o) => o,
            Err(AcpError::Stopped(s)) => return Err(s.into()),
            Err(AcpError::Failed(m)) => return Err(Abort::Fail(redact(&m, &secrets))),
        };
        run.transcript.append(
            "agent_result",
            json!({"stop_reason": out.stop_reason, "text": redact(&out.text, &secrets)}),
        );
        let exit_code = Some(0);
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
        let result = match out.stop_reason.as_str() {
            "end_turn" if !out.text.trim().is_empty() => out.text,
            "end_turn" => return Ok(fail("the agent produced no message".into(), head_sha)),
            other => {
                return Ok(fail(
                    format!("the agent stopped with {other}: {}", out.text.trim()),
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
                let refspec = format!("{head_sha}:refs/heads/{}", req.branch);
                let out = self
                    .net_push(
                        run,
                        ctl,
                        &secrets,
                        &net_env,
                        &req.run_id,
                        &repo,
                        &req.repo_url,
                        &refspec,
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
            summary: redact(&result, &secrets),
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

/// A failed git call as a run failure.
fn require(out: Output, what: &str, secrets: &[String]) -> Result<Output, Abort> {
    if out.success() {
        Ok(out)
    } else {
        Err(Abort::Fail(redact(
            &format!(
                "git {what} failed ({:?}): {}",
                out.code(),
                out.stderr.trim()
            ),
            secrets,
        )))
    }
}

fn with_enospc_hint(m: String) -> String {
    if m.contains("No space left on device") && !m.contains(ENOSPC_HINT) {
        format!("{m}{ENOSPC_HINT}")
    } else {
        m
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

#[cfg(test)]
mod deps_tests {
    use super::*;

    #[test]
    fn deps_env_is_scoped_and_redactable() {
        let env = deps_env("ghs_secret123");
        let get = |k: &str| env.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str());
        assert_eq!(get("CARGO_NET_GIT_FETCH_WITH_CLI"), Some("true"));
        assert_eq!(get("GIT_CONFIG_COUNT"), Some("3"));
        assert_eq!(get("NODE_AUTH_TOKEN"), Some("ghs_secret123"));
        assert_eq!(get("NODE_AUTH_TOKEN"), get("ENZARB_DEPS_TOKEN"));
        assert_eq!(get("GIT_CONFIG_VALUE_0"), Some("ssh://git@github.com/"));
        // The token is only in its own env var, never in git config values.
        for (k, v) in &env {
            if k.starts_with("GIT_CONFIG_") {
                assert!(!v.contains("ghs_secret123"), "{k}");
            }
        }
        let secrets = vec!["ghs_secret123".to_string(), basic_b64("ghs_secret123")];
        let out = redact("a ghs_secret123 b", &secrets);
        assert_eq!(out, "a *** b");
        assert!(!redact(&basic_b64("ghs_secret123"), &secrets).contains("ghs_"));
    }
}

#[cfg(test)]
mod callback_tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn permanent_statuses() {
        for c in [400, 401, 403, 404, 410, 422] {
            let s = StatusCode::from_u16(c).unwrap();
            assert!(callback_status_is_permanent(s), "{c}");
        }
        for c in [300, 302, 408, 429, 500, 502, 503, 504] {
            let s = StatusCode::from_u16(c).unwrap();
            assert!(!callback_status_is_permanent(s), "{c}");
        }
    }

    #[test]
    fn old_state_json_has_no_callback_marker() {
        let mut rec: RunRecord = serde_json::from_value(json!({
            "run_id": "r", "state": "succeeded", "repo_url": "u", "base_ref": "main",
            "branch": "b", "push": false, "created_ms": 1
        }))
        .unwrap();
        assert_eq!(rec.callback, None);
        assert!(!serde_json::to_string(&rec).unwrap().contains("callback"));
        rec.callback = Some(CallbackState::Abandoned);
        let text = serde_json::to_string(&rec).unwrap();
        assert!(text.contains(r#""callback":"abandoned""#), "{text}");
    }
}

#[cfg(test)]
mod npm_tests {
    use super::*;

    #[test]
    fn npm_token_wins_node_auth_token_and_is_redactable() {
        let mut env = deps_env("ghs_deps");
        apply_npm_env(&mut env, "ghp_npm123");
        let all = |k: &str| {
            env.iter()
                .filter(|(a, _)| a == k)
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(all("NODE_AUTH_TOKEN"), vec!["ghp_npm123"]);
        // cargo/git keep the deps token.
        assert_eq!(all("ENZARB_DEPS_TOKEN"), vec!["ghs_deps"]);
        // The npm token is in no other value (git config etc).
        for (k, v) in &env {
            if k != "NODE_AUTH_TOKEN" {
                assert!(!v.contains("ghp_npm123"), "{k}");
            }
        }
        let secrets = vec!["ghp_npm123".to_string(), basic_b64("ghp_npm123")];
        assert_eq!(redact("a ghp_npm123 b", &secrets), "a *** b");
        assert!(!redact(&basic_b64("ghp_npm123"), &secrets).contains("=="));
    }

    #[test]
    fn deps_token_alone_still_feeds_node_auth_token() {
        let env = deps_env("ghs_deps");
        let v: Vec<_> = env.iter().filter(|(k, _)| k == "NODE_AUTH_TOKEN").collect();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].1, "ghs_deps");
    }

    #[test]
    fn stored_request_never_holds_the_npm_token() {
        let mut req = RunStart {
            run_id: "r".into(),
            repo_url: "u".into(),
            base_ref: "main".into(),
            branch: "b".into(),
            push: false,
            clone_token: "c".into(),
            prompt: "p".into(),
            timeout_secs: 1,
            callback_url: "cb".into(),
            callback_token: "t".into(),
            log_channel: None,
            deps_token: Some("d".into()),
            npm_token: Some("ghp_npm123".into()),
            claude_token: Some("sk-ant-oat01-x".into()),
        };
        req.clone_token = String::new();
        req.deps_token = None;
        req.npm_token = None;
        req.claude_token = None;
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("ghp_npm123") && !json.contains("npm_token"));
        assert!(!json.contains("oat01") && !json.contains("claude_token"));
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    #[test]
    fn prune_skips_active_young_symlinks_files_and_foreign_dirs() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let old = SystemTime::now() - Duration::from_secs(3600);
        let mk = |n: &str| {
            std::fs::create_dir_all(r.join(n).join("target")).unwrap();
            std::fs::File::open(r.join(n))
                .unwrap()
                .set_modified(old)
                .unwrap();
        };
        for n in ["run-dead", "run-live", "other"] {
            mk(n);
        }
        std::fs::create_dir_all(r.join("run-young")).unwrap();
        std::fs::write(r.join("run-f.pid"), "1").unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), r.join("run-link")).unwrap();
        let active = || HashSet::from(["run-live".to_string()]);
        let n = prune_worktrees(r, Duration::from_secs(60), active);
        assert_eq!(n, 1);
        assert!(!r.join("run-dead").exists());
        for keep in ["run-live", "run-young", "other", "run-f.pid", "run-link"] {
            assert!(r.join(keep).exists(), "{keep}");
        }
        assert!(outside.path().exists());
        assert_eq!(
            prune_worktrees(&r.join("missing"), Duration::ZERO, active),
            0
        );
    }

    #[test]
    fn enospc_gets_a_hint_once() {
        let m = with_enospc_hint("git failed: No space left on device".into());
        assert!(m.contains("workspace volume is full"));
        assert_eq!(with_enospc_hint(m.clone()), m);
        assert_eq!(with_enospc_hint("other".into()), "other");
    }
}
