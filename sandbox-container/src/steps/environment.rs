//! Tells environment failures (disk full, OOM, missing toolchain, network, load) from code
//! failures of a step.
//!
//! Runs on the failure path only, from `finalize`. The evidence is collected while the step runs
//! (the cgroup `oom_kill` counter and PSI before it, the child's end and stderr after).
//!
//! Everything is conservative: a false negative is a plain failure, a false positive parks a real
//! code failure in a platform wait. So toolchain/network are matched only in the phases that fail
//! before the step's own command runs, and load needs PSI saturated at the start and at the kill.

use std::io;
use std::path::Path;

use enzarb_proto::{EnvironmentKind, StepEnvironment};

/// CPU / IO `some avg10` at or above this (percent) is saturated.
const PSI_SOME_SATURATED: f64 = 80.0;
/// Memory `full avg10` at or above this (percent) is saturated.
const PSI_MEMORY_FULL_SATURATED: f64 = 20.0;

/// The phase that failed before the step's own command ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Git prepare of the workspaces.
    Fetch,
    /// `mise install` of the tools.
    Install,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Phase::Fetch => "git fetch",
            Phase::Install => "mise install",
        }
    }
}

/// PSI `avg10` readings (percent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pressure {
    pub cpu_some: f64,
    pub io_some: f64,
    pub memory_full: f64,
}

impl Pressure {
    /// The first saturated resource: its name, signal and reading.
    fn saturated(&self) -> Option<(&'static str, &'static str, f64)> {
        if self.cpu_some >= PSI_SOME_SATURATED {
            Some(("cpu", "some", self.cpu_some))
        } else if self.io_some >= PSI_SOME_SATURATED {
            Some(("io", "some", self.io_some))
        } else if self.memory_full >= PSI_MEMORY_FULL_SATURATED {
            Some(("memory", "full", self.memory_full))
        } else {
            None
        }
    }

    fn reading(&self, resource: &str) -> f64 {
        match resource {
            "cpu" => self.cpu_some,
            "io" => self.io_some,
            _ => self.memory_full,
        }
    }
}

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
    /// Set when the failure came before the step's own command ran.
    pub phase: Option<Phase>,
    /// We ended it on its timeout (as opposed to a cancel).
    pub timed_out: bool,
    /// PSI before the command was spawned / when the timeout fired.
    pub pressure_start: Option<Pressure>,
    pub pressure_kill: Option<Pressure>,
}

const NETWORK_SIGNATURES: &[&str] = &[
    "could not resolve host",
    "temporary failure in name resolution",
    "network is unreachable",
    "connection timed out",
    "failed to connect to",
];

const TOOLCHAIN_SIGNATURES: &[&str] = &[
    "command not found",
    "failed to install",
    "no version is set for",
];

/// The first line of `text` holding one of `sigs`, trimmed.
fn signature_line(text: &str, sigs: &[&str]) -> Option<String> {
    text.lines().find_map(|l| {
        let low = l.to_ascii_lowercase();
        sigs.iter().any(|s| low.contains(s)).then(|| {
            let l = l.trim();
            match l.char_indices().nth(200) {
                Some((i, _)) => format!("{}…", &l[..i]),
                None => l.to_string(),
            }
        })
    })
}

fn classify_phase(ev: &Evidence) -> Option<StepEnvironment> {
    let phase = ev.phase?;
    let (kind, line) = if let Some(l) = signature_line(&ev.text, NETWORK_SIGNATURES) {
        (EnvironmentKind::Network, l)
    } else {
        (
            EnvironmentKind::ToolchainMissing,
            signature_line(&ev.text, TOOLCHAIN_SIGNATURES)?,
        )
    };
    Some(StepEnvironment {
        kind,
        detail: format!("{line} ({})", phase.label()),
    })
}

/// Saturated PSI both at the start and at the kill of a step we killed on its timeout.
fn classify_load(ev: &Evidence) -> Option<StepEnvironment> {
    if !(ev.we_killed && ev.timed_out) {
        return None;
    }
    let (start, kill) = (ev.pressure_start?, ev.pressure_kill?);
    let (res, sig, at_kill) = kill.saturated()?;
    // The resource reported must be saturated at both reads.
    let at_start = start.reading(res);
    let limit = if res == "memory" {
        PSI_MEMORY_FULL_SATURATED
    } else {
        PSI_SOME_SATURATED
    };
    if at_start < limit {
        return None;
    }
    Some(StepEnvironment {
        kind: EnvironmentKind::Load,
        detail: format!(
            "{res} pressure {sig} avg10={at_kill:.1}% at timeout ({at_start:.1}% at step start)"
        ),
    })
}

/// `some` / `full` `avg10` of a PSI file.
#[derive(Debug, PartialEq)]
pub struct PsiLine {
    pub some_avg10: f64,
    pub full_avg10: Option<f64>,
}

pub fn parse_psi(s: &str) -> Option<PsiLine> {
    let avg10 = |kind: &str| -> Option<f64> {
        s.lines().find_map(|l| {
            let mut it = l.split_whitespace();
            if it.next()? != kind {
                return None;
            }
            it.find_map(|kv| kv.strip_prefix("avg10=")?.parse().ok())
        })
    };
    Some(PsiLine {
        some_avg10: avg10("some")?,
        full_avg10: avg10("full"),
    })
}

/// PSI from `<root>/{cpu,io,memory}`; `None` when any is missing or unparsable.
pub fn read_pressure(root: &Path) -> Option<Pressure> {
    let read = |n: &str| parse_psi(&std::fs::read_to_string(root.join(n)).ok()?);
    Some(Pressure {
        cpu_some: read("cpu")?.some_avg10,
        io_some: read("io")?.some_avg10,
        memory_full: read("memory")?.full_avg10?,
    })
}

/// PSI of this system.
pub fn system_pressure() -> Option<Pressure> {
    read_pressure(Path::new("/proc/pressure"))
}

/// `Some` for a failed step whose cause is the environment. Disk wins, then toolchain/network,
/// then load (timeouts) or OOM.
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
    if let Some(e) = classify_phase(ev) {
        return Some(e);
    }
    if ev.we_killed {
        return classify_load(ev);
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

    const HI: Pressure = Pressure {
        cpu_some: 82.1,
        io_some: 0.0,
        memory_full: 0.0,
    };
    const LO: Pressure = Pressure {
        cpu_some: 1.0,
        io_some: 2.0,
        memory_full: 0.0,
    };

    fn timeout(start: Option<Pressure>, kill: Option<Pressure>) -> Evidence {
        Evidence {
            we_killed: true,
            timed_out: true,
            pressure_start: start,
            pressure_kill: kill,
            ..ev()
        }
    }

    fn phased(phase: Phase, text: &str) -> Evidence {
        Evidence {
            phase: Some(phase),
            text: text.into(),
            ..ev()
        }
    }

    #[test]
    fn fetch_dns_failure_is_network() {
        let e = phased(
            Phase::Fetch,
            "fatal: unable to access: Could not resolve host: github.com",
        );
        let c = classify(&e).unwrap();
        assert_eq!(c.kind, EnvironmentKind::Network);
        assert!(c.detail.contains("Could not resolve host: github.com"));
        assert!(c.detail.ends_with("(git fetch)"));
    }

    #[test]
    fn install_command_not_found_is_toolchain() {
        let e = phased(Phase::Install, "mise ERROR\nsh: 1: node: command not found");
        let c = classify(&e).unwrap();
        assert_eq!(c.kind, EnvironmentKind::ToolchainMissing);
        assert!(c.detail.contains("command not found"));
        assert!(c.detail.contains("mise install"));
    }

    #[test]
    fn long_signature_line_is_trimmed() {
        let e = phased(
            Phase::Fetch,
            &format!("network is unreachable {}", "x".repeat(500)),
        );
        assert!(classify(&e).unwrap().detail.chars().count() < 230);
    }

    #[test]
    fn step_phase_output_is_never_matched() {
        for t in [
            "could not resolve host: example.com",
            "connection refused",
            "sh: x: command not found",
            "network is unreachable",
        ] {
            let mut e = ev();
            e.text = t.into();
            assert_eq!(kind(&e), None, "{t}");
        }
    }

    #[test]
    fn phase_failure_without_signature_is_plain() {
        assert_eq!(
            kind(&phased(Phase::Fetch, "fatal: repository not found")),
            None
        );
        assert_eq!(kind(&phased(Phase::Install, "boom")), None);
        let mut e = phased(Phase::Fetch, "could not resolve host");
        e.failed = false;
        assert_eq!(kind(&e), None);
    }

    #[test]
    fn load_needs_saturation_at_start_and_kill() {
        let c = classify(&timeout(Some(HI), Some(HI))).unwrap();
        assert_eq!(c.kind, EnvironmentKind::Load);
        assert!(c.detail.contains("cpu pressure some avg10=82.1%"));
        assert!(c.detail.contains("at step start"));
        let io = Pressure {
            cpu_some: 0.0,
            io_some: 90.0,
            memory_full: 0.0,
        };
        let c = classify(&timeout(Some(io), Some(io))).unwrap();
        assert!(c.detail.contains("io pressure some avg10=90.0%"));
        let mem = Pressure {
            cpu_some: 0.0,
            io_some: 0.0,
            memory_full: 25.0,
        };
        let c = classify(&timeout(Some(mem), Some(mem))).unwrap();
        assert!(c.detail.contains("memory pressure full avg10=25.0%"));
    }

    #[test]
    fn load_negatives() {
        assert_eq!(kind(&timeout(Some(LO), Some(LO))), None);
        assert_eq!(kind(&timeout(Some(LO), Some(HI))), None);
        assert_eq!(kind(&timeout(Some(HI), Some(LO))), None);
        assert_eq!(kind(&timeout(None, Some(HI))), None);
        assert_eq!(kind(&timeout(Some(HI), None)), None);
        // Saturated at both reads, but by different resources.
        let io = Pressure {
            cpu_some: 0.0,
            io_some: 90.0,
            memory_full: 0.0,
        };
        assert_eq!(kind(&timeout(Some(io), Some(HI))), None);
        // A cancel is not a timeout.
        let mut e = timeout(Some(HI), Some(HI));
        e.timed_out = false;
        assert_eq!(kind(&e), None);
        // Not killed by us.
        let mut e = timeout(Some(HI), Some(HI));
        e.we_killed = false;
        e.timed_out = false;
        assert_eq!(kind(&e), None);
        assert_eq!(kind(&ev()), None);
    }

    #[test]
    fn enospc_beats_network_and_load() {
        let mut e = phased(
            Phase::Fetch,
            "Could not resolve host\nNo space left on device",
        );
        assert_eq!(kind(&e), Some(EnvironmentKind::DiskFull));
        e = timeout(Some(HI), Some(HI));
        e.text = "write: No space left on device".into();
        assert_eq!(kind(&e), Some(EnvironmentKind::DiskFull));
        e.text.clear();
        e.workspace_full = Some("workspace has 0 free blocks (statvfs)".into());
        assert_eq!(kind(&e), Some(EnvironmentKind::DiskFull));
    }

    #[test]
    fn oom_is_skipped_when_we_killed() {
        let mut e = timeout(Some(LO), Some(LO));
        e.oom_before = Some(0);
        e.oom_after = Some(1);
        assert_eq!(kind(&e), None);
    }

    #[test]
    fn parses_psi() {
        let cpu = "some avg10=1.23 avg60=0.50 avg300=0.10 total=12345\n";
        assert_eq!(
            parse_psi(cpu),
            Some(PsiLine {
                some_avg10: 1.23,
                full_avg10: None
            })
        );
        let mem = "some avg10=3.00 avg60=1.00 avg300=0.00 total=1\nfull avg10=21.50 avg60=0.00 avg300=0.00 total=2\n";
        assert_eq!(
            parse_psi(mem),
            Some(PsiLine {
                some_avg10: 3.0,
                full_avg10: Some(21.5)
            })
        );
        assert_eq!(parse_psi(""), None);
        assert_eq!(parse_psi("garbage\nsome avg10=x"), None);
        assert_eq!(parse_psi("full avg10=1.0\n"), None);
    }

    #[test]
    fn reads_pressure_dir() {
        let d = std::env::temp_dir().join(format!("psi-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(read_pressure(&d), None);
        std::fs::write(d.join("cpu"), "some avg10=1.5 avg60=0 avg300=0 total=0\n").unwrap();
        std::fs::write(
            d.join("io"),
            "some avg10=2.5 avg60=0 avg300=0 total=0\nfull avg10=0.00 avg60=0 avg300=0 total=0\n",
        )
        .unwrap();
        assert_eq!(read_pressure(&d), None, "memory missing");
        std::fs::write(
            d.join("memory"),
            "some avg10=0.0 avg60=0 avg300=0 total=0\nfull avg10=4.0 avg60=0 avg300=0 total=0\n",
        )
        .unwrap();
        assert_eq!(
            read_pressure(&d),
            Some(Pressure {
                cpu_some: 1.5,
                io_some: 2.5,
                memory_full: 4.0
            })
        );
        std::fs::remove_dir_all(&d).unwrap();
    }
}
