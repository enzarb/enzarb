use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use tokio::process::Command;

use crate::AppState;

/// Bridge a WebSocket connection to a tmux window's pty.
/// The client sends keystrokes as binary/text frames; we forward to tmux via `send-keys`.
/// Output is streamed back by attaching to the tmux pipe.
pub async fn handle_ws(mut socket: WebSocket, window_name: String, state: AppState) {
    let target = state.process_store.window_target(&window_name);

    // Start `tmux pipe-pane` to capture output into a FIFO
    let fifo_path = format!("/tmp/enzarb-{}.fifo", &window_name);
    let _ = tokio::fs::remove_file(&fifo_path).await;

    let _ = nix::unistd::mkfifo(
        fifo_path.as_str(),
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    );

    let pipe_target = target.clone();
    let pipe_path = fifo_path.clone();
    let _ = Command::new("tmux")
        .args([
            "pipe-pane",
            "-t",
            &pipe_target,
            "-o",
            &format!("cat >> {}", pipe_path),
        ])
        .status()
        .await;

    // Open FIFO for reading (non-blocking via tokio)
    let fifo = match tokio::fs::OpenOptions::new()
        .read(true)
        .open(&fifo_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(error = %e, "failed to open fifo");
            let _ = socket.close().await;
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut fifo_reader = tokio::io::BufReader::new(fifo);

    let send_target = target.clone();
    // Task: read from WebSocket, send keystrokes to tmux
    let input_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            let keys = match msg {
                Message::Text(t) => t.to_string(),
                Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                Message::Close(_) => break,
                _ => continue,
            };
            let _ = Command::new("tmux")
                .args(["send-keys", "-t", &send_target, &keys, ""])
                .status()
                .await;
        }
    });

    // Task: read from FIFO, send to WebSocket
    let output_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 4096];
        loop {
            match fifo_reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let _ = tokio::join!(input_task, output_task);

    // Cleanup: stop pipe-pane
    let _ = Command::new("tmux")
        .args(["pipe-pane", "-t", &target])
        .status()
        .await;
    let _ = tokio::fs::remove_file(&fifo_path).await;
}
