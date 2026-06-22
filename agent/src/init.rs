use anyhow::Result;
use std::path::PathBuf;
use tokio::process::Command;

pub async fn bootstrap() -> Result<()> {
    let home = home_dir();
    let enzarb_dir = home.join(".enzarb");
    tokio::fs::create_dir_all(&enzarb_dir).await?;
    tokio::fs::create_dir_all(home.join(".enzarb/tasks")).await?;

    let mise_toml = home.join("mise.toml");
    // Earlier agents hand-wrote mise.toml with unquoted versions (`tool = latest`),
    // which newer mise rejects as invalid TOML. Detect that specific malformation
    // and back it up; tools can be re-added via the /tools API.
    let malformed = match tokio::fs::read_to_string(&mise_toml).await {
        Ok(c) => c.lines().any(|l| l.trim().ends_with("= latest")),
        Err(_) => false,
    };
    if malformed {
        tracing::warn!("mise.toml has unquoted versions; backing up — re-add tools via /tools API");
        let _ = tokio::fs::rename(&mise_toml, home.join("mise.toml.bak")).await;
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

    setup_buildx().await;
    setup_git().await;

    Ok(())
}

/// Set the project's git identity and clone the repo on first boot. The git
/// credential helper and docker credential helper are registered in the
/// workspace image (credential-free), so this only handles the per-project,
/// runtime bits. Best-effort.
async fn setup_git() {
    let Ok(remote) = std::env::var("ENZARB_GIT_REMOTE") else {
        tracing::debug!("ENZARB_GIT_REMOTE unset; skipping git setup");
        return;
    };
    let Some(host) = remote
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
    else {
        tracing::warn!("ENZARB_GIT_REMOTE has no host: {remote}");
        return;
    };

    let slug = std::env::var("ENZARB_PROJECT_SLUG").unwrap_or_else(|_| "project".to_string());
    let git = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        async move { Command::new("git").args(&args).status().await }
    };

    let _ = git(&["config", "--global", "user.name", &slug]).await;
    let email = format!("{slug}@workspaces.{host}");
    let _ = git(&["config", "--global", "user.email", &email]).await;

    // Clone the repo on first boot if the working copy isn't there yet.
    let dest = home_dir().join(&slug);
    if !dest.join(".git").exists() {
        match Command::new("git")
            .args(["clone", &remote, &dest.to_string_lossy()])
            .status()
            .await
        {
            Ok(s) if s.success() => tracing::info!("cloned project repo to {}", dest.display()),
            Ok(s) => tracing::warn!("git clone exited with status {s}"),
            Err(e) => tracing::warn!("git clone failed: {e}"),
        }
    }
}

/// Register the buildkitd sidecar as the default `docker buildx` builder so
/// `docker build`/`docker buildx build` use it out of the box. Best-effort:
/// failures (e.g. no docker CLI, sidecar not ready yet) are logged, not fatal.
async fn setup_buildx() {
    let Ok(addr) = std::env::var("BUILDKIT_HOST") else {
        tracing::debug!("BUILDKIT_HOST unset; skipping buildx setup");
        return;
    };

    // Idempotent: `create` fails if the builder already exists, so check first.
    let exists = Command::new("docker")
        .args(["buildx", "inspect", "enzarb"])
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);

    let result = if exists {
        Command::new("docker")
            .args(["buildx", "use", "enzarb"])
            .status()
            .await
    } else {
        Command::new("docker")
            .args([
                "buildx", "create", "--name", "enzarb", "--driver", "remote", "--use", &addr,
            ])
            .status()
            .await
    };

    match result {
        Ok(s) if s.success() => tracing::info!("default buildx builder -> {addr}"),
        Ok(s) => tracing::warn!("buildx setup exited with status {s}"),
        Err(e) => tracing::warn!("buildx setup failed: {e}"),
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/home/user"))
}
