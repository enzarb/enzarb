//! Clone bookkeeping shared by steps and chat.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

/// Per-repository async locks: every git operation that changes a repo's
/// worktrees or refs (steps and chats) holds that repo's lock.
#[derive(Clone, Default)]
pub struct RepoLocks(Arc<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>);

impl RepoLocks {
    /// The lock of the repo at `dir`.
    pub fn get(&self, dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.0
            .lock()
            .unwrap()
            .entry(dir.to_path_buf())
            .or_default()
            .clone()
    }
}

/// Directory name of the clone of `url` under `<workspace>/repos/` (what chat
/// opens; steps keep it pointing at the latest task workspace of that repo).
pub fn repo_dir_name(url: &str) -> String {
    let tail: String = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':'])
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let hash = Sha256::digest(url.as_bytes());
    let hex: String = hash.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{tail}-{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_names_are_stable_and_safe() {
        let a = repo_dir_name("https://github.com/enzarb/enzarb.git");
        assert!(a.starts_with("enzarb-enzarb-"), "{a}");
        assert_eq!(a, repo_dir_name("https://github.com/enzarb/enzarb.git"));
        assert_ne!(a, repo_dir_name("https://github.com/enzarb/other"));
        assert!(
            repo_dir_name("file:///x/../y z")
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }
}
