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

    Ok(())
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
