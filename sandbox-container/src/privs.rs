//! Privilege handling (ADR-0016).
//!
//! sandbox-init runs as root in the sandbox container so it can supervise `dockerd`
//! ([`crate::dockerd`]). Everything else it starts (agents, git, `mise`, PTY shells) must run as
//! the sandbox user, uid/gid 1000. Every such spawn goes through [`as_sandbox_user`] (or, for PTYs,
//! [`wrap_pty_command`]); only the dockerd supervisor spawns as root. The `spawn_sites_use_the_privilege_drop`
//! test fails if a new `Command::new` appears elsewhere.
//!
//! When sandbox-init is not root (tests, a plain `docker run --user 1000`), all of this is a no-op,
//! so behaviour is unchanged.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use nix::unistd::{Gid, Uid};

/// uid and gid of the sandbox user (`enzarb` in the image; the operator's `SANDBOX_UID`).
pub const SANDBOX_ID: u32 = 1000;
/// Hidden first argument of sandbox-init: drop privileges, then exec the rest of the arguments.
pub const EXEC_FLAG: &str = "--exec-as-sandbox-user";
const HOME: &str = "/home/enzarb";

pub fn is_root() -> bool {
    Uid::effective().is_root()
}

/// Whether children must be dropped to the sandbox user.
fn drops() -> bool {
    is_root()
}

/// In a forked child, before `exec`: drop to the sandbox user for good and forbid regaining
/// privileges through setuid/file-capability binaries.
///
/// Order matters: supplementary groups and gid go first, while we are still allowed to.
pub fn drop_privileges() -> io::Result<()> {
    let gid = Gid::from_raw(SANDBOX_ID);
    let uid = Uid::from_raw(SANDBOX_ID);
    nix::unistd::setgroups(&[gid]).map_err(io::Error::from)?;
    nix::unistd::setresgid(gid, gid, gid).map_err(io::Error::from)?;
    nix::unistd::setresuid(uid, uid, uid).map_err(io::Error::from)?;
    nix::sys::prctl::set_no_new_privs().map_err(io::Error::from)?;
    // Verify: root must not be recoverable.
    if nix::unistd::setuid(Uid::from_raw(0)).is_ok() {
        return Err(io::Error::other("privilege drop did not stick"));
    }
    Ok(())
}

/// Make `cmd` run as the sandbox user when we are root. Sets `HOME`, `USER` and `LOGNAME`
/// (a uid change does not update them); call it before adding the command's own environment so
/// that wins. `PATH` is untouched: the image's `ENV PATH` is inherited unchanged.
pub fn as_sandbox_user(cmd: &mut Command) {
    if !drops() {
        return;
    }
    cmd.env("HOME", HOME)
        .env("USER", "enzarb")
        .env("LOGNAME", "enzarb");
    // SAFETY: only async-signal-safe syscalls (setgroups/setres[ug]id/prctl) run in the closure.
    unsafe {
        cmd.pre_exec(drop_privileges);
    }
}

/// For `portable-pty`, which has no `pre_exec` hook: run `command` through sandbox-init itself in
/// [`EXEC_FLAG`] mode, which drops privileges and execs it. Returns `command` unchanged when not
/// root.
pub fn wrap_pty_command(command: Vec<String>) -> Vec<String> {
    if !drops() {
        return command;
    }
    let exe =
        std::env::current_exe().unwrap_or_else(|_| PathBuf::from("/usr/local/bin/sandbox-init"));
    let mut out = vec![exe.to_string_lossy().into_owned(), EXEC_FLAG.to_string()];
    out.extend(command);
    out
}

/// Environment for PTY children when privileges are dropped (see [`as_sandbox_user`]).
pub fn pty_env() -> Vec<(&'static str, &'static str)> {
    if drops() {
        vec![("HOME", HOME), ("USER", "enzarb"), ("LOGNAME", "enzarb")]
    } else {
        Vec::new()
    }
}

/// `sandbox-init --exec-as-sandbox-user prog args...`: drop privileges, then exec. Never returns.
pub fn exec_mode(args: &[String]) -> ! {
    let Some((prog, rest)) = args.split_first() else {
        eprintln!("sandbox-init {EXEC_FLAG}: no command");
        std::process::exit(2);
    };
    if is_root()
        && let Err(e) = drop_privileges()
    {
        eprintln!("sandbox-init: cannot drop privileges: {e}");
        std::process::exit(126);
    }
    let err = Command::new(prog).args(rest).exec();
    eprintln!("sandbox-init: cannot run {prog}: {err}");
    std::process::exit(127);
}

/// Give a file or directory sandbox-init just created to the sandbox user (no-op unless root).
/// Does not follow symlinks.
pub fn own(path: &Path) -> io::Result<()> {
    if !is_root() {
        return Ok(());
    }
    nix::unistd::fchownat(
        nix::fcntl::AT_FDCWD,
        path,
        Some(Uid::from_raw(SANDBOX_ID)),
        Some(Gid::from_raw(SANDBOX_ID)),
        nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
    )
    .map_err(io::Error::from)
}

/// `create_dir_all` that gives every directory it creates to the sandbox user.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    create_dir_all_mode(path, None)
}

/// [`create_dir_all`] with `mode` for the new directories.
pub fn create_dir_all_mode(path: &Path, mode: Option<u32>) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut missing = Vec::new();
    let mut p = Some(path);
    while let Some(q) = p {
        if q.as_os_str().is_empty() || q.exists() {
            break;
        }
        missing.push(q);
        p = q.parent();
    }
    for q in missing.into_iter().rev() {
        let mut b = std::fs::DirBuilder::new();
        if let Some(m) = mode {
            b.mode(m);
        }
        match b.create(q) {
            Ok(()) => own(q)?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn create_dir_all_makes_nested_dirs_with_mode() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a/b/c");
        create_dir_all_mode(&p, Some(0o700)).unwrap();
        assert!(p.is_dir());
        let m = std::fs::metadata(t.path().join("a")).unwrap();
        assert_eq!(m.mode() & 0o777, 0o700);
        // Idempotent.
        create_dir_all(&p).unwrap();
        // The root itself already exists: untouched.
        create_dir_all(t.path()).unwrap();
    }

    #[test]
    fn nothing_changes_when_not_root() {
        if is_root() {
            return;
        }
        let mut c = Command::new("true");
        as_sandbox_user(&mut c);
        assert_eq!(c.get_envs().count(), 0);
        let cmd = vec!["bash".to_string(), "-l".to_string()];
        assert_eq!(wrap_pty_command(cmd.clone()), cmd);
        assert!(pty_env().is_empty());
        let f = tempfile::NamedTempFile::new().unwrap();
        own(f.path()).unwrap();
    }

    /// Needs root (the CI image smoke covers the same path inside the container).
    #[test]
    fn dropped_child_is_uid_1000_and_cannot_come_back() {
        if !is_root() {
            eprintln!("skipped: not root");
            return;
        }
        let mut c = Command::new("sh");
        c.args([
            "-c",
            "echo $(id -u):$(id -g):$(id -G):$HOME; grep NoNewPrivs /proc/self/status",
        ]);
        as_sandbox_user(&mut c);
        let out = c.output().unwrap();
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("1000:1000:1000:/home/enzarb"), "{s}");
        assert!(s.contains("NoNewPrivs:\t1"), "{s}");
    }

    /// Guards the invariant of the module doc: every child is spawned through the helper.
    #[test]
    fn spawn_sites_use_the_privilege_drop() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // Files allowed to spawn directly, and why.
        let allowed = [
            "dockerd.rs", // the one root child
            "privs.rs",   // the helper itself
            "reaper.rs",  // tests only
        ];
        let mut stack = vec![src];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                if p.extension().is_none_or(|x| x != "rs") || allowed.contains(&name.as_str()) {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                // Only the code above the test module counts.
                let code = text.split("#[cfg(test)]").next().unwrap();
                if code.contains("Command::new(") || code.contains("CommandBuilder::new(") {
                    assert!(
                        code.contains("as_sandbox_user(") || code.contains("wrap_pty_command("),
                        "{} spawns a process without privs::as_sandbox_user / wrap_pty_command",
                        p.display()
                    );
                }
            }
        }
    }
}
