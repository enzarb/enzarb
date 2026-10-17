use std::sync::Arc;

use anyhow::Context;
use sandbox_init::{
    AppState, Config,
    capture::ClaudeTokenCapture,
    chat::ChatManager,
    docker_prune::{ActivityGate, Pruner},
    dockerd::Dockerd,
    mcp, privs,
    reaper::Reaper,
    repos::RepoLocks,
    serve,
    steps::StepManager,
};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

fn main() -> anyhow::Result<()> {
    // `sandbox-init --exec-as-sandbox-user prog args..`: PTY children drop root through us.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(privs::EXEC_FLAG) {
        privs::exec_mode(&args[2..]);
    }
    // `sandbox-init mcp-submit-result <schema> <out>`: the agent's submit_result tool.
    if args.get(1).map(String::as_str) == Some(mcp::SUBCOMMAND) {
        mcp::main(&args[2..]);
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
    // Prunes Docker data at startup, after runs and chats end, and when space runs low.
    let gate = ActivityGate::default();
    let pruner = Pruner::new(
        config.docker_prune.clone(),
        dockerd.subscribe(),
        gate.clone(),
        reaper.clone(),
    );
    let prune_stop = CancellationToken::new();
    let prune_task = pruner.spawn(prune_stop.clone());
    let locks = RepoLocks::default();
    let steps =
        StepManager::new(config.clone(), reaper.clone(), locks.clone()).with_activity(gate.clone());
    steps.recover();
    let chats = ChatManager::with_activity(config.clone(), reaper.clone(), locks, gate);
    chats.set_sink(steps.sink());
    chats.recover().await;
    let capture = ClaudeTokenCapture::new(config.claude_token_file.clone());
    let state = Arc::new(
        AppState::new(config, reaper.clone())
            .with_steps(steps.clone())
            .with_chats(chats.clone())
            .with_observer(Arc::new(capture))
            .with_dockerd(dockerd.clone())
            .with_pruner(pruner),
    );

    tokio::select! {
        r = serve(listener, state) => r.context("server failed")?,
        _ = sigterm.recv() => tracing::info!("SIGTERM received"),
        _ = sigint.recv() => tracing::info!("SIGINT received"),
    }

    // Steps first: each running step is interrupted at a checkpoint and stays resumable
    // (the engine resumes it after the restart), instead of being killed into a failure.
    steps.interrupt_all(grace).await;
    // Chats next: `CLOSE shutdown` to each browser, then their agents stop.
    chats.shutdown(grace).await;
    prune_stop.cancel();
    let _ = prune_task.await;
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
