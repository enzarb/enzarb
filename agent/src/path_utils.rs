use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use axum::http::StatusCode;

use crate::init::home_dir;

/// Resolve a user-supplied path relative to $HOME, rejecting path traversal attacks.
/// Returns FORBIDDEN if the resolved path escapes the home directory.
///
/// Two layers of defense:
///  1. Lexical normalization rejects the obvious `..` escapes cheaply.
///  2. The deepest *existing* ancestor is canonicalized so that a symlink under
///     $HOME pointing outside it is caught here, rather than being silently
///     followed by the subsequent filesystem operation.
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

    // Resolve symlinks along the existing portion of the path and re-check
    // containment against the canonical home directory.
    let canonical_home = home.canonicalize().unwrap_or(home);
    let resolved = canonicalize_existing_prefix(&normalized);
    if !resolved.starts_with(&canonical_home) {
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

/// Canonicalize the deepest existing ancestor of `path` (resolving any
/// symlinks), then re-append the non-existent trailing components. For paths
/// that fully exist this is equivalent to `canonicalize`; for paths being
/// created (uploads, new files) it resolves the existing parent so a symlinked
/// parent directory cannot smuggle the target outside $HOME.
fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if let Ok(canonical) = current.canonicalize() {
            let mut resolved = canonical;
            for comp in tail.iter().rev() {
                resolved.push(comp);
            }
            return resolved;
        }
        match current.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                if !current.pop() {
                    return path.to_path_buf();
                }
            }
            // No existing ancestor could be canonicalized; fall back to the
            // lexically-normalized path (already containment-checked).
            None => return path.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn home() -> PathBuf {
        home_dir()
    }

    #[test]
    fn rejects_parent_traversal() {
        assert_eq!(
            resolve_safe("../etc/passwd").unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            resolve_safe("a/b/../../../etc").unwrap_err(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn absolute_is_reanchored_under_home() {
        // A leading slash is stripped and re-anchored under $HOME, so it stays contained.
        let resolved = resolve_safe("/foo/bar").unwrap();
        assert!(resolved.starts_with(home()));
    }

    #[test]
    fn allows_normal_path() {
        let resolved = resolve_safe(".enzarb/tasks").unwrap();
        assert!(resolved.starts_with(home()));
    }

    #[test]
    fn rejects_symlink_escape() {
        // Create a symlink under $HOME pointing outside it, then verify a path
        // through that symlink is rejected.
        let link = home().join(".test-escape-link");
        let _ = std::fs::remove_file(&link);
        if symlink("/tmp", &link).is_err() {
            // Environment without a writable $HOME; skip rather than fail.
            return;
        }
        let result = resolve_safe(".test-escape-link/secret");
        let _ = std::fs::remove_file(&link);
        assert_eq!(result.unwrap_err(), StatusCode::FORBIDDEN);
    }
}
