use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::process::Command;

#[derive(Debug, Serialize, Deserialize)]
struct Tool {
    name: String,
    version: String,
}

pub async fn bootstrap(project_id: &str) -> Result<()> {
    let home = home_dir();
    let enzarb_dir = home.join(".enzarb");
    tokio::fs::create_dir_all(&enzarb_dir).await?;
    tokio::fs::create_dir_all(home.join(".enzarb/tasks")).await?;

    let mise_toml = home.join("mise.toml");
    if !mise_toml.exists() {
        tracing::info!(project_id, "first boot: generating mise.toml from ENZARB_TOOLS");
        write_mise_toml(&mise_toml).await?;
    }

    tracing::info!("running mise install");
    let status = Command::new("mise")
        .arg("install")
        .current_dir(&home)
        .status()
        .await?;

    if !status.success() {
        tracing::warn!("mise install exited with non-zero status");
    }

    Ok(())
}

async fn write_mise_toml(path: &PathBuf) -> Result<()> {
    let tools_json = std::env::var("ENZARB_TOOLS").unwrap_or_else(|_| "[]".to_string());
    let tools: Vec<Tool> = serde_json::from_str(&tools_json).unwrap_or_default();

    let mut content = String::from("[tools]\n");
    for tool in tools {
        let version = if tool.version.is_empty() || tool.version == "latest" {
            "latest".to_string()
        } else {
            format!("\"{}\"", tool.version)
        };
        content.push_str(&format!("{} = {}\n", tool.name, version));
    }

    tokio::fs::write(path, content).await?;
    Ok(())
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/user"))
}
