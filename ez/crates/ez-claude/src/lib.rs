//! The `claude-code` driver: ez's prompts, modes, tools and hooks on top of Claude Code's model
//! loop (ADR-0028). Claude Code runs as claude-agent-acp, a child of ez; ez serves every tool to
//! it over a loopback MCP endpoint so one ez configuration controls both drivers.

mod driver;
mod tool_server;

pub use driver::{
    COMMAND_ENV, ClaudeDriver, DriverConfig, SERVER_NAME, mcp_name, session_new_params,
};
pub use tool_server::{ToolCall, ToolReply, ToolServer};
