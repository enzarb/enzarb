use std::path::{Component, Path, PathBuf};

use axum::http::StatusCode;

use crate::init::home_dir;

/// Resolve a user-supplied path relative to $HOME, rejecting path traversal attacks.
/// Returns FORBIDDEN if the resolved path escapes the home directory.
pub fn resolve_safe(path: &str) -> Result<PathBuf, StatusCode> {
    let home = home_dir();
    let joined = if path.starts_with('/') {
        let stripped = path.trim_start_matches('/');
        home.join(stripped)
    } else {
        home.join(path)
    };

    let normalized = normalize_path(&joined);
    if !normalized.starts_with(&home) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(normalized)
}

pub fn normalize_path(path: &Path) -> PathBuf {
    let mut components = vec![];
    for c in path.components() {
        match c {
            Component::ParentDir => {
                components.pop();
            }
            Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}
