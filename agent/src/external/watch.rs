use axum::{
    extract::{Extension, Query, State},
    response::{IntoResponse, Response, Sse, sse::Event},
};
use inotify::{Inotify, WatchMask};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio_stream::wrappers::ReceiverStream;

use crate::AppState;
use crate::auth::ProjectPermissions;
use crate::path_utils::resolve_safe;

#[derive(Debug, Deserialize)]
pub struct WatchQuery {
    pub path: String,
}

pub async fn watch(
    State(_state): State<AppState>,
    Extension(perms): Extension<ProjectPermissions>,
    Query(q): Query<WatchQuery>,
) -> Response {
    if let Err(e) = perms.require("files:read") {
        return e.into_response();
    }

    let path = match resolve_safe(&q.path) {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };

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
