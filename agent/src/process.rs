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

const STATE_FILE: &str = ".enzarb/processes.json";
pub const LOG_DIR: &str = ".enzarb/tasks";
const BROADCAST_CAPACITY: usize = 512;

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
        let home = home_dir();
        let cwd = process
            .cwd
            .as_deref()
            .unwrap_or_else(|| home.to_str().unwrap_or("/"));

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(&process.command);
        cmd.args(&process.args);
        cmd.cwd(cwd);
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

        // Blocking reader thread: PTY output → broadcast channel.
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut reader = reader;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let _ = tx.send(buf[..n].to_vec());
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
            input_writer: Some(Arc::new(std::sync::Mutex::new(writer))),
            pty_master: Some(master),
            pid,
        })
    }

    async fn spawn_oneshot(&self, process: &Process) -> Result<RuntimeHandle> {
        let home = home_dir();
        let cwd = process
            .cwd
            .as_deref()
            .unwrap_or_else(|| home.to_str().unwrap_or("/"));

        let log_path = home_dir().join(LOG_DIR).join(format!("{}.log", process.id));
        if let Some(parent) = log_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut cmd = tokio::process::Command::new(&process.command);
        cmd.args(&process.args);
        cmd.current_dir(cwd);
        cmd.envs(&process.env);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

        let (output_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let tx = output_tx.clone();

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

    /// Returns (output_receiver, optional_input_writer, optional_pty_master) for
    /// attaching a WebSocket terminal to a running process.
    pub async fn attach(
        &self,
        id: &str,
    ) -> Option<(
        broadcast::Receiver<Vec<u8>>,
        Option<Arc<std::sync::Mutex<Box<dyn Write + Send>>>>,
        Option<Arc<std::sync::Mutex<Box<dyn portable_pty::MasterPty + Send>>>>,
    )> {
        let handles = self.handles.read().await;
        handles.get(id).map(|h| {
            (
                h.output_tx.subscribe(),
                h.input_writer.clone(),
                h.pty_master.clone(),
            )
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
