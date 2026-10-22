//! claude-agent-acp as a child process, driven over newline-delimited JSON-RPC (ACP).
//!
//! ez owns the system prompt and every tool: the session is created with `systemPrompt` in
//! override mode, Claude Code's built-in tools disabled (`tools: []`), and ez's tools served by
//! [`ToolServer`] as the `ez` MCP server. Claude Code only runs the model loop (with the
//! subscription token, which never leaves Claude Code).

use crate::tool_server::{ToolCall, ToolServer};
use anyhow::{Context, Result, anyhow, bail};
use ez_llm::ToolSpec;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};

/// Env var overriding the child command (whitespace-separated; default `claude-agent-acp`).
pub const COMMAND_ENV: &str = "EZ_CLAUDE_AGENT_ACP";
/// The MCP server name ez's tools are served under; tools appear as `mcp__ez__<name>`.
pub const SERVER_NAME: &str = "ez";
const STDERR_TAIL: usize = 40;

pub struct DriverConfig {
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub model: String,
    pub tools: Vec<ToolSpec>,
    /// Command to run; `None` reads [`COMMAND_ENV`], falling back to `claude-agent-acp`.
    pub command: Option<Vec<String>>,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// A running claude-agent-acp session. Dropping it kills the child and stops the tool server.
pub struct ClaudeDriver {
    server: ToolServer,
    _child: Child,
    out: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: AtomicU64,
    session_id: String,
    stderr: Arc<Mutex<VecDeque<String>>>,
    /// `session/update` payloads (the `update` object) in arrival order.
    pub updates: mpsc::UnboundedReceiver<Value>,
    /// Tool calls the child made against ez's MCP server.
    pub tool_calls: mpsc::UnboundedReceiver<ToolCall>,
}

/// The MCP name Claude Code gives an ez tool.
pub fn mcp_name(tool: &str) -> String {
    format!("mcp__{SERVER_NAME}__{tool}")
}

fn command(cfg: &DriverConfig) -> Result<Vec<String>> {
    let cmd = match &cfg.command {
        Some(c) => c.clone(),
        None => std::env::var(COMMAND_ENV)
            .ok()
            .map(|v| v.split_whitespace().map(String::from).collect())
            .filter(|v: &Vec<String>| !v.is_empty())
            .unwrap_or_else(|| vec!["claude-agent-acp".into()]),
    };
    if cmd.is_empty() {
        bail!("empty claude-agent-acp command");
    }
    Ok(cmd)
}

/// `session/new` params: ez's prompt, no built-in tools, ez's tools over MCP.
pub fn session_new_params(cfg: &DriverConfig, url: &str, token: &str) -> Value {
    let aliases: Map<String, Value> = cfg
        .tools
        .iter()
        .map(|t| (t.name.clone(), Value::String(mcp_name(&t.name))))
        .collect();
    let mut options = json!({
        "tools": [],
        "allowedTools": [format!("mcp__{SERVER_NAME}__*")],
        "toolAliases": aliases,
        // No CLAUDE.md, settings hooks or project .mcp.json: ez's configuration is the only one.
        "settingSources": [],
        "strictMcpConfig": true,
    });
    if !cfg.model.is_empty() {
        options["model"] = json!(cfg.model);
    }
    json!({
        "cwd": cfg.cwd,
        "mcpServers": [{
            "type": "http",
            "name": SERVER_NAME,
            "url": url,
            "headers": [{"name": "Authorization", "value": format!("Bearer {token}")}],
        }],
        "systemPrompt": {
            "mode": "override",
            "content": [{"type": "text", "text": cfg.system_prompt}],
        },
        "_meta": {"claudeCode": {"options": options}},
    })
}

/// Answer to an agent→client request. Permissions are always granted: ez gates each call in
/// its own tool handler, and nothing but ez's MCP tools is enabled.
fn answer(method: &str, params: &Value) -> std::result::Result<Value, Value> {
    match method {
        "session/request_permission" => {
            let options = params["options"].as_array().cloned().unwrap_or_default();
            let pick = options
                .iter()
                .find(|o| o["kind"] == "allow_once")
                .or_else(|| options.iter().find(|o| o["kind"] == "allow_always"))
                .or(options.first());
            match pick.and_then(|o| o["optionId"].as_str()) {
                Some(id) => Ok(json!({"outcome": {"outcome": "selected", "optionId": id}})),
                None => Ok(json!({"outcome": {"outcome": "cancelled"}})),
            }
        }
        _ => Err(json!({"code": -32601, "message": format!("method not found: {method}")})),
    }
}

impl ClaudeDriver {
    /// Start the tool server and the child, then `initialize` and `session/new`.
    pub async fn start(cfg: DriverConfig) -> Result<Self> {
        let (server, tool_calls) = ToolServer::start(cfg.tools.clone()).await?;
        let argv = command(&cfg)?;
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&cfg.cwd)
            // Long-running ez tools (builds, tests) must not hit Claude Code's MCP timeout.
            .env("MCP_TOOL_TIMEOUT", "1800000")
            .env("DISABLE_AUTOUPDATER", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting `{}`", argv.join(" ")))?;
        let mut stdin = child.stdin.take().context("child stdin")?;
        let stdout = child.stdout.take().context("child stdout")?;
        let stderr_pipe = child.stderr.take().context("child stderr")?;

        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if stdin.write_all(line.as_bytes()).await.is_err()
                    || stdin.write_all(b"\n").await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let stderr = Arc::new(Mutex::new(VecDeque::new()));
        tokio::spawn({
            let stderr = stderr.clone();
            async move {
                let mut lines = BufReader::new(stderr_pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "ez_claude::child", "{line}");
                    let mut tail = stderr.lock().unwrap();
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            }
        });

        let pending: Pending = Arc::default();
        let (updates_tx, updates) = mpsc::unbounded_channel();
        tokio::spawn({
            let pending = pending.clone();
            let out = out.clone();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                        tracing::debug!(target: "ez_claude::child", "non-JSON stdout: {line}");
                        continue;
                    };
                    let method = msg["method"].as_str();
                    match (method, msg.get("id")) {
                        (Some(m), Some(id)) => {
                            let reply = match answer(m, &msg["params"]) {
                                Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
                                Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
                            };
                            let _ = out.send(reply.to_string());
                        }
                        (Some("session/update"), None) => {
                            let _ = updates_tx.send(msg["params"]["update"].clone());
                        }
                        (Some(_), None) => {}
                        (None, Some(id)) => {
                            let Some(id) = id.as_u64() else { continue };
                            let Some(tx) = pending.lock().unwrap().remove(&id) else {
                                continue;
                            };
                            let r = match msg.get("error") {
                                Some(e) => Err(anyhow!(
                                    "claude-agent-acp error: {}",
                                    e["message"].as_str().unwrap_or(&e.to_string())
                                )),
                                None => Ok(msg["result"].clone()),
                            };
                            let _ = tx.send(r);
                        }
                        (None, None) => {}
                    }
                }
                // Child gone: fail whatever is still waiting.
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err(anyhow!("claude-agent-acp exited")));
                }
            }
        });

        let mut driver = Self {
            _child: child,
            out,
            pending,
            next_id: AtomicU64::new(1),
            session_id: String::new(),
            stderr,
            updates,
            tool_calls,
            server,
        };
        driver
            .call(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": {"readTextFile": false, "writeTextFile": false},
                        "terminal": false,
                    },
                    "clientInfo": {"name": "ez", "version": env!("CARGO_PKG_VERSION")},
                }),
            )
            .await?;
        let params = session_new_params(&cfg, driver.server.url(), driver.server.token());
        let created = driver.call("session/new", params).await?;
        driver.session_id = created["sessionId"]
            .as_str()
            .context("session/new returned no sessionId")?
            .to_string();
        Ok(driver)
    }

    fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Result<Value>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        if self.out.send(msg.to_string()).is_err()
            && let Some(tx) = self.pending.lock().unwrap().remove(&id)
        {
            let _ = tx.send(Err(anyhow!("claude-agent-acp exited")));
        }
        rx
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let r = self.request(method, params).await;
        r.unwrap_or_else(|_| Err(anyhow!("claude-agent-acp exited")))
            .map_err(|e| self.with_stderr(e))
            .with_context(|| format!("claude-agent-acp `{method}`"))
    }

    /// Attach the child's recent stderr to an error.
    pub fn with_stderr(&self, e: anyhow::Error) -> anyhow::Error {
        let tail = self.stderr.lock().unwrap();
        if tail.is_empty() {
            return e;
        }
        let lines: Vec<&str> = tail.iter().map(String::as_str).collect();
        anyhow!("{e:#}\nclaude-agent-acp stderr:\n{}", lines.join("\n"))
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Send a prompt; the receiver yields the `session/prompt` result (`stopReason`, `usage`).
    pub fn prompt(&self, text: &str) -> oneshot::Receiver<Result<Value>> {
        self.request(
            "session/prompt",
            json!({
                "sessionId": self.session_id,
                "prompt": [{"type": "text", "text": text}],
            }),
        )
    }

    /// Ask the child to stop the running prompt (`session/cancel`).
    pub fn cancel(&self) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "session/cancel",
            "params": {"sessionId": self.session_id},
        });
        let _ = self.out.send(msg.to_string());
    }

    /// Replace the tools ez serves (new calls see them; Claude Code may cache the list).
    pub fn set_tools(&self, tools: Vec<ToolSpec>) {
        self.server.set_tools(tools);
    }
}
