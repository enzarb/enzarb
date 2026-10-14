use std::sync::Arc;

use anyhow::Context;
use sandbox_init::{
    AppState, Config, agent::AgentRunManager, capture::ClaudeTokenCapture, chat::ChatManager,
    dockerd::Dockerd, privs, reaper::Reaper, serve,
};
use tokio::signal::unix::{SignalKind, signal};

fn main() -> anyhow::Result<()> {
    // `sandbox-init --exec-as-sandbox-user prog args..`: PTY children drop root through us.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(privs::EXEC_FLAG) {
        privs::exec_mode(&args[2..]);
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    // Agents (chat in bypass mode in particular) run as uid 1000 while pid 1 is root: without
    // this they could read sandbox-init's memory and environment through /proc
    // (ptrace-style access is refused for a non-dumpable process).
    if let Err(e) = nix::sys::prctl::set_dumpable(false) {
        tracing::warn!(error = %e, "cannot mark sandbox-init non-dumpable");
    }
    let grace = config.grace;
    let reaper = Reaper::for_current_process();
    tokio::spawn(reaper.clone().run());

    // Install handlers before serving so an early SIGTERM is not lost.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("bind {}", config.listen))?;
    tracing::info!(
        addr = %config.listen,
        reap_all = reaper.reaps_all(),
        root = privs::is_root(),
        "sandbox-init listening"
    );
    // Docker is best-effort: a daemon that cannot start never blocks agents or readiness.
    let dockerd = Dockerd::new(config.dockerd.clone(), reaper.clone());
    if config.dockerd.enabled && !privs::is_root() {
        tracing::warn!("sandbox-init is not root: dockerd will probably fail to start");
    }
    dockerd.start();
    let runs = AgentRunManager::new(config.clone(), reaper.clone());
    runs.recover();
    let chats = ChatManager::new(config.clone(), reaper.clone(), runs.repo_locks());
    chats.recover().await;
    let capture = ClaudeTokenCapture::new(config.claude_token_file.clone());
    let state = Arc::new(
        AppState::new(config, reaper.clone())
            .with_run_manager(Arc::new(runs))
            .with_chats(chats.clone())
            .with_observer(Arc::new(capture))
            .with_dockerd(dockerd.clone()),
    );

    tokio::select! {
        r = serve(listener, state) => r.context("server failed")?,
        _ = sigterm.recv() => tracing::info!("SIGTERM received"),
        _ = sigint.recv() => tracing::info!("SIGINT received"),
    }

    // Chats first: `CLOSE shutdown` to each browser, then their agents stop.
    chats.shutdown(grace).await;
    // Then dockerd, so it stops its containers and releases its data root before the drain
    // completes (and before the blanket signal below hits containerd).
    dockerd.shutdown(grace).await;
    // Forward the signal to the children and give them the grace period.
    if !reaper.terminate_all(grace).await {
        tracing::warn!("children did not exit within the grace period; killed");
    }
    tracing::info!("bye");
    Ok(())
}
