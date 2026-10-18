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

/// Shared Cargo build directory of the repo at `url`:
/// `<workspace>/.cache/target/<owner>/<repo>`. Every task worktree of one repo builds into it
/// instead of its own `target/`. Components are sanitised like [`repo_dir_name`], so they
/// cannot escape. `None` when the URL has no owner and repo part.
pub fn shared_target_dir(workspace: &Path, url: &str) -> Option<PathBuf> {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect()
    };
    let mut parts = url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit(['/', ':']);
    let repo = clean(parts.next()?);
    let owner = clean(parts.next()?);
    if repo.is_empty() || owner.is_empty() {
        return None;
    }
    Some(workspace.join(".cache/target").join(owner).join(repo))
}

/// `CARGO_TARGET_DIR` entry for a step or chat on `url`, creating the directory (as the sandbox
/// user) so Cargo never meets root-owned parents. `None` when the variable is already set in
/// `env` or the image, or the URL does not parse.
pub fn target_dir_env(
    workspace: &Path,
    url: Option<&str>,
    env: &[(String, String)],
) -> Option<(String, String)> {
    if env.iter().any(|(k, _)| k == "CARGO_TARGET_DIR")
        || std::env::var_os("CARGO_TARGET_DIR").is_some()
    {
        return None;
    }
    let dir = shared_target_dir(workspace, url?)?;
    if let Err(e) = crate::privs::create_dir_all(&dir) {
        tracing::warn!(path = %dir.display(), error = %e, "cannot create the shared target dir");
        return None;
    }
    Some((
        "CARGO_TARGET_DIR".into(),
        dir.to_string_lossy().into_owned(),
    ))
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

    #[test]
    fn shared_target_dirs_per_repo() {
        let ws = Path::new("/workspace");
        let d = |u| shared_target_dir(ws, u);
        let want = Some(PathBuf::from("/workspace/.cache/target/enzarb/enzarb"));
        assert_eq!(d("https://github.com/enzarb/enzarb.git"), want);
        assert_eq!(d("https://github.com/enzarb/enzarb/"), want);
        assert_eq!(d("git@github.com:enzarb/enzarb.git"), want);
        assert_ne!(d("https://github.com/enzarb/other"), want);
        assert_eq!(d("nonsense"), None);
        assert_eq!(d(""), None);
        let h = d("https://x/../a b").unwrap();
        assert!(h.starts_with("/workspace/.cache/target/"), "{h:?}");
        assert!(!h.components().any(|c| c.as_os_str() == ".."), "{h:?}");
    }
}
