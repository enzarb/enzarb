//! Tells environment failures (disk full, OOM) from code failures of a step.
//!
//! Runs on the failure path only, from `finalize`. The evidence is collected while the step runs
//! (the cgroup `oom_kill` counter before it, the child's end and stderr after).

use std::io;
use std::path::Path;

use enzarb_proto::{EnvironmentKind, StepEnvironment};

/// What the step left behind.
#[derive(Debug, Default, Clone)]
pub struct Evidence {
    /// The step failed (not success, not interrupted).
    pub failed: bool,
    /// We ended it ourselves (cancel or timeout).
    pub we_killed: bool,
    /// Failure text: the error, stderr tail.
    pub text: String,
    /// The child died of SIGKILL (exit 137).
    pub sigkilled: bool,
    /// `oom_kill` in `memory.events` before / after the step, when readable.
    pub oom_before: Option<u64>,
    pub oom_after: Option<u64>,
    /// Why the workspace looks full (statvfs), when it does.
    pub workspace_full: Option<String>,
}

/// `Some` for a failed step whose cause is the environment. Disk wins over OOM.
pub fn classify(ev: &Evidence) -> Option<StepEnvironment> {
    if !ev.failed {
        return None;
    }
    let disk = |detail: String| StepEnvironment {
        kind: EnvironmentKind::DiskFull,
        detail,
    };
    if ev
        .text
        .to_ascii_lowercase()
        .contains("no space left on device")
    {
        return Some(disk(
            "No space left on device (ENOSPC) in the step output".into(),
        ));
    }
    if let Some(d) = &ev.workspace_full {
        return Some(disk(d.clone()));
    }
    if ev.we_killed {
        return None;
    }
    let (before, after) = (ev.oom_before?, ev.oom_after?);
    if after > before {
        let how = if ev.sigkilled {
            ", command killed (137)"
        } else {
            ""
        };
        return Some(StepEnvironment {
            kind: EnvironmentKind::Oom,
            detail: format!("oom_kill {before}→{after} in memory.events{how}"),
        });
    }
    None
}

/// The `oom_kill` counter of a cgroup v2 `memory.events` file.
pub fn oom_kill_count(path: &Path) -> Option<u64> {
    parse_oom_kill(&std::fs::read_to_string(path).ok()?)
}

fn parse_oom_kill(s: &str) -> Option<u64> {
    s.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        if it.next()? == "oom_kill" {
            it.next()?.parse().ok()
        } else {
            None
        }
    })
}

/// Why `dir`'s filesystem has no space left, if it has none.
pub fn workspace_full(dir: &Path) -> Option<String> {
    let st = nix::sys::statvfs::statvfs(dir).ok()?;
    full_reason(
        (st.blocks() as u64, st.blocks_available() as u64),
        (st.files() as u64, st.files_available() as u64),
    )
}

/// Filesystems that report no totals (some report 0 inodes) are never "full".
fn full_reason(blocks: (u64, u64), files: (u64, u64)) -> Option<String> {
    if blocks.0 > 0 && blocks.1 == 0 {
        Some("workspace has 0 free blocks (statvfs)".into())
    } else if files.0 > 0 && files.1 == 0 {
        Some("workspace has 0 free inodes (statvfs)".into())
    } else {
        None
    }
}

/// ENOSPC (or `StorageFull`).
pub fn is_enospc(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::StorageFull || e.raw_os_error() == Some(nix::libc::ENOSPC)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev() -> Evidence {
        Evidence {
            failed: true,
            ..Evidence::default()
        }
    }

    fn kind(e: &Evidence) -> Option<EnvironmentKind> {
        classify(e).map(|x| x.kind)
    }

    #[test]
    fn parses_memory_events() {
        let s = "low 0\nhigh 0\nmax 3\noom 2\noom_kill 1\noom_group_kill 0\n";
        assert_eq!(parse_oom_kill(s), Some(1));
        assert_eq!(parse_oom_kill("max 1\n"), None);
        assert_eq!(parse_oom_kill("oom_kill x\n"), None);
    }

    #[test]
    fn enospc_text_is_disk_full() {
        let mut e = ev();
        e.text = "git: error: No space left on device".into();
        assert_eq!(kind(&e), Some(EnvironmentKind::DiskFull));
        assert!(!classify(&e).unwrap().detail.is_empty());
    }

    #[test]
    fn enospc_io_error() {
        assert!(is_enospc(&io::Error::from_raw_os_error(nix::libc::ENOSPC)));
        assert!(!is_enospc(&io::Error::from_raw_os_error(nix::libc::EROFS)));
    }

    #[test]
    fn statvfs_reasons() {
        assert!(full_reason((10, 0), (10, 5)).is_some());
        assert!(full_reason((10, 5), (10, 0)).is_some());
        assert!(full_reason((10, 5), (0, 0)).is_none());
        assert!(full_reason((0, 0), (0, 0)).is_none());
    }

    #[test]
    fn oom_needs_counter_delta() {
        let mut e = ev();
        e.oom_before = Some(0);
        e.oom_after = Some(0);
        e.sigkilled = true;
        assert_eq!(kind(&e), None, "bare SIGKILL is not OOM");
        e.oom_after = Some(1);
        assert_eq!(kind(&e), Some(EnvironmentKind::Oom));
    }

    #[test]
    fn disk_beats_oom() {
        let mut e = ev();
        e.oom_before = Some(0);
        e.oom_after = Some(1);
        e.workspace_full = Some("workspace has 0 free blocks (statvfs)".into());
        assert_eq!(kind(&e), Some(EnvironmentKind::DiskFull));
    }

    #[test]
    fn our_kill_success_and_unreadable_counter_are_not_oom() {
        let mut e = ev();
        e.oom_before = Some(0);
        e.oom_after = Some(1);
        e.we_killed = true;
        assert_eq!(kind(&e), None);
        e.we_killed = false;
        e.failed = false;
        assert_eq!(kind(&e), None);
        e.failed = true;
        e.oom_before = None;
        assert_eq!(kind(&e), None);
    }
}
