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

pub const TMUX_SESSION: &str = "enzarb";
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
        // tmux destroys a session when its last window closes (and with
        // `exit-empty on`, the server too). If a previous process was the last
        // window and exited, the session is gone, so `new-window -t` would fail.
        // Re-ensure it here so creating a process always works, not just at boot.
        ensure_tmux_session().await?;

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
    // Interactive shells launch with this managed rcfile so we control the prompt
    // (sets PS1 last, after sourcing the system/user rc) without touching the
    // image rootfs. The file lives on the home PVC.
    let shell_cmd = match write_managed_bashrc().await {
        Ok(rc) => Some(format!("bash --rcfile {}", rc.to_string_lossy())),
        Err(e) => {
            tracing::warn!(error = %e, "failed to write managed bashrc; using default shell");
            None
        }
    };

    // Give shells inside tmux a 256-color terminfo (default is `screen`, which
    // makes many tools disable colour) and pass truecolor through to the client.
    // Set before creating the session so its shell inherits the right TERM.
    let _ = Command::new("tmux")
        .args(["set-option", "-g", "default-terminal", "tmux-256color"])
        .status()
        .await;
    let _ = Command::new("tmux")
        .args([
            "set-option",
            "-ga",
            "terminal-overrides",
            ",xterm-256color:RGB",
        ])
        .status()
        .await;

    let status = Command::new("tmux")
        .args(["has-session", "-t", TMUX_SESSION])
        .status()
        .await?;

    if !status.success() {
        let mut args = vec!["new-session", "-d", "-s", TMUX_SESSION, "-n", "agent"];
        if let Some(cmd) = &shell_cmd {
            args.push(cmd);
        }
        Command::new("tmux").args(&args).status().await?;
    }

    // Make our prompt the default for any shell tmux spawns without an explicit
    // command (idempotent; safe to set every time).
    if let Some(cmd) = &shell_cmd {
        let _ = Command::new("tmux")
            .args(["set-option", "-g", "default-command", cmd])
            .status()
            .await;
    }
    Ok(())
}

// write_managed_bashrc writes (idempotently) the enzarb-managed bash rcfile that
// sources the standard rc files and then overrides PS1 to a clean prompt: the
// hostname is the project slug, with no generic `user` login name.
async fn write_managed_bashrc() -> Result<PathBuf> {
    let path = home_dir().join(".enzarb").join("bashrc");
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    // Raw string so the literal backslashes in PS1 and the shell `$`/quotes are
    // written verbatim, without Rust escaping.
    let contents = r#"# Managed by enzarb — do not edit. Sources the standard rc files, enables bash
# completion (including mise-installed tools), then sets a clean prompt last.
[ -r /etc/profile ] && . /etc/profile 2>/dev/null
[ -r /etc/bash.bashrc ] && . /etc/bash.bashrc 2>/dev/null
[ -r "$HOME/.bashrc" ] && . "$HOME/.bashrc" 2>/dev/null

# Base bash-completion (dynamic completion loader).
if ! type _init_completion >/dev/null 2>&1; then
  for f in /usr/share/bash-completion/bash_completion /etc/bash_completion; do
    [ -r "$f" ] && . "$f" && break
  done
fi

# Completions for mise and common mise-installed CLIs. Each tool's script is
# generated once and cached, so shells start fast and tools installed later get
# picked up on their next shell. Tools not present are skipped.
__ez_comp_dir="$HOME/.enzarb/completions"
mkdir -p "$__ez_comp_dir" 2>/dev/null
__ez_comp() {
  local tool="$1"; shift
  command -v "$tool" >/dev/null 2>&1 || return
  local f="$__ez_comp_dir/$tool.bash"
  if [ ! -s "$f" ]; then
    "$@" >"$f" 2>/dev/null || { rm -f "$f"; return; }
  fi
  [ -s "$f" ] && . "$f" 2>/dev/null
}
__ez_comp mise      mise completion bash
__ez_comp kubectl   kubectl completion bash
__ez_comp helm      helm completion bash
__ez_comp gh        gh completion -s bash
__ez_comp kustomize kustomize completion bash
__ez_comp k9s       k9s completion bash
__ez_comp pnpm      pnpm completion bash
__ez_comp deno      deno completions bash
__ez_comp just      just --completions bash
__ez_comp uv        uv generate-shell-completion bash
__ez_comp rustup    rustup completions bash
__ez_comp npm       npm completion
# terraform-style tools complete via the binary itself rather than a script.
command -v terraform  >/dev/null 2>&1 && complete -C terraform terraform
command -v terragrunt >/dev/null 2>&1 && complete -C terragrunt terragrunt

PS1='\[\e[1;32m\]\h\[\e[0m\]:\[\e[1;34m\]\w\[\e[0m\]\$ '
"#;
    tokio::fs::write(&path, contents).await?;
    Ok(path)
}
