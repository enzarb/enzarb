use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::init::home_dir;

const TMUX_SESSION: &str = "enzarb";
const STATE_FILE: &str = ".enzarb/processes.json";
const LOG_DIR: &str = ".enzarb/tasks";
#[allow(dead_code)]
pub const MAX_LOG_BYTES: u64 = 100 * 1024 * 1024; // 100MB cap for one-shot task logs

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
    pub window_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct State {
    processes: Vec<Process>,
}

#[derive(Clone)]
pub struct ProcessStore {
    inner: Arc<RwLock<HashMap<String, Process>>>,
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
            inner: Arc::new(RwLock::new(map)),
            state_path,
        };

        // Ensure tmux session exists
        ensure_tmux_session().await?;

        // Relaunch persistent processes
        store.rehydrate().await?;

        Ok(store)
    }

    async fn rehydrate(&self) -> Result<()> {
        let processes: Vec<Process> = {
            let inner = self.inner.read().await;
            inner
                .values()
                .filter(|p| p.kind == ProcessKind::Persistent)
                .cloned()
                .collect()
        };

        for p in processes {
            tracing::info!(id = p.id, name = p.name, "rehydrating persistent process");
            if let Err(e) = self.start_in_tmux(&p).await {
                tracing::warn!(id = p.id, error = %e, "failed to rehydrate process");
            }
        }
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
        let window_name = format!("proc-{}", &id[..8]);

        let process = Process {
            id: id.clone(),
            name,
            command,
            args,
            cwd,
            env,
            kind,
            status: ProcessStatus::Running,
            exit_code: None,
            started_at: Utc::now(),
            finished_at: None,
            window_name: window_name.clone(),
        };

        self.start_in_tmux(&process).await?;

        let mut inner = self.inner.write().await;
        inner.insert(id, process.clone());
        drop(inner);
        self.persist().await?;

        Ok(process)
    }

    async fn start_in_tmux(&self, process: &Process) -> Result<()> {
        let log_path = home_dir().join(LOG_DIR).join(format!("{}.log", process.id));
        let log_path_str = log_path.to_string_lossy();

        // Build command string with tee for one-shot processes
        let cmd_parts: Vec<String> = std::iter::once(process.command.clone())
            .chain(process.args.iter().cloned())
            .collect();
        let cmd_str = cmd_parts.join(" ");

        let full_cmd = if process.kind == ProcessKind::OneShot {
            format!("({}) 2>&1 | tee {:?}", cmd_str, log_path_str)
        } else {
            cmd_str
        };

        let home = home_dir();
        let home_str = home.to_string_lossy().to_string();
        let cwd = process.cwd.as_deref().unwrap_or(&home_str);

        let mut tmux_cmd = Command::new("tmux");
        tmux_cmd
            .arg("new-window")
            .arg("-t")
            .arg(TMUX_SESSION)
            .arg("-n")
            .arg(&process.window_name)
            .arg("-c")
            .arg(cwd);

        // Set env vars
        for (k, v) in &process.env {
            tmux_cmd.arg("-e").arg(format!("{}={}", k, v));
        }

        tmux_cmd.arg(full_cmd);

        let status = tmux_cmd.status().await?;
        if !status.success() {
            return Err(anyhow!("tmux new-window failed for process {}", process.id));
        }
        Ok(())
    }

    pub async fn kill(&self, id: &str) -> Result<()> {
        let process = {
            let inner = self.inner.read().await;
            inner
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow!("process not found: {}", id))?
        };

        let _ = Command::new("tmux")
            .args([
                "kill-window",
                "-t",
                &format!("{}:{}", TMUX_SESSION, process.window_name),
            ])
            .status()
            .await;

        let mut inner = self.inner.write().await;
        inner.remove(id);
        drop(inner);
        self.persist().await?;
        Ok(())
    }

    pub async fn list(&self) -> Vec<Process> {
        let inner = self.inner.read().await;
        inner.values().cloned().collect()
    }

    pub async fn get(&self, id: &str) -> Option<Process> {
        let inner = self.inner.read().await;
        inner.get(id).cloned()
    }

    pub fn log_path(&self, id: &str) -> PathBuf {
        home_dir().join(LOG_DIR).join(format!("{}.log", id))
    }

    pub fn window_target(&self, window_name: &str) -> String {
        format!("{}:{}", TMUX_SESSION, window_name)
    }

    async fn persist(&self) -> Result<()> {
        let inner = self.inner.read().await;
        let state = State {
            processes: inner.values().cloned().collect(),
        };
        let data = serde_json::to_vec_pretty(&state)?;
        tokio::fs::write(&self.state_path, data).await?;
        Ok(())
    }
}

async fn ensure_tmux_session() -> Result<()> {
    let status = Command::new("tmux")
        .args(["has-session", "-t", TMUX_SESSION])
        .status()
        .await?;

    if !status.success() {
        Command::new("tmux")
            .args(["new-session", "-d", "-s", TMUX_SESSION, "-n", "agent"])
            .status()
            .await?;
    }
    Ok(())
}
