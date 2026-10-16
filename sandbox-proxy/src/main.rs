use std::{sync::Arc, time::Duration};

use anyhow::Result;
use enzarb_sandbox_proxy::{
    app,
    config::Config,
    credential::Keyring,
    kube_resolver::KubeResolver,
    rekey,
    store::{Credentials, PgStore},
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let keyring = Arc::new(load_keyring()?);
    let store = Arc::new(PgStore::from_env()?);
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => {}
        Some("rekey") => {
            let report = rekey::run(store.as_ref(), &keyring).await?;
            anyhow::ensure!(report.ok(), "rekey: {} row(s) failed", report.failed);
            return Ok(());
        }
        Some(other) => anyhow::bail!("unknown subcommand {other:?} (expected serve or rekey)"),
    }
    let chat_pool = store.pool();
    let creds = Arc::new(Credentials::new(store, keyring));
    let cfg = Config::from_env()?;
    let client = kube::Client::try_default().await?;
    let resolver = Arc::new(KubeResolver::start(client, cfg.secret_ttl).await?);
    let shutdown = CancellationToken::new();
    let listen = cfg.listen;
    let chats = Arc::new(enzarb_sandbox_proxy::chat::PgChats { pool: chat_pool });
    let state = app::AppState::new(cfg, resolver, creds, shutdown.clone()).with_chats(chats);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    tracing::info!(%listen, "listening");

    let sd = shutdown.clone();
    tokio::spawn(async move {
        wait_signal().await;
        tracing::info!("shutting down");
        sd.cancel();
    });
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await?;
    // Upgraded websocket tasks observe the cancellation and send close frames.
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}

/// The `enzarb-claude-key` keyring (`ENZARB_CLAUDE_KEYRING_DIR`). Required:
/// chat opens are refused without a stored token, so a missing keyring is a
/// deployment error, not a soft mode.
fn load_keyring() -> Result<Keyring> {
    let dir = std::env::var("ENZARB_CLAUDE_KEYRING_DIR")
        .unwrap_or_else(|_| "/etc/enzarb/claude-key".into());
    let k = Keyring::load(std::path::Path::new(&dir))?;
    tracing::info!(dir, ?k, "claude keyring loaded");
    Ok(k)
}

async fn wait_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("sigterm");
    tokio::select! {
        _ = term.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
    }
}
