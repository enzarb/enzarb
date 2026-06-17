use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::sse::Event,
    response::{IntoResponse, Response, Sse},
};
use futures::stream::Stream;
use inotify::{Inotify, WatchMask};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;

use crate::AppState;
use crate::init::home_dir;

#[derive(Debug, Deserialize)]
pub struct WatchQuery {
    pub path: String,
}

pub async fn watch(State(_state): State<AppState>, Query(q): Query<WatchQuery>) -> Response {
    let path = resolve_watch_path(&q.path);

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(64);

    tokio::spawn(async move {
        if let Err(e) = watch_path(path, tx).await {
            tracing::warn!(error = %e, "inotify watch error");
        }
    });

    let stream = ReceiverStream::new(rx);
    Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        )
        .into_response()
}

async fn watch_path(
    path: PathBuf,
    tx: tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
) -> anyhow::Result<()> {
    let mut inotify = Inotify::init()?;
    inotify.watches().add(
        &path,
        WatchMask::CREATE
            | WatchMask::DELETE
            | WatchMask::MODIFY
            | WatchMask::MOVED_FROM
            | WatchMask::MOVED_TO,
    )?;

    let mut buffer = [0u8; 4096];
    loop {
        let events = inotify.read_events_blocking(&mut buffer)?;
        for event in events {
            let name = event
                .name
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            let kind = if event.mask.contains(inotify::EventMask::CREATE) {
                "create"
            } else if event.mask.contains(inotify::EventMask::DELETE) {
                "delete"
            } else if event.mask.contains(inotify::EventMask::MODIFY) {
                "modify"
            } else if event.mask.contains(inotify::EventMask::MOVED_FROM) {
                "moved_from"
            } else if event.mask.contains(inotify::EventMask::MOVED_TO) {
                "moved_to"
            } else {
                "unknown"
            };

            let data = serde_json::json!({ "kind": kind, "name": name });
            let sse_event = Event::default().event(kind).data(data.to_string());

            if tx.send(Ok(sse_event)).await.is_err() {
                return Ok(()); // client disconnected
            }
        }
    }
}

fn resolve_watch_path(path: &str) -> PathBuf {
    let home = home_dir();
    if path.is_empty() || path == "/" {
        return home;
    }
    let stripped = path.trim_start_matches('/');
    home.join(stripped)
}
