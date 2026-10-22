//! MCP (Model Context Protocol) client support: connect to configured servers and expose their
//! tools as [`ez_tools::Tool`]s named `mcp__<server>__<tool>`.

use async_trait::async_trait;
use ez_tools::{Tool, ToolContext, ToolKind, ToolOutput, truncate_middle};
use globset::{Glob, GlobSet, GlobSetBuilder};
use rmcp::model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_OUTPUT: usize = 30_000;
const MAX_TOOL_NAME: usize = 64;
/// Default number of selected MCP tools above which the lazy meta-tools are exposed instead.
pub const DEFAULT_LAZY_THRESHOLD: usize = 20;

/// One MCP server: stdio (`command`) or streamable HTTP (`url`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// `"stdio"` or `"http"`; inferred from `command` / `url` when absent.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Working directory for stdio servers (relative to the session cwd).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Stdio,
    Http,
}

impl McpServerConfig {
    pub fn stdio(command: impl Into<String>, args: &[&str]) -> Self {
        Self {
            command: Some(command.into()),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    pub fn http(url: impl Into<String>) -> Self {
        Self {
            url: Some(url.into()),
            ..Default::default()
        }
    }

    pub fn transport(&self) -> Result<TransportKind, String> {
        match self.kind.as_deref() {
            Some("stdio") => Ok(TransportKind::Stdio),
            Some("http") | Some("streamable-http") | Some("streamable_http") => {
                Ok(TransportKind::Http)
            }
            Some("sse") => Err("SSE transport is not supported; use type \"http\"".into()),
            Some(other) => Err(format!("unknown MCP server type `{other}`")),
            None if self.command.is_some() => Ok(TransportKind::Stdio),
            None if self.url.is_some() => Ok(TransportKind::Http),
            None => Err("MCP server needs `command` or `url`".into()),
        }
    }
}

/// Parse a Claude Code style `.mcp.json`: `{"mcpServers": {name: {...}}}`.
pub fn parse_mcp_json(text: &str) -> anyhow::Result<BTreeMap<String, McpServerConfig>> {
    let v: Value = serde_json::from_str(text)?;
    let servers = v
        .get("mcpServers")
        .or_else(|| v.get("servers"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(serde_json::from_value(servers)?)
}

/// Convert an ACP `mcpServers` entry (stdio `{name, command, args, env: [{name,value}]}` or
/// http `{type: "http", name, url, headers: [{name,value}]}`) into a name and config.
pub fn from_acp(v: &Value) -> Option<(String, McpServerConfig)> {
    let name = v.get("name")?.as_str()?.to_string();
    let pairs = |key: &str| -> BTreeMap<String, String> {
        v.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some((
                    p.get("name")?.as_str()?.to_string(),
                    p.get("value")?.as_str()?.to_string(),
                ))
            })
            .collect()
    };
    let cfg = match v.get("type").and_then(Value::as_str) {
        Some("http") => McpServerConfig {
            kind: Some("http".into()),
            url: Some(v.get("url")?.as_str()?.to_string()),
            headers: pairs("headers"),
            ..Default::default()
        },
        Some("sse") => return None,
        _ => McpServerConfig {
            kind: Some("stdio".into()),
            command: Some(v.get("command")?.as_str()?.to_string()),
            args: v
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(String::from))
                .collect(),
            env: pairs("env"),
            ..Default::default()
        },
    };
    Some((name, cfg))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `mcp__<server>__<tool>` restricted to `[A-Za-z0-9_-]` and at most 64 chars.
pub fn tool_name(server: &str, tool: &str) -> String {
    let mut s = format!("mcp__{}__{}", sanitize(server), sanitize(tool));
    s.truncate(MAX_TOOL_NAME);
    s
}

type Client = RunningService<RoleClient, ClientConfig>;

/// A single MCP tool exposed to the agent.
pub struct McpTool {
    name: String,
    server: String,
    remote_name: String,
    description: String,
    schema: Value,
    kind: ToolKind,
    client: Arc<Client>,
}

impl McpTool {
    pub fn server(&self) -> &str {
        &self.server
    }

    pub fn remote_name(&self) -> &str {
        &self.remote_name
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let mut params = CallToolRequestParams::new(self.remote_name.clone());
        match input {
            Value::Object(m) => params.arguments = Some(m),
            Value::Null => {}
            _ => anyhow::bail!("tool arguments must be a JSON object"),
        }
        let res = tokio::select! {
            r = self.client.call_tool(params) => r,
            _ = ctx.cancel.cancelled() => return Ok(ToolOutput::error("MCP call cancelled")),
        };
        let res = match res {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::error(format!("MCP call failed: {e}"))),
        };
        let v = serde_json::to_value(&res)?;
        let text = render_result(&v);
        let text = truncate_middle(&text, MAX_OUTPUT);
        if v["isError"].as_bool() == Some(true) {
            Ok(ToolOutput::error(text))
        } else {
            Ok(ToolOutput::ok(text))
        }
    }
}

/// Flatten a `tools/call` result into text.
fn render_result(v: &Value) -> String {
    let mut parts: Vec<String> = vec![];
    for item in v["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("text") => parts.push(item["text"].as_str().unwrap_or_default().to_string()),
            Some("image") => parts.push(format!(
                "[image: {}]",
                item["mimeType"].as_str().unwrap_or("unknown")
            )),
            Some("audio") => parts.push(format!(
                "[audio: {}]",
                item["mimeType"].as_str().unwrap_or("unknown")
            )),
            Some("resource") => {
                let r = &item["resource"];
                match r["text"].as_str() {
                    Some(t) => parts.push(format!(
                        "[resource {}]\n{t}",
                        r["uri"].as_str().unwrap_or("?")
                    )),
                    None => parts.push(format!(
                        "[resource: {} ({})]",
                        r["uri"].as_str().unwrap_or("?"),
                        r["mimeType"].as_str().unwrap_or("binary")
                    )),
                }
            }
            Some("resource_link") => parts.push(format!(
                "[resource: {}]",
                item["uri"].as_str().unwrap_or("?")
            )),
            other => parts.push(format!("[{}]", other.unwrap_or("unknown content"))),
        }
    }
    if parts.is_empty() {
        if let Some(sc) = v.get("structuredContent").filter(|s| !s.is_null()) {
            return sc.to_string();
        }
        return "(no output)".into();
    }
    parts.join("\n")
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        if self.description.is_empty() {
            format!(
                "MCP tool `{}` from server `{}`",
                self.remote_name, self.server
            )
        } else {
            self.description.clone()
        }
    }

    fn input_schema(&self) -> Value {
        self.schema.clone()
    }

    fn kind(&self) -> ToolKind {
        self.kind
    }

    fn title(&self, _: &Value) -> String {
        format!("{}: {}", self.server, self.remote_name)
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        self.invoke(input, ctx).await
    }
}

/// Lazy meta tool: list MCP tools (optionally for one server).
pub struct McpListTools {
    tools: Vec<Arc<McpTool>>,
}

#[async_trait]
impl Tool for McpListTools {
    fn name(&self) -> &str {
        "McpListTools"
    }

    fn description(&self) -> String {
        "List available MCP tools with descriptions and input schemas. Optionally filter by \
         server. Call one with McpCall."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {
            "server": {"type": "string", "description": "Only list tools of this MCP server"}
        }})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
        let server = input.get("server").and_then(Value::as_str);
        let lines: Vec<String> = self
            .tools
            .iter()
            .filter(|t| server.is_none_or(|s| t.server == s))
            .map(|t| {
                format!(
                    "{} — {}\n  input schema: {}",
                    t.name,
                    t.description.lines().next().unwrap_or_default(),
                    t.schema
                )
            })
            .collect();
        if lines.is_empty() {
            return Ok(ToolOutput::ok("(no MCP tools available)"));
        }
        Ok(ToolOutput::ok(truncate_middle(
            &lines.join("\n"),
            MAX_OUTPUT,
        )))
    }
}

/// Lazy meta tool: call an MCP tool by its full name.
pub struct McpCall {
    tools: Vec<Arc<McpTool>>,
}

#[async_trait]
impl Tool for McpCall {
    fn name(&self) -> &str {
        "McpCall"
    }

    fn description(&self) -> String {
        "Call an MCP tool by its full name (`mcp__<server>__<tool>`). Use McpListTools to see \
         available tools and their input schemas."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {
            "tool": {"type": "string", "description": "Full tool name, e.g. mcp__github__search"},
            "arguments": {"type": "object", "description": "Arguments for the tool"}
        }, "required": ["tool"]})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Other
    }

    fn title(&self, input: &Value) -> String {
        format!("mcp: {}", input["tool"].as_str().unwrap_or("?"))
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let name = input
            .get("tool")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required string parameter `tool`"))?;
        let Some(tool) = self.tools.iter().find(|t| t.name == name) else {
            return Ok(ToolOutput::error(format!(
                "unknown or disallowed MCP tool `{name}`; use McpListTools to see available tools"
            )));
        };
        let args = input.get("arguments").cloned().unwrap_or(json!({}));
        tool.invoke(args, ctx).await
    }
}

/// Result of connecting to one server.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerStatus {
    pub name: String,
    /// Tool count, or the error message.
    pub result: Result<usize, String>,
    /// Full tool names (empty on failure).
    pub tools: Vec<String>,
}

/// Owns the connections to all configured MCP servers for a session.
#[derive(Default)]
pub struct McpManager {
    tools: Vec<Arc<McpTool>>,
    status: Vec<ServerStatus>,
    clients: Vec<Arc<Client>>,
}

struct Connected {
    client: Arc<Client>,
    tools: Vec<rmcp::model::Tool>,
}

fn tail(buf: &Mutex<Vec<String>>) -> String {
    buf.lock().unwrap().join("\n")
}

async fn connect(name: &str, cfg: &McpServerConfig, cwd: &Path) -> Result<Connected, String> {
    let stderr_tail: Arc<Mutex<Vec<String>>> = Arc::default();
    let info = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("ez", env!("CARGO_PKG_VERSION")),
    );
    let client = match cfg.transport()? {
        TransportKind::Stdio => {
            let command = cfg.command.clone().ok_or("missing `command`")?;
            let mut cmd = tokio::process::Command::new(&command);
            cmd.args(&cfg.args).envs(&cfg.env);
            cmd.current_dir(match &cfg.cwd {
                Some(p) if p.is_absolute() => p.clone(),
                Some(p) => cwd.join(p),
                None => cwd.to_path_buf(),
            });
            cmd.kill_on_drop(true);
            let (transport, stderr) = TokioChildProcess::builder(cmd)
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("failed to start `{command}`: {e}"))?;
            if let Some(stderr) = stderr {
                let buf = stderr_tail.clone();
                let server = name.to_string();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!("mcp[{server}] stderr: {line}");
                        let mut b = buf.lock().unwrap();
                        b.push(line);
                        if b.len() > 10 {
                            b.remove(0);
                        }
                    }
                });
            }
            info.serve(transport).await
        }
        TransportKind::Http => {
            let url = cfg.url.clone().ok_or("missing `url`")?;
            let mut config = StreamableHttpClientTransportConfig::with_uri(url);
            let mut headers = HashMap::new();
            for (k, v) in &cfg.headers {
                if k.eq_ignore_ascii_case("authorization")
                    && let Some(tok) = v.strip_prefix("Bearer ")
                {
                    config = config.auth_header(tok.trim());
                    continue;
                }
                let hn = http::HeaderName::from_bytes(k.as_bytes())
                    .map_err(|e| format!("invalid header name `{k}`: {e}"))?;
                let hv = http::HeaderValue::from_str(v)
                    .map_err(|e| format!("invalid value for header `{k}`: {e}"))?;
                headers.insert(hn, hv);
            }
            let config = config.custom_headers(headers);
            info.serve(StreamableHttpClientTransport::from_config(config))
                .await
        }
    };
    let client = client.map_err(|e| {
        let t = tail(&stderr_tail);
        if t.is_empty() {
            format!("initialize failed: {e}")
        } else {
            format!("initialize failed: {e}\nserver stderr:\n{t}")
        }
    })?;
    let tools = client
        .list_all_tools()
        .await
        .map_err(|e| format!("tools/list failed: {e}"))?;
    Ok(Connected {
        client: Arc::new(client),
        tools,
    })
}

impl McpManager {
    /// Connect to all servers concurrently (30s timeout each). Failures are logged and recorded
    /// in [`statuses`](Self::statuses), never fatal.
    pub async fn start(configs: BTreeMap<String, McpServerConfig>, cwd: &Path) -> McpManager {
        let futs = configs.iter().map(|(name, cfg)| async move {
            let r = match tokio::time::timeout(CONNECT_TIMEOUT, connect(name, cfg, cwd)).await {
                Ok(r) => r,
                Err(_) => Err(format!(
                    "timed out after {}s connecting",
                    CONNECT_TIMEOUT.as_secs()
                )),
            };
            (name.clone(), r)
        });
        let results = futures::future::join_all(futs).await;
        let mut m = McpManager::default();
        let mut used: HashSet<String> = HashSet::new();
        for (name, r) in results {
            match r {
                Err(e) => {
                    tracing::warn!("MCP server `{name}` failed: {e}");
                    m.status.push(ServerStatus {
                        name,
                        result: Err(e),
                        tools: vec![],
                    });
                }
                Ok(c) => {
                    let mut names = vec![];
                    for t in c.tools {
                        let v = serde_json::to_value(&t).unwrap_or_default();
                        let remote = t.name.to_string();
                        let mut full = tool_name(&name, &remote);
                        let mut n = 2;
                        while !used.insert(full.clone()) {
                            let suffix = format!("_{n}");
                            full = tool_name(&name, &remote);
                            full.truncate(MAX_TOOL_NAME - suffix.len());
                            full.push_str(&suffix);
                            n += 1;
                        }
                        let read_only = v["annotations"]["readOnlyHint"].as_bool() == Some(true);
                        let schema = match v.get("inputSchema") {
                            Some(s @ Value::Object(_)) => s.clone(),
                            _ => json!({"type": "object", "properties": {}}),
                        };
                        names.push(full.clone());
                        m.tools.push(Arc::new(McpTool {
                            name: full,
                            server: name.clone(),
                            remote_name: remote,
                            description: t.description.as_deref().unwrap_or_default().to_string(),
                            schema,
                            kind: if read_only {
                                ToolKind::Read
                            } else {
                                ToolKind::Other
                            },
                            client: c.client.clone(),
                        }));
                    }
                    m.status.push(ServerStatus {
                        name,
                        result: Ok(names.len()),
                        tools: names,
                    });
                    m.clients.push(c.client);
                }
            }
        }
        m
    }

    /// Per-server `(name, tool count | error)` in config order.
    pub fn statuses(&self) -> Vec<(String, Result<usize, String>)> {
        self.status
            .iter()
            .map(|s| (s.name.clone(), s.result.clone()))
            .collect()
    }

    pub fn server_statuses(&self) -> &[ServerStatus] {
        &self.status
    }

    /// All discovered tools, unfiltered.
    pub fn all_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .iter()
            .map(|t| t.clone() as Arc<dyn Tool>)
            .collect()
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name.clone()).collect()
    }

    /// Tools whose full names match any of the glob `selectors`; above `lazy_threshold`
    /// matches, `McpListTools` + `McpCall` are returned instead.
    pub fn tools_for(&self, selectors: &[String], lazy_threshold: usize) -> Vec<Arc<dyn Tool>> {
        let Some(set) = selector_set(selectors) else {
            return vec![];
        };
        let selected: Vec<Arc<McpTool>> = self
            .tools
            .iter()
            .filter(|t| set.is_match(&t.name))
            .cloned()
            .collect();
        if selected.is_empty() {
            return vec![];
        }
        if selected.len() <= lazy_threshold {
            selected.into_iter().map(|t| t as Arc<dyn Tool>).collect()
        } else {
            vec![
                Arc::new(McpListTools {
                    tools: selected.clone(),
                }),
                Arc::new(McpCall { tools: selected }),
            ]
        }
    }

    /// Stop all servers (also happens on drop).
    pub fn shutdown(&mut self) {
        self.tools.clear();
        for c in self.clients.drain(..) {
            c.cancellation_token().cancel();
        }
    }
}

impl Drop for McpManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn selector_set(patterns: &[String]) -> Option<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for p in patterns {
        match Glob::new(p) {
            Ok(g) => {
                b.add(g);
            }
            Err(e) => tracing::warn!("invalid mcp_tools selector `{p}`: {e}"),
        }
    }
    b.build().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(tool_name("my server", "do.it"), "mcp__my_server__do_it");
        assert_eq!(tool_name("a", &"x".repeat(100)).len(), 64);
    }

    #[test]
    fn mcp_json() {
        let m = parse_mcp_json(
            r#"{"mcpServers": {
              "a": {"command": "npx", "args": ["-y", "x"], "env": {"K": "v"}},
              "b": {"type": "http", "url": "http://h/mcp", "headers": {"Authorization": "Bearer t"}}
            }}"#,
        )
        .unwrap();
        assert_eq!(m["a"].transport(), Ok(TransportKind::Stdio));
        assert_eq!(m["a"].args, ["-y", "x"]);
        assert_eq!(m["a"].env["K"], "v");
        assert_eq!(m["b"].transport(), Ok(TransportKind::Http));
        assert_eq!(m["b"].headers["Authorization"], "Bearer t");
        assert!(parse_mcp_json("{}").unwrap().is_empty());
        assert!(McpServerConfig::default().transport().is_err());
    }

    #[test]
    fn acp_shapes() {
        let (n, c) = from_acp(&json!({"name": "s", "command": "/bin/x", "args": ["1"],
            "env": [{"name": "A", "value": "b"}]}))
        .unwrap();
        assert_eq!((n.as_str(), c.command.as_deref()), ("s", Some("/bin/x")));
        assert_eq!(c.env["A"], "b");
        let (_, c) = from_acp(&json!({"type": "http", "name": "h", "url": "http://u",
            "headers": [{"name": "X", "value": "y"}]}))
        .unwrap();
        assert_eq!(c.transport(), Ok(TransportKind::Http));
        assert_eq!(c.headers["X"], "y");
        assert!(from_acp(&json!({"type": "sse", "name": "h", "url": "u"})).is_none());
    }

    #[test]
    fn render() {
        let v = json!({"content": [{"type": "text", "text": "hi"},
            {"type": "image", "data": "x", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///a", "mimeType": "x/y", "blob": "AA=="}}]});
        assert_eq!(
            render_result(&v),
            "hi\n[image: image/png]\n[resource: file:///a (x/y)]"
        );
    }
}
