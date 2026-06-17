use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio_util::io::ReaderStream;

use crate::AppState;
use crate::init::home_dir;

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: String, // "file" | "dir" | "symlink"
    pub size: Option<u64>,
    pub modified: Option<String>,
}

pub async fn list(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> Result<Json<Vec<FileEntry>>, StatusCode> {
    let abs = resolve_safe(&q.path)?;

    let mut entries = vec![];
    let mut read_dir = tokio::fs::read_dir(&abs).await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let meta = entry.metadata().await.ok();
        let kind = if let Some(m) = &meta {
            if m.is_dir() { "dir" } else if m.is_symlink() { "symlink" } else { "file" }
        } else {
            "unknown"
        };
        let rel_path = entry.path()
            .strip_prefix(home_dir())
            .unwrap_or(&entry.path())
            .to_string_lossy()
            .to_string();

        entries.push(FileEntry {
            name: entry.file_name().to_string_lossy().to_string(),
            path: rel_path,
            kind: kind.to_string(),
            size: meta.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
            modified: meta.and_then(|m| m.modified().ok())
                .and_then(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339().into()),
        });
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(entries))
}

pub async fn download(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> Response {
    let abs = match resolve_safe(&q.path) {
        Ok(p) => p,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let file = match tokio::fs::File::open(&abs).await {
        Ok(f) => f,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let filename = abs.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "download".to_string());

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (
        [
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", filename)),
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        ],
        body,
    ).into_response()
}

pub async fn upload(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
    body: Body,
) -> Response {
    let abs = match resolve_safe(&q.path) {
        Ok(p) => p,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    if let Some(parent) = abs.parent() {
        if let Err(_) = tokio::fs::create_dir_all(parent).await {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    use tokio::io::AsyncWriteExt;
    let mut file = match tokio::fs::File::create(&abs).await {
        Ok(f) => f,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    use futures::StreamExt;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(data) => {
                if file.write_all(&data).await.is_err() {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        }
    }

    StatusCode::CREATED.into_response()
}

pub async fn delete(
    State(_state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> StatusCode {
    let abs = match resolve_safe(&q.path) {
        Ok(p) => p,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    let meta = match tokio::fs::metadata(&abs).await {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND,
    };

    let result = if meta.is_dir() {
        tokio::fs::remove_dir_all(&abs).await
    } else {
        tokio::fs::remove_file(&abs).await
    };

    match result {
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Resolve path relative to $HOME, rejecting path traversal attacks.
fn resolve_safe(path: &str) -> Result<PathBuf, StatusCode> {
    let home = home_dir();
    let joined = if path.starts_with('/') {
        // Allow absolute paths only within home
        let stripped = path.trim_start_matches('/');
        home.join(stripped)
    } else {
        home.join(path)
    };

    // Canonicalize to catch `..` traversal — use lexical normalization since file may not exist yet
    let normalized = normalize_path(&joined);
    if !normalized.starts_with(&home) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(normalized)
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = vec![];
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => { components.pop(); }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}
