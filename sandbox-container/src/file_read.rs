//! `file_read` -> `file`: read one UTF-8 text file of the workspace.
//!
//! A relative path is resolved against `base` (the worktree of the chat open
//! on the requesting mux session) when there is one, else against the
//! workspace root. The path is resolved (`..`, symlinks) and must stay under the workspace
//! root; the size is capped at [`MAX_FILE_READ_BYTES`]. Failures are reported
//! in the reply (`code`), never as a session error.

use std::io::Read;
use std::path::{Path, PathBuf};

use enzarb_proto::{Ctrl, MAX_FILE_READ_BYTES, file_error_code as code};

fn fail(id: String, path: String, c: &str, message: &str) -> Ctrl {
    Ctrl::File {
        id,
        path,
        content: None,
        code: Some(c.to_string()),
        message: Some(message.to_string()),
    }
}

/// Resolve `path` (relative: against `base`, else `root`); it must stay under `root`.
fn resolve(root: &Path, base: Option<&Path>, path: &str) -> Result<PathBuf, &'static str> {
    let root = root.canonicalize().map_err(|_| code::IO)?;
    let base = match base {
        Some(b) => b.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                code::NOT_FOUND
            } else {
                code::IO
            }
        })?,
        None => root.clone(),
    };
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    // Canonicalising follows symlinks, so a link out of the workspace is caught.
    let real = joined.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            // A missing path whose lexical form escapes is still an escape.
            if lexically_escapes(&root, &joined) {
                code::OUTSIDE_WORKSPACE
            } else {
                code::NOT_FOUND
            }
        } else {
            code::IO
        }
    })?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(code::OUTSIDE_WORKSPACE)
    }
}

fn lexically_escapes(root: &Path, p: &Path) -> bool {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    !out.starts_with(root)
}

/// Answer one `file_read`. Blocking: run it on a blocking thread.
pub fn read(root: &Path, base: Option<&Path>, id: String, path: String) -> Ctrl {
    let real = match resolve(root, base, &path) {
        Ok(r) => r,
        Err(c) => return fail(id, path, c, "cannot read that path"),
    };
    let file = match std::fs::File::open(&real) {
        Ok(f) => f,
        Err(_) => return fail(id, path, code::IO, "cannot open file"),
    };
    match file.metadata() {
        Ok(m) if !m.is_file() => return fail(id, path, code::NOT_A_FILE, "not a regular file"),
        Ok(m) if m.len() > MAX_FILE_READ_BYTES => {
            return fail(
                id,
                path,
                code::TOO_LARGE,
                "file is larger than the read limit",
            );
        }
        Ok(_) => {}
        Err(_) => return fail(id, path, code::IO, "cannot stat file"),
    }
    // The size can grow after the stat: read at most one byte over the cap.
    let mut buf = Vec::new();
    if file
        .take(MAX_FILE_READ_BYTES + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        return fail(id, path, code::IO, "read failed");
    }
    if buf.len() as u64 > MAX_FILE_READ_BYTES {
        return fail(
            id,
            path,
            code::TOO_LARGE,
            "file is larger than the read limit",
        );
    }
    match String::from_utf8(buf) {
        Ok(content) => Ctrl::File {
            id,
            path,
            content: Some(content),
            code: None,
            message: None,
        },
        Err(_) => fail(id, path, code::NOT_UTF8, "file is not UTF-8 text"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(c: Ctrl) -> String {
        match c {
            Ctrl::File {
                code: Some(c),
                content: None,
                ..
            } => c,
            other => panic!("expected an error reply, got {other:?}"),
        }
    }

    fn root() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("ws/.ez")).unwrap();
        std::fs::write(d.path().join("ws/.ez/workflows.toml"), "a = 1\n").unwrap();
        std::fs::write(d.path().join("secret"), "x").unwrap();
        d
    }

    #[test]
    fn reads_relative_and_absolute() {
        let d = root();
        let ws = d.path().join("ws");
        for p in [
            ".ez/workflows.toml".to_string(),
            ws.join(".ez/workflows.toml").display().to_string(),
        ] {
            match read(&ws, None, "i".into(), p) {
                Ctrl::File {
                    content: Some(c),
                    id,
                    ..
                } => {
                    assert_eq!(c, "a = 1\n");
                    assert_eq!(id, "i");
                }
                o => panic!("{o:?}"),
            }
        }
    }

    #[test]
    fn refuses_escapes() {
        let d = root();
        let ws = d.path().join("ws");
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "../secret".into())),
            "outside_workspace"
        );
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "../nope".into())),
            "outside_workspace"
        );
        let abs = d.path().join("secret").display().to_string();
        assert_eq!(
            code_of(read(&ws, None, "i".into(), abs)),
            "outside_workspace"
        );
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "/etc/passwd".into())),
            "outside_workspace"
        );
    }

    #[test]
    fn relative_paths_use_the_chat_worktree() {
        let d = root();
        let ws = d.path().join("ws");
        let wt = ws.join("worktrees/chat-1");
        std::fs::create_dir_all(wt.join(".ez")).unwrap();
        std::fs::write(wt.join(".ez/workflows.toml"), "b = 2\n").unwrap();
        let get = |base: Option<&Path>, p: &str| match read(&ws, base, "i".into(), p.into()) {
            Ctrl::File {
                content: Some(c), ..
            } => c,
            o => panic!("{o:?}"),
        };
        assert_eq!(get(Some(&wt), ".ez/workflows.toml"), "b = 2\n");
        assert_eq!(get(None, ".ez/workflows.toml"), "a = 1\n");
        // Absolute paths ignore the base.
        let abs = ws.join(".ez/workflows.toml").display().to_string();
        assert_eq!(get(Some(&wt), &abs), "a = 1\n");
        // A missing file in the worktree is not_found, not the workspace's.
        assert_eq!(
            code_of(read(&ws, Some(&wt), "i".into(), ".ez/modes.toml".into())),
            "not_found"
        );
        // Leaving the worktree is fine while it stays in the workspace;
        // leaving the workspace is refused.
        assert_eq!(get(Some(&wt), "../../.ez/workflows.toml"), "a = 1\n");
        assert_eq!(
            code_of(read(&ws, Some(&wt), "i".into(), "../../../secret".into())),
            "outside_workspace"
        );
        // A worktree that is gone: not_found.
        let gone = ws.join("worktrees/chat-2");
        assert_eq!(
            code_of(read(&ws, Some(&gone), "i".into(), ".ez/modes.toml".into())),
            "not_found"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_out() {
        let d = root();
        let ws = d.path().join("ws");
        std::os::unix::fs::symlink(d.path().join("secret"), ws.join("link")).unwrap();
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "link".into())),
            "outside_workspace"
        );
    }

    #[test]
    fn size_cap_and_kinds() {
        let d = root();
        let ws = d.path().join("ws");
        std::fs::write(ws.join("ok"), vec![b'a'; MAX_FILE_READ_BYTES as usize]).unwrap();
        std::fs::write(ws.join("big"), vec![b'a'; MAX_FILE_READ_BYTES as usize + 1]).unwrap();
        std::fs::write(ws.join("bin"), [0xff, 0xfe]).unwrap();
        assert!(matches!(
            read(&ws, None, "i".into(), "ok".into()),
            Ctrl::File {
                content: Some(_),
                ..
            }
        ));
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "big".into())),
            "too_large"
        );
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "bin".into())),
            "not_utf8"
        );
        assert_eq!(
            code_of(read(&ws, None, "i".into(), ".ez".into())),
            "not_a_file"
        );
        assert_eq!(
            code_of(read(&ws, None, "i".into(), "missing".into())),
            "not_found"
        );
    }
}
