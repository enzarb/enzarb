use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::init::home_dir;

const ENV_CONTEXT_PATH: &str = "/var/run/enzarb/env/context.sh";

/// Parse `export KEY=VALUE` lines from the mounted env context ConfigMap and
/// return them as a map. Called at spawn time so ConfigMap updates (~60s
/// propagation) are picked up without an agent restart.
fn load_env_context() -> HashMap<String, String> {
    let Ok(contents) = std::fs::read_to_string(ENV_CONTEXT_PATH) else {
        return HashMap::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim().strip_prefix("export ")?;
            let (k, v) = line.split_once('=')?;
            Some((k.to_owned(), v.to_owned()))
        })
        .collect()
}

fn resolve_cwd(cwd: Option<&str>) -> String {
    let home = home_dir();
    let home_str = home.to_string_lossy();
    match cwd {
        None => home_str.into_owned(),
        Some("~") => home_str.into_owned(),
        Some(p) if p.starts_with("~/") => format!("{}/{}", home_str, &p[2..]),
        Some(p) => p.to_owned(),
    }
}

const STATE_FILE: &str = ".enzarb/processes.json";
pub const LOG_DIR: &str = ".enzarb/tasks";
const BROADCAST_CAPACITY: usize = 512;
// Rolling scrollback kept in memory so reconnecting clients can replay output.
const SCROLLBACK_LIMIT: usize = 1024 * 1024; // 1 MiB

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessKind {
    Persistent,
    OneShot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    Running,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Process {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub env: HashMap<String, String>,
    pub kind: ProcessKind,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct State {
    processes: Vec<Process>,
}

// Non-serializable runtime state kept only while the process is alive.
pub struct RuntimeHandle {
    pub output_tx: broadcast::Sender<Vec<u8>>,
    // Rolling tail of all output; replayed to reconnecting clients.
    pub scrollback: Arc<std::sync::Mutex<Vec<u8>>>,
    // Only set for PTY (persistent) processes.
    pub input_writer: Option<Arc<std::sync::Mutex<Box<dyn Write + Send>>>>,
    // Mutex provides Sync (MasterPty is Send but not Sync).
    pub pty_master: Option<Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>>,
    pub pid: Option<u32>,
}

#[derive(Clone)]
pub struct ProcessStore {
    records: Arc<RwLock<HashMap<String, Process>>>,
    handles: Arc<RwLock<HashMap<String, RuntimeHandle>>>,
    state_path: PathBuf,
}

impl ProcessStore {
    pub async fn load_or_create() -> Result<Self> {
        let state_path = home_dir().join(STATE_FILE);
        let mut map = HashMap::new();

        if state_path.exists() {
            let data = tokio::fs::read(&state_path).await?;
            if let Ok(state) = serde_json::from_slice::<State>(&data) {
                for p in state.processes {
                    map.insert(p.id.clone(), p);
                }
            }
        }

        let store = ProcessStore {
            records: Arc::new(RwLock::new(map)),
            handles: Arc::new(RwLock::new(HashMap::new())),
            state_path,
        };

        store.rehydrate().await?;
        Ok(store)
    }

    async fn rehydrate(&self) -> Result<()> {
        let to_relaunch: Vec<Process> = {
            let records = self.records.read().await;
            records
                .values()
                .filter(|p| p.kind == ProcessKind::Persistent && p.status == ProcessStatus::Running)
                .cloned()
                .collect()
        };

        for p in to_relaunch {
            tracing::info!(id = p.id, name = p.name, "rehydrating persistent process");
            match self.spawn_pty(&p).await {
                Ok(handle) => {
                    self.handles.write().await.insert(p.id.clone(), handle);
                }
                Err(e) => {
                    tracing::warn!(id = p.id, error = %e, "failed to rehydrate");
                }
            }
        }

        // One-shot processes that were Running when the agent died can't be recovered.
        {
            let mut records = self.records.write().await;
            for p in records.values_mut() {
                if p.kind == ProcessKind::OneShot && p.status == ProcessStatus::Running {
                    p.status = ProcessStatus::Failed;
                    p.finished_at = Some(Utc::now());
                }
            }
        }
        self.persist().await?;
        Ok(())
    }

    pub async fn create(
        &self,
        name: String,
        command: String,
        args: Vec<String>,
        cwd: Option<String>,
        env: HashMap<String, String>,
        kind: ProcessKind,
    ) -> Result<Process> {
        let id = Uuid::new_v4().to_string();
        let process = Process {
            id: id.clone(),
            name,
            command,
            args,
            cwd,
            env,
            kind: kind.clone(),
            status: ProcessStatus::Running,
            exit_code: None,
            started_at: Utc::now(),
            finished_at: None,
        };

        let handle = match kind {
            ProcessKind::Persistent => self.spawn_pty(&process).await?,
            ProcessKind::OneShot => self.spawn_oneshot(&process).await?,
        };

        self.records
            .write()
            .await
            .insert(id.clone(), process.clone());
        self.handles.write().await.insert(id, handle);
        self.persist().await?;
        Ok(process)
    }

    async fn spawn_pty(&self, process: &Process) -> Result<RuntimeHandle> {
        let cwd = resolve_cwd(process.cwd.as_deref());

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Run through `mise exec` so tool-specific PATH entries (e.g. npm-global
        // bins inside a mise-managed node install) are available, matching what
        // an interactive shell session sees after `mise activate`.
        let mut cmd = CommandBuilder::new("mise");
        cmd.args(["exec", "--", &process.command]);
        cmd.args(&process.args);
        cmd.cwd(&cwd);
        // Context env (e.g. POD_NAMESPACE) from the mounted ConfigMap; process env overrides.
        for (k, v) in load_env_context() {
            cmd.env(k, v);
        }
        for (k, v) in &process.env {
            cmd.env(k, v);
        }
        cmd.env("TERM", "xterm-256color");

        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let pid = child.process_id();
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let master = Arc::new(std::sync::Mutex::new(pair.master));

        let (output_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let tx = output_tx.clone();
        let scrollback: Arc<std::sync::Mutex<Vec<u8>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sb = scrollback.clone();

        // Blocking reader thread: PTY output → broadcast channel + scrollback.
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut reader = reader;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let chunk = buf[..n].to_vec();
                        let _ = tx.send(chunk.clone());
                        if let Ok(mut sb) = sb.lock() {
                            sb.extend_from_slice(&chunk);
                            if sb.len() > SCROLLBACK_LIMIT {
                                let drop = sb.len() - SCROLLBACK_LIMIT;
                                sb.drain(..drop);
                            }
                        }
                    }
                }
            }
        });

        // Wait for child exit and update the record.
        let store = self.clone();
        let id = process.id.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || child.wait()).await;
            let (status, code) = match result {
                Ok(Ok(s)) if s.success() => (ProcessStatus::Exited, Some(0)),
                _ => (ProcessStatus::Failed, Some(1)),
            };
            store.mark_exited(&id, status, code).await;
        });

        Ok(RuntimeHandle {
            output_tx,
            scrollback,
            input_writer: Some(Arc::new(std::sync::Mutex::new(writer))),
            pty_master: Some(master),
            pid,
        })
    }

    async fn spawn_oneshot(&self, process: &Process) -> Result<RuntimeHandle> {
        let cwd = resolve_cwd(process.cwd.as_deref());

        let log_path = home_dir().join(LOG_DIR).join(format!("{}.log", process.id));
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Run through `mise exec` for the same reason as spawn_pty.
        let mut cmd = tokio::process::Command::new("mise");
        cmd.args(["exec", "--", &process.command]);
        cmd.args(&process.args);
        cmd.current_dir(&cwd);
        // Context env (e.g. POD_NAMESPACE) from the mounted ConfigMap; process env overrides.
        cmd.envs(load_env_context());
        cmd.envs(&process.env);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

        let (output_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let tx = output_tx.clone();
        let scrollback: Arc<std::sync::Mutex<Vec<u8>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let sb = scrollback.clone();

        let store = self.clone();
        let id = process.id.clone();
        tokio::spawn(async move {
            let log_file = match tokio::fs::File::create(&log_path).await {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to open log file");
                    return;
                }
            };
            let mut log = tokio::io::BufWriter::new(log_file);
            let mut combined =
                futures::stream::select(ReaderStream::new(stdout), ReaderStream::new(stderr));
            while let Some(chunk) = combined.next().await {
                if let Ok(bytes) = chunk {
                    let _ = tx.send(bytes.to_vec());
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut log, &bytes).await;
                    if let Ok(mut sb) = sb.lock() {
                        sb.extend_from_slice(&bytes);
                        if sb.len() > SCROLLBACK_LIMIT {
                            let drop = sb.len() - SCROLLBACK_LIMIT;
                            sb.drain(..drop);
                        }
                    }
                }
            }
            let _ = tokio::io::AsyncWriteExt::flush(&mut log).await;

            let (status, code) = match child.wait().await {
                Ok(s) => {
                    let code = s.code();
                    if s.success() {
                        (ProcessStatus::Exited, code.or(Some(0)))
                    } else {
                        (ProcessStatus::Failed, code.or(Some(1)))
                    }
                }
                Err(_) => (ProcessStatus::Failed, Some(1)),
            };
            store.mark_exited(&id, status, code).await;
        });

        Ok(RuntimeHandle {
            output_tx,
            scrollback,
            input_writer: None,
            pty_master: None,
            pid,
        })
    }

    async fn mark_exited(&self, id: &str, status: ProcessStatus, code: Option<i32>) {
        {
            let mut records = self.records.write().await;
            if let Some(p) = records.get_mut(id) {
                p.status = status;
                p.exit_code = code;
                p.finished_at = Some(Utc::now());
            }
        }
        self.handles.write().await.remove(id);
        let _ = self.persist().await;
    }

    pub async fn kill(&self, id: &str) -> Result<()> {
        let pid = {
            let handles = self.handles.read().await;
            handles.get(id).and_then(|h| h.pid)
        };

        if let Some(pid) = pid {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
        }

        self.records.write().await.remove(id);
        self.handles.write().await.remove(id);
        self.persist().await?;
        Ok(())
    }

    pub async fn list(&self) -> Vec<Process> {
        self.records.read().await.values().cloned().collect()
    }

    pub async fn get(&self, id: &str) -> Option<Process> {
        self.records.read().await.get(id).cloned()
    }

    pub fn log_path(&self, id: &str) -> PathBuf {
        home_dir().join(LOG_DIR).join(format!("{}.log", id))
    }

    /// Returns (scrollback_snapshot, output_receiver, optional_input_writer,
    /// optional_pty_master) for attaching a WebSocket terminal to a running process.
    ///
    /// The receiver is subscribed BEFORE the scrollback is snapshotted so there
    /// is no gap: any output written after subscription is in the live channel,
    /// and everything before is in the scrollback. Clients send scrollback first,
    /// then relay the live channel, giving seamless reconnect replay.
    pub async fn attach(
        &self,
        id: &str,
    ) -> Option<(
        Vec<u8>,
        broadcast::Receiver<Vec<u8>>,
        Option<Arc<std::sync::Mutex<Box<dyn Write + Send>>>>,
        Option<Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>>,
    )> {
        let handles = self.handles.read().await;
        handles.get(id).map(|h| {
            // Subscribe before snapshotting: guarantees no output gap on reconnect.
            let rx = h.output_tx.subscribe();
            let scrollback = h.scrollback.lock().map(|s| s.clone()).unwrap_or_default();
            (scrollback, rx, h.input_writer.clone(), h.pty_master.clone())
        })
    }

    async fn persist(&self) -> Result<()> {
        let records = self.records.read().await;
        let state = State {
            processes: records.values().cloned().collect(),
        };
        let data = serde_json::to_vec_pretty(&state)?;
        tokio::fs::write(&self.state_path, data).await?;
        Ok(())
    }
}
