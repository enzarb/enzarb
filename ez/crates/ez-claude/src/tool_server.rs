//! In-process MCP server (Streamable HTTP, stateless JSON) that exposes ez's tools to the
//! claude-code child. Every call is forwarded to the agent loop over a channel, so ez's gates,
//! hooks and transcript apply exactly as they do under the native driver.

use anyhow::{Context, Result};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use ez_llm::ToolSpec;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::Value;
use std::sync::{Arc, RwLock};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// A tool call from the child, waiting for ez to run it.
#[derive(Debug)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
    pub reply: oneshot::Sender<ToolReply>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolReply {
    pub text: String,
    pub is_error: bool,
}

type Specs = Arc<RwLock<Vec<ToolSpec>>>;

#[derive(Clone)]
struct Handler {
    specs: Specs,
    calls: mpsc::UnboundedSender<ToolCall>,
}

fn to_mcp_tool(spec: &ToolSpec) -> Tool {
    let schema = match &spec.input_schema {
        Value::Object(o) => o.clone(),
        _ => serde_json::Map::new(),
    };
    Tool::new(spec.name.clone(), spec.description.clone(), schema)
}

impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = self.specs.read().unwrap().iter().map(to_mcp_tool).collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let (reply, rx) = oneshot::channel();
        let call = ToolCall {
            name: request.name.to_string(),
            arguments: Value::Object(request.arguments.unwrap_or_default()),
            reply,
        };
        let failed = |msg: &str| CallToolResult::error(vec![ContentBlock::text(msg)]).into();
        if self.calls.send(call).is_err() {
            return Ok(failed("ez is not accepting tool calls (session closed)"));
        }
        // Dropped without a reply: the turn ended or was cancelled before the tool ran.
        let Ok(r) = rx.await else {
            return Ok(failed("tool call cancelled"));
        };
        let content = vec![ContentBlock::text(r.text)];
        Ok(if r.is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        }
        .into())
    }
}

/// The running MCP endpoint; stops when dropped.
pub struct ToolServer {
    url: String,
    token: String,
    specs: Specs,
    ct: CancellationToken,
}

async fn require_bearer(State(token): State<Arc<str>>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == &*token);
    if ok {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

impl ToolServer {
    /// Bind `127.0.0.1:0` and serve `specs` at `/mcp` behind a fresh bearer token.
    pub async fn start(specs: Vec<ToolSpec>) -> Result<(Self, mpsc::UnboundedReceiver<ToolCall>)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let specs: Specs = Arc::new(RwLock::new(specs));
        let ct = CancellationToken::new();
        let config = StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .with_cancellation_token(ct.child_token());
        let handler = Handler {
            specs: specs.clone(),
            calls: tx,
        };
        let service: StreamableHttpService<Handler, LocalSessionManager> =
            StreamableHttpService::new(move || Ok(handler.clone()), Default::default(), config);
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let router = axum::Router::new().nest_service("/mcp", service).layer(
            middleware::from_fn_with_state(Arc::<str>::from(token.as_str()), require_bearer),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding the ez tool server")?;
        let addr = listener.local_addr()?;
        tokio::spawn({
            let ct = ct.clone();
            async move {
                let served = axum::serve(listener, router)
                    .with_graceful_shutdown(async move { ct.cancelled_owned().await })
                    .await;
                if let Err(e) = served {
                    tracing::warn!("ez tool server stopped: {e}");
                }
            }
        });
        let server = Self {
            url: format!("http://{addr}/mcp"),
            token,
            specs,
            ct,
        };
        Ok((server, rx))
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Replace the advertised tools (seen by the child's next `tools/list`).
    pub fn set_tools(&self, specs: Vec<ToolSpec>) {
        *self.specs.write().unwrap() = specs;
    }
}

impl Drop for ToolServer {
    fn drop(&mut self) {
        self.ct.cancel();
    }
}
