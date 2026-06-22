use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::AppState;
use crate::tmux::TMUX_SESSION;

#[derive(serde::Deserialize)]
struct Resize {
    rows: u16,
    cols: u16,
}

/// Bridge a WebSocket to a tmux window through a real PTY.
///
/// The client sends keystrokes as binary frames (written straight to the PTY,
/// no per-keystroke process spawn) and terminal size as JSON text frames. We run
/// `tmux` in the PTY attached to a per-connection grouped session, so each client
/// can view its own window and the PTY size drives the window size.
pub async fn handle_ws(mut socket: WebSocket, window_name: String, _state: AppState) {
    // A grouped session (shares windows with the main session) gives this client
    // an independent current-window and size, so concurrent terminals don't fight
    // over the active window.
    let client_session = format!(
        "{TMUX_SESSION}-ws-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    // Target the window in the grouped session, not the main session — otherwise
    // every new connection calls select-window on `enzarb:...` and the last one
    // wins, making all clients land on the same window.
    let window_target = format!("{}:{}", client_session, window_name);

    let pair = match native_pty_system().openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "openpty failed");
            let _ = socket.close().await;
            return;
        }
    };

    let mut cmd = CommandBuilder::new("tmux");
    cmd.args([
        "new-session",
        "-s",
        client_session.as_str(),
        "-t",
        TMUX_SESSION,
        ";",
        "set-option",
        "status",
        "off",
        ";",
        "select-window",
        "-t",
        window_target.as_str(),
    ]);
    for (k, v) in std::env::vars() {
        cmd.env(k, v);
    }
    cmd.env("TERM", "xterm-256color");
    cmd.cwd(crate::init::home_dir().into_os_string());

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "tmux attach spawn failed");
            let _ = socket.close().await;
            return;
        }
    };
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "pty reader");
            return;
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "pty writer");
            return;
        }
    };
    let master = pair.master;

    let (mut ws_tx, mut ws_rx) = socket.split();

    // PTY output -> channel -> WebSocket. The PTY read is blocking, so it lives
    // on a dedicated thread that hands chunks to the async side.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // WebSocket input -> channel -> blocking PTY writer thread.
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(bytes) = in_rx.recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    let output = async move {
        while let Some(chunk) = out_rx.recv().await {
            if ws_tx.send(Message::Binary(chunk.into())).await.is_err() {
                break;
            }
        }
    };

    let input = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(b) => {
                    if in_tx.send(b.to_vec()).is_err() {
                        break;
                    }
                }
                Message::Text(t) => {
                    if let Ok(r) = serde_json::from_str::<Resize>(&t) {
                        let _ = master.resize(PtySize {
                            rows: r.rows,
                            cols: r.cols,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    };

    // Whichever side ends first, tear the other down by dropping it.
    tokio::select! {
        _ = output => {},
        _ = input => {},
    }

    let _ = child.kill();
    // The grouped session lingers after detach; remove it.
    let _ = Command::new("tmux")
        .args(["kill-session", "-t", &client_session])
        .status()
        .await;
}
