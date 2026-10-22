//! Interactive chat over ACP (ADR-0014, docs/design/chat-tab.md §2–§4).
//!
//! Each `open{kind:"acp"}` gets its own `claude-agent-acp` child, started in
//! its own process group under the reaper, in the chat's own git repository
//! (`<ws>/worktrees/chat-<chat_id>`, branch `enzarb/chat-<chat_id>`). That is
//! a separate `git init` borrowing the job clone's objects (alternates), not a
//! worktree of the clone, so the chat's `.git/config` and hooks are not the
//! ones of the clone jobs fetch and push from (issue #52).
//! sandbox-init relays ACP's NDJSON between the channel and the child's stdio
//! through [`filter::Filter`], which is the security boundary: the browser
//! cannot change the agent's capabilities, cwd, MCP servers, mode beyond
//! `bypassPermissions` (when `ENZARB_CHAT_ALLOW_BYPASS`)/`default`/`plan`, or
//! answer a permission request with anything but
//! `allow_once`/`reject_once`/`cancelled`.
//!
//! Permission mode: every session starts in `ENZARB_CHAT_PERMISSION_MODE`
//! (default `bypassPermissions`: the sandbox is the boundary). In bypass the
//! agent asks for no permission; in the other modes sandbox-init allows every
//! request but leaving plan mode itself. What bounds the agent is its scrubbed
//! environment (no clone, deps or npm token), its worktree, the pod's egress
//! policy and gVisor. It runs as the same uid as sandbox-init, so file modes
//! do not hide sandbox-init's files from it (see the README).
//!
//! The child's environment is built from scratch ([`PASS_ENV`] plus the Claude
//! token and `CLAUDE_CONFIG_DIR`): no clone, deps or npm token exists in it.
//!
//! Limits: `ENZARB_MAX_CHATS` live chats (a new open over the cap closes the
//! oldest with `superseded`), one `acp` channel per socket (`chat_limit`),
//! [`MAX_LINE`] per NDJSON line and the idle and total lifetimes (`limit`,
//! `idle`). On shutdown every chat is closed with `shutdown`. Chats never use
//! run slots and do not hold up drain.
//!
//! On disk:
//! * `<ws>/.enzarb/chats/<chat_id>/meta.json` (0600): the registry entry
//!   (`acp_session_id`, clone, worktree, times); `open.resume` and
//!   `session/load` are checked against it;
//! * `.../transcript.jsonl` (0600): every relayed message, redacted;
//! * `<ws>/.enzarb/chat-home/.claude`: `CLAUDE_CONFIG_DIR` of chat agents, so
//!   Claude's session files survive pod restarts (`session/load`).
//!
//! The chat repo is removed when the chat ends, after its branch (and so any
//! commit) is fetched back into the clone; it is recreated from that branch
//! on resume. Chat repos left by a crash are harvested the same way and
//! removed at startup.

pub mod filter;

use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use enzarb_proto::session::{ChannelHandle, ChannelSender, OpenRequest};
use enzarb_proto::{
    MAX_REPO_LEN, MAX_RESUME_LEN, close_code, close_reason, feature, open_error_code,
};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, mpsc, watch};
use tokio::time::Instant;

use crate::acp::{self, AcpError, Spawned};
use crate::capture::write_atomic;
use crate::config::{ChatAgent, Config};
use crate::docker_prune::ActivityGate;
use crate::exec::{self, CmdSpec, Ctl, LineSink, terminate};
use crate::reaper::{ExitStatus, Reaper};
use crate::repos::{RepoLocks, repo_dir_name};
use crate::steps::SessionSink;
use filter::{Filter, Out};

/// Longest NDJSON line in either direction; longer closes the chat (`limit`).
pub const MAX_LINE: usize = 4 * 1024 * 1024;
/// Queued bytes per direction above which sandbox-init stops reading the
/// other side (backpressure instead of unbounded buffering).
const HIGH_WATER: usize = 8 * 1024 * 1024;
/// Largest transcript; later records are dropped (one marker is written).
const MAX_TRANSCRIPT: u64 = 64 * 1024 * 1024;
/// Environment variables a chat agent inherits from sandbox-init (toolchain
/// and locale only). Everything else is dropped.
pub const PASS_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TERM",
    "MISE_DATA_DIR",
    "MISE_CACHE_DIR",
    "MISE_YES",
    "MISE_TRUSTED_CONFIG_PATHS",
    "MISE_IDIOMATIC_VERSION_FILE_ENABLE_TOOLS",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTC_WRAPPER",
    "SCCACHE_DIR",
    "SCCACHE_CACHE_SIZE",
    "npm_config_cache",
    "PLAYWRIGHT_BROWSERS_PATH",
    "PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD",
    "DOCKER_HOST",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
];

/// Why a chat ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum End {
    /// The browser closed the channel or the socket dropped.
    BrowserClosed,
    /// Closed by sandbox-init with this `CLOSE` reason.
    Reason(&'static str),
    /// The agent exited.
    Exited(Option<ExitStatus>),
}

impl End {
    fn reason(&self) -> String {
        match self {
            End::BrowserClosed => String::new(),
            End::Reason(r) => (*r).to_string(),
            End::Exited(Some(ExitStatus::Exited(c))) => format!("exit:{c}"),
            End::Exited(Some(ExitStatus::Signaled(s))) => format!("signal:{s}"),
            End::Exited(None) => "exit:-1".into(),
        }
    }
}

/// A chat channel that is starting or live.
struct Live {
    conn: u64,
    mux_session: u64,
    chat_id: Option<String>,
    /// Set once the relay ended: the chat is tearing down (cancel grace,
    /// kill, worktree removal) and still counts against the cap.
    closing: Arc<AtomicBool>,
    stop: watch::Sender<Option<&'static str>>,
    done: watch::Receiver<()>,
}

#[derive(Default)]
struct State {
    live: Vec<Live>,
    shutting_down: bool,
}

/// `meta.json` of a chat.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Meta {
    chat_id: String,
    #[serde(default)]
    repo: Option<String>,
    clone: String,
    worktree: PathBuf,
    #[serde(default)]
    acp_session_id: Option<String>,
    created: u64,
    last_active: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// A 16-hex-digit random id.
fn new_chat_id() -> String {
    let mut b = [0u8; 8];
    let ok = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .is_ok();
    if !ok {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let x =
            now_ms() ^ (u64::from(std::process::id()) << 40) ^ SEQ.fetch_add(1, Ordering::Relaxed);
        b = x.to_be_bytes();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn valid_chat_id(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// `open.repo`: a repo URL of at most `MAX_REPO_LEN` printable ASCII bytes
/// with no whitespace (it only selects an existing clone; the directory name
/// is derived and hashed, so it cannot name a path).
pub fn valid_repo(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_REPO_LEN && s.bytes().all(|b| b.is_ascii_graphic())
}

/// `open.resume`: an ACP session id (see [`filter::valid_session_id`]).
pub fn valid_resume(s: &str) -> bool {
    s.len() <= MAX_RESUME_LEN && filter::valid_session_id(s)
}

/// Splits a byte stream into lines of at most `cap` bytes.
pub struct LineBuf {
    buf: Vec<u8>,
    cap: usize,
}

/// A line exceeded the cap.
#[derive(Debug, PartialEq, Eq)]
pub struct LineTooLong;

impl LineBuf {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap,
        }
    }

    /// Append `data`; returns the complete, non-empty lines (without `\n`).
    pub fn push(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, LineTooLong> {
        let mut out = Vec::new();
        let mut rest = data;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            if self.buf.len() + i > self.cap {
                return Err(LineTooLong);
            }
            self.buf.extend_from_slice(&rest[..i]);
            let line = std::mem::take(&mut self.buf);
            if line.iter().any(|b| !b.is_ascii_whitespace()) {
                out.push(line);
            }
            rest = &rest[i + 1..];
        }
        if self.buf.len() + rest.len() > self.cap {
            return Err(LineTooLong);
        }
        self.buf.extend_from_slice(rest);
        Ok(out)
    }
}

/// Kills the agent's process group if the relay is dropped early.
struct PidGuard(u32);

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = killpg(Pid::from_raw(self.0 as i32), Signal::SIGKILL);
    }
}

/// Append-only, size-capped, 0600 transcript. Records are also captured as
/// session events of run `chat-<chat_id>` (ADR-0022) when a sink is set.
struct Transcript {
    file: Mutex<Option<(std::fs::File, u64)>>,
    capture: Option<(Arc<dyn SessionSink>, String)>,
}

/// A transcript record as a session event `(kind, payload)`.
fn session_event(kind: &str, fields: &Value) -> (&'static str, Value) {
    use enzarb_proto::step::session_kind as k;
    match kind {
        "prompt" => (k::USER, fields.clone()),
        "update" => {
            let u = fields.get("update").cloned().unwrap_or(Value::Null);
            let kind = match u.get("sessionUpdate").and_then(Value::as_str) {
                Some("agent_message_chunk") => k::ASSISTANT,
                Some("agent_thought_chunk") => k::THINKING,
                Some("user_message_chunk") => k::USER,
                Some("tool_call") => k::TOOL_CALL,
                Some("tool_call_update") => match u.get("status").and_then(Value::as_str) {
                    Some("completed" | "failed") => k::TOOL_RESULT,
                    _ => k::TOOL_CALL,
                },
                Some("usage_update") => k::USAGE,
                _ => k::SYSTEM,
            };
            (kind, u)
        }
        other => (k::SYSTEM, json!({"record": other, "fields": fields})),
    }
}

impl Transcript {
    fn open(path: &Path) -> Self {
        let fresh = !path.exists();
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| tracing::warn!(path = %path.display(), error = %e, "cannot open chat transcript"))
            .ok()
            .map(|f| {
                if fresh {
                    let _ = crate::privs::own(path);
                }
                let len = f.metadata().map(|m| m.len()).unwrap_or(0);
                (f, len)
            });
        Self {
            file: Mutex::new(f),
            capture: None,
        }
    }

    /// `fields` must already be redacted.
    fn append(&self, kind: &str, fields: Value) {
        if let Some((sink, run_id)) = &self.capture {
            let (k, payload) = session_event(kind, &fields);
            sink.event(run_id, k, payload);
        }
        let mut rec = json!({"ts": now_ms(), "kind": kind});
        match fields {
            Value::Object(m) => {
                for (k, v) in m {
                    if k != "ts" && k != "kind" {
                        rec[k] = v;
                    }
                }
            }
            other => rec["data"] = other,
        }
        let mut g = self.file.lock().unwrap();
        let Some((f, len)) = g.as_mut() else { return };
        if *len >= MAX_TRANSCRIPT {
            return;
        }
        let mut line = rec.to_string();
        line.push('\n');
        if *len + line.len() as u64 >= MAX_TRANSCRIPT {
            line = format!("{}\n", json!({"ts": now_ms(), "kind": "truncated"}));
        }
        if f.write_all(line.as_bytes()).is_ok() {
            *len += line.len() as u64;
        }
    }
}

/// Owns chat channels: admission, supersede, shutdown, worktrees.
pub struct ChatManager {
    cfg: Config,
    reaper: Arc<Reaper>,
    locks: RepoLocks,
    available: bool,
    state: Mutex<State>,
    next: AtomicU64,
    /// Counts live chats for the Docker pruner.
    activity: ActivityGate,
    /// Where chat sessions are captured (the step executor).
    sink: std::sync::OnceLock<Arc<dyn SessionSink>>,
}

/// True when `bin` names an executable file (directly or on `PATH`).
pub(crate) fn find_executable(bin: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let exec = |p: &Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if bin.contains('/') {
        return exec(Path::new(bin));
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| exec(&d.join(bin))))
}

/// The chat agent's argv: `acp_bin` when it is installed, else `mise exec
/// <chat_runtime_tool> -- acp_bin` (mise installs the runtime on first use);
/// ez gets its `acp` subcommand. `None` when neither is available.
pub(crate) fn chat_command(cfg: &Config) -> Option<(String, Vec<String>)> {
    let agent_args: &[&str] = match cfg.chat_agent {
        ChatAgent::Ez => &["acp"],
        ChatAgent::Claude => &[],
    };
    if find_executable(&cfg.acp_bin) {
        return Some((
            cfg.acp_bin.clone(),
            agent_args.iter().map(|s| s.to_string()).collect(),
        ));
    }
    if cfg.mise_bin.is_empty()
        || cfg.chat_runtime_tool.is_empty()
        || !find_executable(&cfg.mise_bin)
    {
        return None;
    }
    let args = [
        "exec",
        cfg.chat_runtime_tool.as_str(),
        "--",
        cfg.acp_bin.as_str(),
    ];
    Some((
        cfg.mise_bin.clone(),
        args.iter()
            .chain(agent_args)
            .map(|s| s.to_string())
            .collect(),
    ))
}

/// ez's user config for chats: the built-in `claude` profile (claude-code
/// driver, the chat's Claude token) as the default. A repository's
/// `.ez/config.toml` and a mode's `profile` still take precedence.
pub(crate) fn ez_config(model: &str) -> String {
    let model = toml_string(model);
    format!(
        "# Written by sandbox-init for chats (ADR-0028). Do not edit: rewritten on every chat.\n\
         default_profile = \"claude\"\n\n\
         [profiles.claude]\n\
         provider = \"claude-code\"\n\
         model = {model}\n"
    )
}

/// A TOML basic string.
fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl ChatManager {
    pub fn new(cfg: Config, reaper: Arc<Reaper>, locks: RepoLocks) -> Arc<Self> {
        Self::with_activity(cfg, reaper, locks, ActivityGate::default())
    }

    /// Like [`new`](Self::new), reporting live chats to `gate` so Docker is not pruned under them.
    pub fn with_activity(
        cfg: Config,
        reaper: Arc<Reaper>,
        locks: RepoLocks,
        activity: ActivityGate,
    ) -> Arc<Self> {
        let available = chat_command(&cfg).is_some();
        if cfg.max_chats > 0 && !available {
            tracing::warn!(bin = %cfg.acp_bin, "chat disabled: the ACP agent is not installed");
        }
        // Not readable by other uids (the chat agent shares sandbox-init's uid,
        // so this does not hide it from the agent).
        let enzarb_dir = cfg.workspace.join(".enzarb");
        if enzarb_dir.is_dir() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&enzarb_dir, std::fs::Permissions::from_mode(0o700));
        }
        Arc::new(Self {
            cfg,
            reaper,
            locks,
            available,
            state: Mutex::default(),
            next: AtomicU64::new(1),
            activity,
            sink: std::sync::OnceLock::new(),
        })
    }

    /// Capture every chat's session as events of run `chat-<chat_id>`.
    pub fn set_sink(&self, sink: Arc<dyn SessionSink>) {
        let _ = self.sink.set(sink);
    }

    /// Chat is enabled and the adapter is present.
    pub fn enabled(&self) -> bool {
        self.cfg.max_chats > 0 && self.available
    }

    /// `welcome.features`: `["acp"]` when chat is enabled.
    pub fn features(&self) -> Vec<String> {
        if self.enabled() {
            vec![feature::ACP.to_string()]
        } else {
            Vec::new()
        }
    }

    /// Number of chats starting or live.
    pub fn live(&self) -> usize {
        self.state.lock().unwrap().live.len()
    }

    fn chats_dir(&self) -> PathBuf {
        self.cfg.workspace.join(".enzarb/chats")
    }

    fn worktrees_dir(&self) -> PathBuf {
        self.cfg.workspace.join("worktrees")
    }

    fn repos_dir(&self) -> PathBuf {
        self.cfg.workspace.join("repos")
    }

    /// Handle `open{kind:"acp"}` from mux session `mux_session`.
    pub fn open(self: &Arc<Self>, mux_session: u64, req: OpenRequest) {
        if !self.enabled() {
            let msg = if self.cfg.max_chats == 0 {
                "chat is disabled in this sandbox"
            } else {
                "the chat agent is not installed in this sandbox"
            };
            return req.reject_with_code(open_error_code::CHAT_DISABLED, msg);
        }
        if req.repo.as_deref().is_some_and(|r| !valid_repo(r))
            || req.resume.as_deref().is_some_and(|r| !valid_resume(r))
        {
            return req
                .reject_with_code(open_error_code::INVALID_REQUEST, "invalid repo or resume");
        }
        let conn = self.next.fetch_add(1, Ordering::Relaxed);
        let (stop_tx, stop_rx) = watch::channel(None);
        let (done_tx, done_rx) = watch::channel(());
        let closing = Arc::new(AtomicBool::new(false));
        {
            let mut st = self.state.lock().unwrap();
            if st.shutting_down {
                drop(st);
                return req.reject("the sandbox is shutting down");
            }
            // A chat of this socket that is still tearing down does not block
            // a new open (it is waited for in `make_room` and counts against
            // the cap until its agent is gone); a live one does.
            if st
                .live
                .iter()
                .any(|l| l.mux_session == mux_session && !l.closing.load(Ordering::SeqCst))
            {
                drop(st);
                return req.reject_with_code(
                    open_error_code::CHAT_LIMIT,
                    "this connection already has a chat channel",
                );
            }
            st.live.push(Live {
                conn,
                mux_session,
                chat_id: None,
                closing: closing.clone(),
                stop: stop_tx,
                done: done_rx,
            });
        }
        let this = self.clone();
        // Held until the chat's agent is gone: the Docker pruner waits for it.
        let active = self.activity.begin();
        tokio::spawn(async move {
            let _done = done_tx;
            this.run(conn, req, stop_rx, closing).await;
            this.state.lock().unwrap().live.retain(|l| l.conn != conn);
            drop(active);
        });
    }

    /// Close every chat with `shutdown` and wait up to `wait` for them to end.
    /// New opens are refused afterwards.
    pub async fn shutdown(&self, wait: Duration) {
        let dones: Vec<watch::Receiver<()>> = {
            let mut st = self.state.lock().unwrap();
            st.shutting_down = true;
            st.live
                .iter()
                .map(|l| {
                    let _ = l.stop.send(Some(close_reason::SHUTDOWN));
                    l.done.clone()
                })
                .collect()
        };
        let all = async {
            for mut d in dones {
                while d.changed().await.is_ok() {}
            }
        };
        if tokio::time::timeout(wait, all).await.is_err() {
            tracing::warn!("chats did not end within the grace period");
        }
    }

    /// Startup cleanup: remove chat worktrees of no live chat (left by a
    /// crash or restart), prune git's worktree metadata and delete chat
    /// records older than the retention.
    /// Await it before serving, so no open races the pruning.
    pub async fn recover(self: &Arc<Self>) {
        let this = self.clone();
        {
            // Read per entry, after the listing: a chat records its id before
            // it creates its worktree, so a concurrent open is always seen.
            let live = |id: &str| {
                this.state
                    .lock()
                    .unwrap()
                    .live
                    .iter()
                    .any(|l| l.chat_id.as_deref() == Some(id))
            };
            let wt = this.worktrees_dir();
            let mut removed = 0;
            if let Ok(entries) = std::fs::read_dir(&wt) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let Some(id) = name.strip_prefix("chat-") else {
                        continue;
                    };
                    if live(id) {
                        continue;
                    }
                    let Ok(md) = std::fs::symlink_metadata(e.path()) else {
                        continue;
                    };
                    // A chat repo left by a crash: keep its branch first.
                    let kept = match read_meta(&this.chats_dir().join(id)) {
                        Some(m) if md.is_dir() => {
                            let clone = this.repos_dir().join(&m.clone);
                            let lock = this.locks.get(&clone);
                            let _g = lock.lock().await;
                            this.harvest(&clone, &e.path()).await
                        }
                        _ => !md.is_dir() || !e.path().join(".git").is_dir(),
                    };
                    this.drop_chat_repo(&e.path(), kept);
                    let r: std::io::Result<()> = Ok(());
                    if r.is_ok() {
                        removed += 1;
                    }
                }
            }
            if removed > 0 {
                tracing::info!(removed, "pruned stale chat worktrees");
                if let Ok(entries) = std::fs::read_dir(this.repos_dir()) {
                    for e in entries.flatten() {
                        if e.path().join(".git").exists() {
                            let lock = this.locks.get(&e.path());
                            let _g = lock.lock().await;
                            let _ = this.git(&e.path(), &["worktree", "prune"]).await;
                        }
                    }
                }
            }
            let cutoff = now_ms().saturating_sub(this.cfg.chat_retention.as_millis() as u64);
            if let Ok(entries) = std::fs::read_dir(this.chats_dir()) {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !valid_chat_id(&name) || live(&name) {
                        continue;
                    }
                    let last = read_meta(&e.path())
                        .map(|m| m.last_active)
                        .or_else(|| {
                            e.metadata()
                                .ok()?
                                .modified()
                                .ok()?
                                .duration_since(UNIX_EPOCH)
                                .ok()
                                .map(|d| d.as_millis() as u64)
                        })
                        .unwrap_or_default();
                    if last < cutoff {
                        let _ = std::fs::remove_dir_all(e.path());
                    }
                }
            }
        }
    }

    fn set_chat_id(&self, conn: u64, id: &str) {
        let mut st = self.state.lock().unwrap();
        if let Some(l) = st.live.iter_mut().find(|l| l.conn == conn) {
            l.chat_id = Some(id.to_string());
        }
    }

    /// Close (with `superseded`) the oldest other chats while the cap is
    /// reached, and any other chat on the same `chat_id`; wait for them.
    async fn make_room(&self, conn: u64, chat_id: &str) {
        let dones: Vec<watch::Receiver<()>> = {
            let st = self.state.lock().unwrap();
            let others: Vec<&Live> = st.live.iter().filter(|l| l.conn != conn).collect();
            let excess = (others.len() + 1).saturating_sub(self.cfg.max_chats);
            others
                .iter()
                .enumerate()
                .filter(|(i, l)| {
                    // only an older chat is ever stopped by a newer one
                    l.conn < conn && (*i < excess || l.chat_id.as_deref() == Some(chat_id))
                })
                .map(|(_, l)| {
                    let _ = l.stop.send(Some(close_reason::SUPERSEDED));
                    l.done.clone()
                })
                .collect()
        };
        let wait = async {
            for mut d in dones {
                while d.changed().await.is_ok() {}
            }
        };
        let limit = self.cfg.cancel_grace + Duration::from_secs(15);
        if tokio::time::timeout(limit, wait).await.is_err() {
            tracing::warn!("superseded chat did not end in time");
        }
    }

    /// `open.claude_token`, else the legacy file (old proxies).
    fn resolve_token(&self, req: &OpenRequest) -> Option<String> {
        crate::capture::resolve_token(req.claude_token.as_ref(), || {
            crate::capture::read_legacy_valid(&self.cfg.claude_token_file)
        })
    }

    /// The clone directory name for `repo`, if it is cloned. Without `repo`,
    /// the first clone.
    fn resolve_clone(&self, repo: Option<&str>) -> Option<String> {
        let cloned = |name: &str| self.repos_dir().join(name).join(".git").exists();
        match repo {
            Some(url) => {
                let name = repo_dir_name(url);
                cloned(&name).then_some(name)
            }
            None => {
                let mut names: Vec<String> = std::fs::read_dir(self.repos_dir())
                    .ok()?
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| !n.starts_with('.') && cloned(n))
                    .collect();
                names.sort();
                names.into_iter().next()
            }
        }
    }

    /// The registry entry whose ACP session is `sid`.
    fn find_session(&self, sid: &str) -> Option<Meta> {
        std::fs::read_dir(self.chats_dir())
            .ok()?
            .flatten()
            .filter(|e| valid_chat_id(&e.file_name().to_string_lossy()))
            .filter_map(|e| read_meta(&e.path()))
            .find(|m| m.acp_session_id.as_deref() == Some(sid) && valid_chat_id(&m.chat_id))
    }

    fn write_meta(&self, m: &Meta) {
        let dir = self.chats_dir().join(&m.chat_id);
        let _ = crate::privs::create_dir_all_mode(&dir, Some(0o700));
        if let Ok(b) = serde_json::to_vec_pretty(m)
            && let Err(e) = write_atomic(&dir.join("meta.json"), &b, 0o600)
        {
            tracing::warn!(chat = %m.chat_id, error = %e, "cannot write chat meta");
        }
    }

    async fn git(&self, cwd: &Path, args: &[&str]) -> std::io::Result<exec::Output> {
        let mut spec = CmdSpec::new(&self.cfg.git_bin, &crate::netgit::local_args(args));
        spec.cwd = Some(cwd.to_path_buf());
        spec.env = self.git_env();
        let (_keep, cancel) = watch::channel(false);
        let mut ctl = Ctl {
            cancel,
            deadline: Some(Instant::now() + Duration::from_secs(120)),
        };
        exec::run(&self.reaper, spec, &mut ctl, None).await
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

    async fn rev(&self, clone: &Path, r: &str) -> Option<String> {
        let out = self
            .git(
                clone,
                &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")],
            )
            .await
            .ok()?;
        out.success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Create `worktrees/chat-<id>`, the chat's own repository (issue #52):
    /// a fresh `git init` that borrows the job clone's objects through
    /// `objects/info/alternates`, with the clone's `origin/*` refs and tags,
    /// on branch `enzarb/chat-<id>`: from that branch when the clone has it
    /// (resume), else from the default branch. The chat never gets a `.git`
    /// that points into the clone jobs fetch and push from, so its config,
    /// hooks and refs are its own.
    async fn create_worktree(&self, clone: &Path, chat_id: &str) -> Result<PathBuf, String> {
        let wt = self.worktrees_dir().join(format!("chat-{chat_id}"));
        let branch = format!("enzarb/chat-{chat_id}");
        let lock = self.locks.get(clone);
        let _g = lock.lock().await;
        let _ = crate::privs::create_dir_all(&self.worktrees_dir());
        let kept = self.harvest(clone, &wt).await;
        self.drop_chat_repo(&wt, kept);
        let _ = self.git(clone, &["worktree", "prune"]).await;
        let resumed = self.rev(clone, &format!("refs/heads/{branch}")).await;
        let mut start = resumed.clone();
        for r in [
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
            "refs/remotes/origin/master",
            "HEAD",
        ] {
            if start.is_some() {
                break;
            }
            start = self.rev(clone, r).await;
        }
        let start = start.ok_or("the clone has no commit to start from")?;
        let res = self
            .init_chat_repo(clone, &wt, &branch, resumed.is_some(), &start)
            .await;
        if res.is_err() {
            let _ = std::fs::remove_dir_all(&wt);
        }
        res.map(|_| wt)
    }

    async fn init_chat_repo(
        &self,
        clone: &Path,
        wt: &Path,
        branch: &str,
        resumed: bool,
        start: &str,
    ) -> Result<(), String> {
        let w = wt.to_string_lossy().into_owned();
        let c = clone.to_string_lossy().into_owned();
        self.git_ok(&self.worktrees_dir(), &["init", "-q", "--template=", &w])
            .await?;
        let info = wt.join(".git/objects/info");
        crate::privs::create_dir_all(&info)
            .and_then(|_| {
                let f = info.join("alternates");
                std::fs::write(&f, format!("{}\n", clone.join(".git/objects").display()))
                    .and_then(|()| crate::privs::own(&f))
            })
            .map_err(|e| format!("cannot link the clone's objects: {e}"))?;
        let own = format!("+refs/heads/{branch}:refs/heads/{branch}");
        let mut args = vec![
            "fetch",
            "-q",
            "--no-tags",
            "--",
            &c,
            "+refs/remotes/origin/*:refs/remotes/origin/*",
            "+refs/tags/*:refs/tags/*",
        ];
        if resumed {
            args.push(&own);
        }
        self.git_ok(wt, &args).await?;
        // `origin` is the real remote (the chat holds no token to use it).
        if let Ok(out) = self
            .git(clone, &["config", "--get", "remote.origin.url"])
            .await
            && out.success()
        {
            let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !url.is_empty() {
                self.git_ok(wt, &["remote", "add", "origin", &url]).await?;
            }
        }
        self.git_ok(wt, &["checkout", "-q", "-B", branch, start])
            .await
    }

    async fn git_ok(&self, cwd: &Path, args: &[&str]) -> Result<(), String> {
        let out = self
            .git(cwd, args)
            .await
            .map_err(|e| format!("cannot run git: {e}"))?;
        if out.success() {
            Ok(())
        } else {
            Err(format!(
                "git {} failed: {}",
                args.first().unwrap_or(&""),
                out.stderr.trim()
            ))
        }
    }

    /// Remove the chat repo after copying its branch `enzarb/chat-<id>`
    /// (and so any commit) back into the clone, where resume finds it.
    async fn remove_worktree(&self, clone: &Path, wt: &Path) {
        let lock = self.locks.get(clone);
        let _g = lock.lock().await;
        let kept = self.harvest(clone, wt).await;
        self.drop_chat_repo(wt, kept);
        let _ = self.git(clone, &["worktree", "prune"]).await;
    }

    /// Fetch the chat branch of the chat repo `wt` into `clone`. Token-free:
    /// the chat repo's config is the chat's, so nothing secret may be in
    /// this process's environment (none is).
    /// True when the branch is safe in the clone (or there is no chat repo).
    async fn harvest(&self, clone: &Path, wt: &Path) -> bool {
        let Some(id) = wt
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("chat-"))
            .filter(|id| valid_chat_id(id))
        else {
            return false;
        };
        if !wt.join(".git").is_dir() {
            return true;
        }
        if !clone.join(".git").is_dir() {
            return false;
        }
        let branch = format!("refs/heads/enzarb/chat-{id}");
        let w = wt.to_string_lossy().into_owned();
        let spec = format!("+{branch}:{branch}");
        if let Err(e) = self
            .git_ok(clone, &["fetch", "-q", "--no-tags", "--", &w, &spec])
            .await
        {
            tracing::warn!(chat = %id, error = %e, "cannot keep the chat branch");
            return false;
        }
        true
    }

    /// Remove a chat repo, or, when its branch could not be kept, move it
    /// aside to `worktrees/.lost-chat-<id>-<ms>` so no commit is lost.
    fn drop_chat_repo(&self, wt: &Path, kept: bool) {
        let Ok(md) = std::fs::symlink_metadata(wt) else {
            return;
        };
        if !md.is_dir() {
            let _ = std::fs::remove_file(wt);
        } else if kept {
            let _ = std::fs::remove_dir_all(wt);
        } else {
            let name = wt.file_name().and_then(|n| n.to_str()).unwrap_or("chat");
            let lost = self
                .worktrees_dir()
                .join(format!(".lost-{name}-{}", now_ms()));
            tracing::warn!(from = %wt.display(), to = %lost.display(), "keeping a chat repo whose branch could not be saved");
            if std::fs::rename(wt, &lost).is_err() {
                let _ = std::fs::remove_dir_all(wt);
            }
        }
    }

    /// ez's config and session directories for chat `chat_id`:
    /// `<ws>/.enzarb/ez-config` (written here) and
    /// `<ws>/.enzarb/chats/<chat_id>/ez-sessions`, kept with the chat record so
    /// `session/load` finds the transcript after the chat repo is gone.
    fn ez_dirs(&self, chat_id: &str) -> Vec<(String, String)> {
        let conf = self.cfg.workspace.join(".enzarb/ez-config");
        let sessions = self.chats_dir().join(chat_id).join("ez-sessions");
        let _ = crate::privs::create_dir_all_mode(&conf, Some(0o700));
        let _ = crate::privs::create_dir_all_mode(&sessions, Some(0o700));
        if let Err(e) = write_atomic(
            &conf.join("config.toml"),
            ez_config(&self.cfg.chat_model).as_bytes(),
            0o600,
        ) {
            tracing::warn!(error = %e, "cannot write the ez chat config");
        }
        vec![
            ("EZ_CONFIG_DIR".into(), conf.to_string_lossy().into_owned()),
            (
                "EZ_SESSIONS_DIR".into(),
                sessions.to_string_lossy().into_owned(),
            ),
        ]
    }

    /// The agent's whole environment.
    fn agent_env(&self, token: &str, config_dir: &Path) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = PASS_ENV
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        env.extend(self.git_env());
        env.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), token.to_string()));
        env.push((
            "CLAUDE_CONFIG_DIR".into(),
            config_dir.to_string_lossy().into_owned(),
        ));
        env
    }

    /// Secrets redacted from browser traffic and transcripts.
    fn secrets(&self, token: &str) -> Vec<String> {
        let mut s = filter::token_forms(token);
        if let Ok(t) = std::fs::read_to_string(&self.cfg.auth_token_file) {
            let t = t.trim();
            if t.len() >= 8 {
                s.push(t.to_string());
            }
        }
        s
    }

    async fn run(
        self: &Arc<Self>,
        conn: u64,
        req: OpenRequest,
        mut stop: watch::Receiver<Option<&'static str>>,
        closing: Arc<AtomicBool>,
    ) {
        let Some(token) = self.resolve_token(&req) else {
            return req.reject_with_code(
                open_error_code::NO_CREDENTIAL,
                "Claude is not logged in in this sandbox",
            );
        };
        // Resume: only a session in sandbox-init's registry, on the requested clone.
        let resumed = req
            .resume
            .as_deref()
            .and_then(|sid| self.find_session(sid))
            .filter(|m| match req.repo.as_deref() {
                Some(url) => repo_dir_name(url) == m.clone,
                None => true,
            })
            .filter(|m| self.repos_dir().join(&m.clone).join(".git").exists());
        let clone_name = match &resumed {
            Some(m) => Some(m.clone.clone()),
            None => self.resolve_clone(req.repo.as_deref()),
        };
        let Some(clone_name) = clone_name else {
            return req.reject_with_code(
                open_error_code::REPO_NOT_CLONED,
                "no job has cloned this repository in the sandbox yet",
            );
        };
        let clone = self.repos_dir().join(&clone_name);
        let (chat_id, created, resume_sid) = match &resumed {
            Some(m) => (m.chat_id.clone(), m.created, m.acp_session_id.clone()),
            None => (new_chat_id(), now_ms(), None),
        };
        self.set_chat_id(conn, &chat_id);
        self.make_room(conn, &chat_id).await;
        if stop.borrow().is_some() {
            return req.reject("the chat was closed while starting");
        }
        let wt = match self.create_worktree(&clone, &chat_id).await {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(chat = %chat_id, error = %e, "cannot create chat worktree");
                return req.reject("cannot create the chat worktree");
            }
        };
        let mut meta = Meta {
            chat_id: chat_id.clone(),
            repo: req.repo.clone(),
            clone: clone_name,
            worktree: wt.clone(),
            acp_session_id: resume_sid.clone(),
            created,
            last_active: now_ms(),
        };
        self.write_meta(&meta);
        let mut t = Transcript::open(&self.chats_dir().join(&chat_id).join("transcript.jsonl"));
        t.capture = self
            .sink
            .get()
            .map(|s| (s.clone(), format!("chat-{chat_id}")));
        let transcript = Arc::new(t);
        let secrets = self.secrets(&token);
        let config_dir = self.cfg.workspace.join(".enzarb/chat-home/.claude");
        let _ = crate::privs::create_dir_all_mode(&config_dir, Some(0o700));
        let mut env = self.agent_env(&token, &config_dir);
        if self.cfg.chat_agent == ChatAgent::Ez {
            env.extend(self.ez_dirs(&chat_id));
        }
        env.extend(crate::repos::target_dir_env(
            &self.cfg.workspace,
            req.repo.as_deref(),
            &env,
        ));
        let sink: LineSink = {
            let t = transcript.clone();
            let secrets = secrets.clone();
            Arc::new(move |line: &str| {
                t.append(
                    "log",
                    json!({"source": "acp", "line": filter::redact_str(line, &secrets)}),
                );
            })
        };
        let (bin, args) = chat_command(&self.cfg).unwrap_or((self.cfg.acp_bin.clone(), Vec::new()));
        let sp = match acp::spawn_agent(&self.reaper, &bin, &args, &wt, &env, true, Some(sink)) {
            Ok(sp) => sp,
            Err(e) => {
                let m = match e {
                    AcpError::Failed(m) => m,
                    AcpError::Stopped(s) => format!("{s:?}"),
                };
                self.remove_worktree(&clone, &wt).await;
                return req.reject(&format!("cannot start the chat agent: {m}"));
            }
        };
        let guard = PidGuard(sp.pid);
        let handle = match req.accept() {
            Ok(h) => h,
            Err(_) => {
                drop(guard);
                self.remove_worktree(&clone, &wt).await;
                return;
            }
        };
        tracing::info!(chat = %chat_id, pid = sp.pid, resume = resume_sid.is_some(), "chat opened");
        transcript.append(
            "lifecycle",
            json!({"event": "opened", "resume": resume_sid.is_some()}),
        );
        let filter = match self.cfg.chat_agent {
            ChatAgent::Ez => Filter::ez(wt.clone(), secrets, resume_sid),
            ChatAgent::Claude => Filter::new(
                self.cfg.chat_permission_mode,
                self.cfg.chat_allow_bypass,
                wt.clone(),
                secrets,
                resume_sid,
            ),
        };
        let registry = {
            let this = self.clone();
            let base = meta.clone();
            move |sid: &str| {
                let mut m = base.clone();
                m.acp_session_id = Some(sid.to_string());
                m.last_active = now_ms();
                this.write_meta(&m);
            }
        };
        let (end, session) = relay(
            handle,
            sp,
            filter,
            &mut stop,
            &transcript,
            &registry,
            RelayLimits {
                closing,
                idle: self.cfg.chat_idle,
                max: self.cfg.chat_max,
                cancel_grace: self.cfg.cancel_grace,
            },
        )
        .await;
        drop(guard);
        transcript.append(
            "lifecycle",
            json!({"event": "closed", "reason": end.reason()}),
        );
        if let Some((sink, run_id)) = &transcript.capture {
            sink.ended(run_id);
        }
        tracing::info!(chat = %chat_id, reason = %end.reason(), "chat closed");
        if let Some(s) = session {
            meta.acp_session_id = Some(s);
        } else if let Some(m) = read_meta(&self.chats_dir().join(&chat_id)) {
            meta.acp_session_id = m.acp_session_id;
        }
        meta.last_active = now_ms();
        self.write_meta(&meta);
        self.remove_worktree(&clone, &wt).await;
    }
}

/// Read and validate `meta.json` (the agent runs as the same uid, so its
/// contents are not trusted: ids and the clone name are re-checked).
fn read_meta(dir: &Path) -> Option<Meta> {
    let m: Meta = serde_json::from_slice(&std::fs::read(dir.join("meta.json")).ok()?).ok()?;
    let clone_ok = !m.clone.is_empty()
        && !m.clone.starts_with('.')
        && m.clone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    let sid_ok = m
        .acp_session_id
        .as_deref()
        .is_none_or(filter::valid_session_id);
    (valid_chat_id(&m.chat_id)
        && dir.file_name().is_some_and(|n| n == m.chat_id.as_str())
        && clone_ok
        && sid_ok)
        .then_some(m)
}

struct RelayLimits {
    /// Set when the relay loop ends (teardown starts).
    closing: Arc<AtomicBool>,
    idle: Duration,
    max: Duration,
    cancel_grace: Duration,
}

/// Resolves with the stop reason once one is set (never if the sender is gone).
async fn stopped(rx: &mut watch::Receiver<Option<&'static str>>) -> &'static str {
    loop {
        if let Some(r) = *rx.borrow_and_update() {
            return r;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// A writer task with byte accounting: `queued` drops as data is written.
struct Queue {
    tx: Option<mpsc::UnboundedSender<Bytes>>,
    queued: Arc<AtomicUsize>,
}

impl Queue {
    fn push(&self, v: &Value) {
        let mut b = v.to_string().into_bytes();
        b.push(b'\n');
        self.queued.fetch_add(b.len(), Ordering::Relaxed);
        if let Some(tx) = &self.tx {
            let _ = tx.send(Bytes::from(b));
        }
    }
    fn full(&self) -> bool {
        self.queued.load(Ordering::Relaxed) >= HIGH_WATER
    }
}

fn agent_writer(
    mut stdin: tokio::net::unix::pipe::Sender,
    drained: Arc<Notify>,
) -> (Queue, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Bytes>();
    let queued = Arc::new(AtomicUsize::new(0));
    let q = queued.clone();
    let task = tokio::spawn(async move {
        while let Some(b) = rx.recv().await {
            let n = b.len();
            let ok = stdin.write_all(&b).await.is_ok();
            q.fetch_sub(n, Ordering::Relaxed);
            drained.notify_one();
            if !ok {
                // the agent is gone; keep draining so the accounting stays right
                while let Some(b) = rx.recv().await {
                    q.fetch_sub(b.len(), Ordering::Relaxed);
                }
                break;
            }
        }
        // dropping `stdin` closes the agent's stdin
    });
    (
        Queue {
            tx: Some(tx),
            queued,
        },
        task,
    )
}

fn browser_writer(
    send: Arc<tokio::sync::Mutex<ChannelSender>>,
    drained: Arc<Notify>,
) -> (Queue, tokio::task::JoinHandle<()>) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Bytes>();
    let queued = Arc::new(AtomicUsize::new(0));
    let q = queued.clone();
    let task = tokio::spawn(async move {
        let mut ok = true;
        while let Some(b) = rx.recv().await {
            let n = b.len();
            if ok {
                ok = send.lock().await.send(b).await.is_ok();
            }
            q.fetch_sub(n, Ordering::Relaxed);
            drained.notify_one();
        }
    });
    (
        Queue {
            tx: Some(tx),
            queued,
        },
        task,
    )
}

/// Relay until the chat ends. Returns why, and the ACP session id if one was
/// established.
#[allow(clippy::too_many_arguments)]
async fn relay(
    handle: ChannelHandle,
    sp: Spawned,
    mut filter: Filter,
    stop: &mut watch::Receiver<Option<&'static str>>,
    transcript: &Transcript,
    registry: &(dyn Fn(&str) + Send + Sync),
    limits: RelayLimits,
) -> (End, Option<String>) {
    let Spawned {
        pid,
        mut exit,
        stdin,
        mut stdout,
    } = sp;
    let (send, mut recv) = handle.split();
    let send = Arc::new(tokio::sync::Mutex::new(send));
    let drained = Arc::new(Notify::new());
    let (mut to_agent, agent_task) = agent_writer(stdin, drained.clone());
    let (mut to_browser, browser_task) = browser_writer(send.clone(), drained.clone());
    let mut from_browser = LineBuf::new(MAX_LINE);
    let mut from_agent = LineBuf::new(MAX_LINE);
    let mut buf = vec![0u8; 64 * 1024];
    let deadline = Instant::now() + limits.max;
    let mut last = Instant::now();
    // `Some` once the agent's exit status resolved (a oneshot is polled once).
    let mut exited: Option<Option<ExitStatus>> = None;
    let mut exit_deadline = Instant::now();

    let dispatch = |outs: Vec<Out>, to_agent: &Queue, to_browser: &Queue, browser: bool| {
        for o in outs {
            match o {
                Out::Agent(v) => to_agent.push(&v),
                Out::Browser(v) if browser => to_browser.push(&v),
                Out::Browser(_) => {}
                Out::Record(k, v) => transcript.append(k, v),
                Out::Session(s) => registry(&s),
            }
        }
    };

    let end = loop {
        let idle_at = last + limits.idle;
        tokio::select! {
            biased;
            r = stopped(stop) => break End::Reason(r),
            d = recv.recv(), if !to_agent.full() => match d {
                None => break End::BrowserClosed,
                Some(d) => {
                    last = Instant::now();
                    match from_browser.push(d.bytes()) {
                        Err(LineTooLong) => break End::Reason(close_reason::LIMIT),
                        Ok(lines) => for l in lines {
                            let outs = filter.from_browser(&l);
                            dispatch(outs, &to_agent, &to_browser, true);
                        },
                    }
                }
            },
            n = stdout.read(&mut buf), if !to_browser.full() => match n {
                Ok(0) | Err(_) => {
                    if exited.is_none()
                        && let Ok(st) = tokio::time::timeout(Duration::from_secs(2), &mut exit).await
                    {
                        exited = Some(st.ok());
                    }
                    break End::Exited(exited.flatten());
                }
                Ok(n) => {
                    last = Instant::now();
                    match from_agent.push(&buf[..n]) {
                        Err(LineTooLong) => break End::Reason(close_reason::LIMIT),
                        Ok(lines) => for l in lines {
                            let outs = filter.from_agent(&l);
                            dispatch(outs, &to_agent, &to_browser, true);
                        },
                    }
                }
            },
            st = &mut exit, if exited.is_none() => {
                // Read what the agent wrote before it died: stdout EOF ends the
                // loop, or this deadline if a grandchild keeps stdout open.
                exited = Some(st.ok());
                exit_deadline = Instant::now() + Duration::from_secs(2);
            }
            () = tokio::time::sleep_until(exit_deadline), if exited.is_some() => {
                break End::Exited(exited.flatten());
            }
            () = tokio::time::sleep_until(deadline) => break End::Reason(close_reason::LIMIT),
            () = tokio::time::sleep_until(idle_at) => break End::Reason(close_reason::IDLE),
            () = drained.notified() => {}
        }
    };

    limits.closing.store(true, Ordering::SeqCst);
    // Pending permissions resolve `cancelled`, never allowed; the turn is cancelled.
    let outs = filter.cancel_permissions();
    dispatch(outs, &to_agent, &to_browser, false);
    let agent_alive = !matches!(end, End::Exited(_));
    if agent_alive && let Some(c) = filter.session_cancel() {
        to_agent.push(&c);
    }
    // Tell the browser first; it does not wait for the agent.
    to_browser.tx = None;
    // A browser that grants no credit must not hold up teardown: abort the
    // writer (releasing the sender lock) when it does not drain in time.
    let mut browser_task = browser_task;
    if tokio::time::timeout(Duration::from_secs(1), &mut browser_task)
        .await
        .is_err()
    {
        browser_task.abort();
        let _ = browser_task.await;
    }
    {
        let mut s = send.lock().await;
        let code = if end == End::Reason(close_reason::SHUTDOWN) {
            close_code::GOING_AWAY
        } else {
            close_code::NORMAL
        };
        let _ = s.close(code, &end.reason());
    }
    drop(recv);
    // Give the agent `cancel_grace` to end the turn, then close its stdin and stop it.
    if agent_alive {
        let grace_end = Instant::now() + limits.cancel_grace;
        while filter.prompting() {
            let n = tokio::select! {
                n = stdout.read(&mut buf) => n,
                () = tokio::time::sleep_until(grace_end) => break,
            };
            match n {
                Ok(0) | Err(_) => break,
                Ok(n) => match from_agent.push(&buf[..n]) {
                    Err(_) => break,
                    Ok(lines) => {
                        for l in lines {
                            let outs = filter.from_agent(&l);
                            dispatch(outs, &to_agent, &to_browser, false);
                        }
                    }
                },
            }
        }
    }
    to_agent.tx = None;
    let mut agent_task = agent_task;
    if tokio::time::timeout(Duration::from_secs(2), &mut agent_task)
        .await
        .is_err()
    {
        agent_task.abort();
        let _ = agent_task.await;
    }
    drop(stdout);
    if exited.is_none()
        && tokio::time::timeout(Duration::from_secs(2), &mut exit)
            .await
            .is_err()
    {
        terminate(pid, &mut exit).await;
    }
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    (end, filter.session().map(String::from))
}

#[cfg(test)]
mod tests;
