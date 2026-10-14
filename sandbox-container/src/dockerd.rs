//! `dockerd` supervisor (ADR-0016).
//!
//! The sandbox container runs its own Docker daemon, so bind mounts such as
//! `docker run -v /workspace/repo:/src` see the same filesystem as the agent, and no socket has to
//! cross containers (gVisor's gofer cannot carry them). sandbox-init, pid 1, starts the daemon as
//! root, restarts it with exponential backoff when it exits, reports its state in `/healthz`, and
//! stops it first on SIGTERM so it can release its data root before the drain completes.
//!
//! Docker is best-effort: if the daemon cannot start, agents and chats still work and the failure
//! is logged and visible in `/healthz` (which stays HTTP 200).
//!
//! The daemon is a child registered with the [`Reaper`] like every other child, so its exit status
//! reaches the supervisor and is not stolen by the reap-all loop.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

use crate::privs::SANDBOX_ID;
use crate::reaper::{ExitStatus, Reaper};

/// Runtime-specific flags used when `ENZARB_DOCKERD_ARGS` is not set: the gVisor profile
/// (<https://gvisor.dev/docs/tutorials/docker-in-gvisor/>). The operator passes its runtime
/// profile's `dockerdArgs` in `ENZARB_DOCKERD_ARGS`.
pub const DEFAULT_ARGS: &[&str] = &[
    "--iptables=false",
    "--ip6tables=false",
    // vfs works on any backing fs; overlay2 on a gofer-backed PVC is unverified.
    "--storage-driver=vfs",
    "--feature=containerd-snapshotter=false",
];

#[derive(Debug, Clone)]
pub struct DockerdConfig {
    /// `ENZARB_DOCKERD=off|0|false|no` disables the supervisor (default on).
    pub enabled: bool,
    /// `ENZARB_DOCKERD_BIN`, default `dockerd`.
    pub bin: String,
    /// `ENZARB_DOCKERD_ARGS` (whitespace separated): runtime-specific flags, after the common ones.
    pub args: Vec<String>,
    /// `ENZARB_DOCKERD_SOCKET`, default `/var/run/docker.sock`.
    pub socket: PathBuf,
    /// `ENZARB_DOCKERD_DATA_ROOT`, default `/var/lib/docker` (the `docker-data` PVC).
    pub data_root: PathBuf,
    /// `ENZARB_DOCKERD_EXEC_ROOT`, default `/var/run/dockerd`.
    pub exec_root: PathBuf,
    /// Restart backoff: first delay, ceiling, and how long a run must last to reset it.
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub stable_after: Duration,
    /// How often readiness is probed while `starting`.
    pub probe_interval: Duration,
}

impl Default for DockerdConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bin: "dockerd".into(),
            args: DEFAULT_ARGS.iter().map(|s| s.to_string()).collect(),
            socket: "/var/run/docker.sock".into(),
            data_root: "/var/lib/docker".into(),
            exec_root: "/var/run/dockerd".into(),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
            probe_interval: Duration::from_millis(500),
        }
    }
}

impl DockerdConfig {
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let mut c = Self::default();
        if let Some(v) = get("ENZARB_DOCKERD") {
            c.enabled = match v.trim().to_ascii_lowercase().as_str() {
                "" | "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => anyhow::bail!("ENZARB_DOCKERD: must be on or off"),
            };
        }
        if let Some(v) = get("ENZARB_DOCKERD_BIN").filter(|s| !s.is_empty()) {
            c.bin = v;
        }
        if let Some(v) = get("ENZARB_DOCKERD_ARGS") {
            c.args = v.split_whitespace().map(String::from).collect();
        }
        if let Some(v) = get("ENZARB_DOCKERD_SOCKET").filter(|s| !s.is_empty()) {
            c.socket = v.into();
        }
        if let Some(v) = get("ENZARB_DOCKERD_DATA_ROOT").filter(|s| !s.is_empty()) {
            c.data_root = v.into();
        }
        if let Some(v) = get("ENZARB_DOCKERD_EXEC_ROOT").filter(|s| !s.is_empty()) {
            c.exec_root = v.into();
        }
        Ok(c)
    }

    /// Full `dockerd` argument list: the common flags, then the runtime-specific ones.
    pub fn command_args(&self) -> Vec<String> {
        let mut a = vec![
            format!("--host=unix://{}", self.socket.display()),
            // Socket group = the sandbox user's gid, so the unprivileged sandbox can connect.
            format!("--group={SANDBOX_ID}"),
            format!("--data-root={}", self.data_root.display()),
            format!("--exec-root={}", self.exec_root.display()),
            format!("--pidfile={}/docker.pid", self.exec_root.display()),
        ];
        a.extend(self.args.iter().cloned());
        a
    }
}

/// What `/healthz` reports under `dockerd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum State {
    /// `ENZARB_DOCKERD=off`.
    Disabled,
    /// Spawned, the socket does not answer yet.
    Starting,
    Running,
    /// Exited or could not start; `retry_in_secs` is `None` when the supervisor gave up.
    Failed {
        reason: String,
        retry_in_secs: Option<u64>,
    },
    /// Stopped by shutdown.
    Stopped,
}

pub struct Dockerd {
    cfg: DockerdConfig,
    reaper: Arc<Reaper>,
    state: watch::Sender<State>,
    stop: CancellationToken,
    done: watch::Sender<bool>,
    grace: Mutex<Duration>,
    started: AtomicBool,
}

impl Dockerd {
    pub fn new(cfg: DockerdConfig, reaper: Arc<Reaper>) -> Arc<Self> {
        let initial = if cfg.enabled {
            State::Starting
        } else {
            State::Disabled
        };
        Arc::new(Self {
            cfg,
            reaper,
            state: watch::channel(initial).0,
            stop: CancellationToken::new(),
            done: watch::channel(false).0,
            grace: Mutex::new(Duration::from_secs(25)),
            started: AtomicBool::new(false),
        })
    }

    pub fn state(&self) -> State {
        self.state.borrow().clone()
    }

    /// A receiver for state changes.
    pub fn subscribe(&self) -> watch::Receiver<State> {
        self.state.subscribe()
    }

    fn set(&self, s: State) {
        if *self.state.borrow() != s {
            match &s {
                State::Failed { reason, .. } => tracing::warn!(%reason, "dockerd: failed"),
                other => tracing::info!(state = ?other, "dockerd"),
            }
            self.state.send_replace(s);
        }
    }

    /// Run the supervisor in a task; it ends on [`shutdown`](Self::shutdown), or when the daemon
    /// is disabled or not installed.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        self.started.store(true, Ordering::SeqCst);
        tokio::spawn(async move {
            if me.cfg.enabled {
                me.supervise().await;
            }
            me.done.send_replace(true);
        })
    }

    /// Stop the daemon (SIGTERM, then SIGKILL after `grace`) and the restart loop. Returns once
    /// the supervisor has finished. A no-op if [`start`](Self::start) was never called.
    pub async fn shutdown(&self, grace: Duration) {
        if !self.cfg.enabled || !self.started.load(Ordering::SeqCst) {
            return;
        }
        *self.grace.lock().unwrap() = grace;
        self.stop.cancel();
        let mut done = self.done.subscribe();
        let _ = done.wait_for(|d| *d).await;
    }

    /// Sleep for `d`; `true` if shutdown was requested meanwhile.
    async fn cancelled_within(&self, d: Duration) -> bool {
        tokio::select! {
            _ = self.stop.cancelled() => true,
            _ = tokio::time::sleep(d) => false,
        }
    }

    async fn supervise(&self) {
        let mut backoff = self.cfg.backoff_initial;
        while !self.stop.is_cancelled() {
            self.set(State::Starting);
            let _ = std::fs::create_dir_all(&self.cfg.exec_root);
            // A stale pidfile from a killed daemon would make dockerd refuse to start.
            let _ = std::fs::remove_file(self.cfg.exec_root.join("docker.pid"));
            let (pid, mut exit) = match self.spawn() {
                Ok(x) => x,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Not installed (custom image): Docker is simply unavailable.
                    self.set(State::Failed {
                        reason: format!("{} not found; Docker is unavailable", self.cfg.bin),
                        retry_in_secs: None,
                    });
                    return;
                }
                Err(e) => {
                    self.set(State::Failed {
                        reason: format!("cannot start {}: {e}", self.cfg.bin),
                        retry_in_secs: Some(backoff.as_secs()),
                    });
                    if self.cancelled_within(backoff).await {
                        break;
                    }
                    backoff = (backoff * 2).min(self.cfg.backoff_max);
                    continue;
                }
            };
            tracing::info!(pid, "dockerd started");
            let started = tokio::time::Instant::now();
            let mut tick = tokio::time::interval(self.cfg.probe_interval);
            let status = loop {
                let starting = self.state() == State::Starting;
                tokio::select! {
                    biased;
                    _ = self.stop.cancelled() => {
                        self.terminate(pid, &mut exit).await;
                        self.set(State::Stopped);
                        return;
                    }
                    st = &mut exit => break st.ok(),
                    _ = tick.tick(), if starting => {
                        if ping(&self.cfg.socket).await {
                            self.set(State::Running);
                        }
                    }
                }
            };
            // Leftover containerd/shims from a daemon that died: do not let them hold the data root.
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
            if started.elapsed() >= self.cfg.stable_after {
                backoff = self.cfg.backoff_initial;
            }
            self.set(State::Failed {
                reason: format!("dockerd exited ({}); restarting", describe(status)),
                retry_in_secs: Some(backoff.as_secs()),
            });
            if self.cancelled_within(backoff).await {
                break;
            }
            backoff = (backoff * 2).min(self.cfg.backoff_max);
        }
        self.set(State::Stopped);
    }

    fn spawn(&self) -> std::io::Result<(u32, oneshot::Receiver<ExitStatus>)> {
        use std::os::unix::process::CommandExt;
        // The only child that stays root (ADR-0016): every other spawn goes through `privs`.
        let mut cmd = Command::new(&self.cfg.bin);
        cmd.args(self.cfg.command_args())
            .env("HOME", "/root")
            .stdin(Stdio::null());
        // Own session: the whole group (containerd, shims) can be signalled together.
        // SAFETY: `setsid` is async-signal-safe and the closure does nothing else.
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(std::io::Error::from)
            });
        }
        self.reaper.spawn_with(|| {
            let c = cmd.spawn()?;
            let pid = c.id();
            Ok((pid, pid))
        })
    }

    /// SIGTERM, wait `grace`, then SIGKILL the group.
    async fn terminate(&self, pid: u32, exit: &mut oneshot::Receiver<ExitStatus>) {
        let grace = *self.grace.lock().unwrap();
        let p = Pid::from_raw(pid as i32);
        let _ = kill(p, Signal::SIGTERM);
        if tokio::time::timeout(grace, &mut *exit).await.is_err() {
            tracing::warn!("dockerd did not stop within the grace period; killed");
            let _ = killpg(p, Signal::SIGKILL);
            let _ = kill(p, Signal::SIGKILL);
            let _ = tokio::time::timeout(Duration::from_secs(5), &mut *exit).await;
        }
        let _ = killpg(p, Signal::SIGKILL);
    }
}

fn describe(st: Option<ExitStatus>) -> String {
    match st {
        Some(ExitStatus::Exited(c)) => format!("code {c}"),
        Some(ExitStatus::Signaled(s)) => format!("signal {s}"),
        None => "status lost".into(),
    }
}

/// `GET /_ping` on the daemon's socket.
pub async fn ping(socket: &std::path::Path) -> bool {
    let probe = async {
        let mut s = tokio::net::UnixStream::connect(socket).await?;
        s.write_all(b"GET /_ping HTTP/1.0\r\n\r\n").await?;
        let mut buf = Vec::new();
        s.take(1024).read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(buf)
    };
    match tokio::time::timeout(Duration::from_secs(2), probe).await {
        Ok(Ok(buf)) => buf.starts_with(b"HTTP/1.") && buf.windows(5).any(|w| w == b" 200 "),
        _ => false,
    }
}
