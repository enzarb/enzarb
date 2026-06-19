use axum::{Json, extract::Path, http::StatusCode};
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::init::home_dir;

/// Run `mise <args...>` in the workspace home directory, capturing output.
/// Returns (success, stdout, stderr).
async fn run_mise(args: &[&str]) -> Result<(bool, String, String), std::io::Error> {
    let output = Command::new("mise")
        .args(args)
        .current_dir(home_dir())
        .output()
        .await?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

#[derive(Debug, Serialize)]
pub struct InstalledTool {
    pub name: String,
    /// Resolved version (e.g. "20.11.0"), when installed.
    pub version: Option<String>,
    /// Version requested in mise.toml (e.g. "latest", "20").
    pub requested: Option<String>,
    pub installed: bool,
    pub active: bool,
}

/// GET /tools — tools configured for this workspace (from mise.toml) and their
/// install state. Backed by `mise ls --current --json`.
pub async fn list() -> Result<Json<Vec<InstalledTool>>, (StatusCode, String)> {
    let (ok, stdout, stderr) = run_mise(&["ls", "--current", "--json"])
        .await
        .map_err(internal)?;
    if !ok {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, stderr));
    }

    // Output shape: { "<tool>": [ { version, requested_version, installed, active, ... }, ... ], ... }
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_default();
    let mut tools = Vec::new();
    if let Some(map) = parsed.as_object() {
        for (name, versions) in map {
            let entries = versions.as_array().cloned().unwrap_or_default();
            // Prefer the active entry, else the first.
            let entry = entries
                .iter()
                .find(|e| e.get("active").and_then(|v| v.as_bool()).unwrap_or(false))
                .or_else(|| entries.first());
            let Some(entry) = entry else { continue };
            tools.push(InstalledTool {
                name: name.clone(),
                version: entry
                    .get("version")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                requested: entry
                    .get("requested_version")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                installed: entry
                    .get("installed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                active: entry
                    .get("active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            });
        }
    }
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(tools))
}

#[derive(Debug, Deserialize)]
pub struct AddToolRequest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// POST /tools — add a tool to mise.toml and install it.
/// `mise use -y <name>@<version>` writes the config and installs in one step.
pub async fn add(Json(req): Json<AddToolRequest>) -> Result<StatusCode, (StatusCode, String)> {
    let name = req.name.trim();
    if name.is_empty() || !valid_tool_name(name) {
        return Err((StatusCode::BAD_REQUEST, "invalid tool name".into()));
    }
    let version = req
        .version
        .as_deref()
        .filter(|v| !v.is_empty())
        .unwrap_or("latest");
    let spec = format!("{name}@{version}");

    let (ok, _stdout, stderr) = run_mise(&["use", "-y", &spec]).await.map_err(internal)?;
    if !ok {
        tracing::warn!(tool = %spec, %stderr, "mise use failed");
        return Err((StatusCode::BAD_REQUEST, stderr));
    }
    tracing::info!(tool = %spec, "installed tool via mise");
    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /tools/{name} — remove a tool from mise.toml and uninstall it.
pub async fn remove(Path(name): Path<String>) -> Result<StatusCode, (StatusCode, String)> {
    if !valid_tool_name(&name) {
        return Err((StatusCode::BAD_REQUEST, "invalid tool name".into()));
    }
    // Remove from config (authoritative). Failure here is fatal.
    let (ok, _out, stderr) = run_mise(&["use", "-y", "--rm", &name])
        .await
        .map_err(internal)?;
    if !ok {
        tracing::warn!(tool = %name, %stderr, "mise use --rm failed");
        return Err((StatusCode::BAD_REQUEST, stderr));
    }
    // Best-effort: prune the now-unused tool's installed versions to reclaim space.
    if let Ok((false, _, e)) = run_mise(&["prune", "-y", &name]).await {
        tracing::debug!(tool = %name, stderr = %e, "mise prune (non-fatal)");
    }
    tracing::info!(tool = %name, "removed tool via mise");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct RegistryTool {
    /// Short name used with `mise use` (e.g. "node").
    pub short: String,
    /// Backend/full identifier (e.g. "core:node", "aqua:foo/bar").
    pub full: String,
}

/// GET /tools/registry — the catalog of tools mise can install, for UI lookup.
/// Backed by `mise registry` (whitespace-separated columns: short, full…).
pub async fn registry() -> Result<Json<Vec<RegistryTool>>, (StatusCode, String)> {
    let (ok, stdout, stderr) = run_mise(&["registry"]).await.map_err(internal)?;
    if !ok {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, stderr));
    }
    let tools = stdout
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let short = cols.next()?.to_string();
            let full = cols.next().unwrap_or(&short).to_string();
            Some(RegistryTool { short, full })
        })
        .collect();
    Ok(Json(tools))
}

/// GET /tools/{name}/versions — available versions for a tool, newest last.
/// Backed by `mise ls-remote <name>`.
pub async fn versions(Path(name): Path<String>) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    if !valid_tool_name(&name) {
        return Err((StatusCode::BAD_REQUEST, "invalid tool name".into()));
    }
    let (ok, stdout, stderr) = run_mise(&["ls-remote", &name]).await.map_err(internal)?;
    if !ok {
        return Err((StatusCode::BAD_REQUEST, stderr));
    }
    let versions = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Ok(Json(versions))
}

/// Guard against shell/arg injection in tool names passed to mise. Tool names
/// (incl. backends) use alphanumerics and a small set of separators.
fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '+'))
}

fn internal(e: std::io::Error) -> (StatusCode, String) {
    tracing::error!(error = %e, "failed to run mise");
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
