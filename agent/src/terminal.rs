use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures::{SinkExt, StreamExt};
use portable_pty::PtySize;
use std::io::Write;

use crate::AppState;

#[derive(serde::Deserialize)]
struct Resize {
    rows: u16,
    cols: u16,
}

/// Attach a WebSocket to a running process's PTY or output stream.
///
/// Binary frames from the client are written to the process stdin (PTY).
/// JSON text frames with `{"rows":N,"cols":N}` resize the PTY.
/// Output from the process is sent to the client as binary frames.
pub async fn attach_ws(mut socket: WebSocket, process_id: String, state: AppState) {
    // 4004 tells the frontend the process is gone for good (killed, exited, or
    // never existed) so it stops retrying instead of looping forever against a
    // dead process — see CloseFrame below for the exited-in-flight case too.
    const PROCESS_GONE: u16 = 4004;

    let Some((scrollback, mut rx, input_writer, pty_master)) =
        state.process_store.attach(&process_id).await
    else {
        let _ = socket
            .send(Message::Close(Some(CloseFrame {
                code: PROCESS_GONE,
                reason: "process not found".into(),
            })))
            .await;
        return;
    };

    // Replay scrollback so reconnecting clients see all prior output.
    if !scrollback.is_empty()
        && socket
            .send(Message::Binary(scrollback.into()))
            .await
            .is_err()
    {
        return;
    }

    let (mut ws_tx, mut ws_rx) = socket.split();

    let output = async move {
        loop {
            match rx.recv().await {
                Ok(data) => {
                    if ws_tx.send(Message::Binary(data.into())).await.is_err() {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                // The process exited and its output channel was torn down —
                // tell the client explicitly so it doesn't retry against a
                // process that will never come back.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let _ = ws_tx
                        .send(Message::Close(Some(CloseFrame {
                            code: PROCESS_GONE,
                            reason: "process exited".into(),
                        })))
                        .await;
                    return;
                }
            }
        }
    };

    let input = async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Binary(b) => {
                    if let Some(ref writer) = input_writer
                        && let Ok(mut w) = writer.lock()
                    {
                        let _ = w.write_all(&b);
                        let _ = w.flush();
                    }
                }
                Message::Text(t) => {
                    if let Ok(r) = serde_json::from_str::<Resize>(&t)
                        && let Some(ref master) = pty_master
                        && let Ok(m) = master.lock()
                    {
                        let _ = m.resize(PtySize {
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

    tokio::select! {
        _ = output => {},
        _ = input => {},
    }
}
