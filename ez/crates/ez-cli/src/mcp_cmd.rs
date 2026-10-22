//! `ez mcp list`.

use anyhow::Result;
use ez_core::Config;
use ez_mcp::McpManager;

use crate::Cli;

/// Connect to all configured MCP servers (user + project config, `.mcp.json`) and print them.
pub async fn list(cli: &Cli) -> Result<()> {
    let cwd = cli.project_dir()?;
    let config = Config::load(&cwd, None)?;
    if config.mcp_servers.is_empty() {
        println!("no MCP servers configured in {}", cwd.display());
        return Ok(());
    }
    let manager = McpManager::start(config.mcp_servers.clone(), &cwd).await;
    for s in manager.server_statuses() {
        match &s.result {
            Ok(n) => {
                println!("{}: ok ({n} tools)", s.name);
                for t in &s.tools {
                    println!("  {t}");
                }
            }
            Err(e) => println!("{}: FAILED: {}", s.name, e.replace('\n', "\n  ")),
        }
    }
    Ok(())
}
