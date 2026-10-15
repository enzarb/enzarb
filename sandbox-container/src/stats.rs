//! On-demand sandbox stats snapshot (`stats_request` -> `stats`).
//!
//! Raw cgroup v2 counters (`cpu.stat`, `memory.current`, `io.stat`) and used
//! storage (statvfs) with a timestamp. Nothing is sampled in the background:
//! every request reads the files once. A missing or oddly formatted file makes
//! its fields absent; it is never an error.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use enzarb_proto::SandboxStats;

/// Where a snapshot reads from. Faked in tests.
pub trait StatsSource {
    /// Contents of a cgroup file relative to the cgroup root, `None` when it
    /// cannot be read.
    fn read_file(&self, rel: &str) -> Option<String>;
    /// `(used, total)` bytes of the workspace filesystem, `None` when unknown.
    fn statfs_used(&self) -> Option<(u64, u64)>;
    /// Unix time in milliseconds.
    fn now_ms(&self) -> u64;
}

/// The real source: cgroup files under `cgroup_root`, statvfs of `data_path`.
#[derive(Debug, Clone)]
pub struct SystemSource {
    pub cgroup_root: PathBuf,
    pub data_path: PathBuf,
}

impl StatsSource for SystemSource {
    fn read_file(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.cgroup_root.join(rel)).ok()
    }

    fn statfs_used(&self) -> Option<(u64, u64)> {
        let s = nix::sys::statvfs::statvfs(&self.data_path).ok()?;
        // u64 on the 64-bit Linux targets the image is built for.
        let frsize: u64 = s.fragment_size();
        let total = s.blocks().checked_mul(frsize)?;
        let free = s.blocks_free().checked_mul(frsize)?;
        Some((total.saturating_sub(free), total))
    }

    fn now_ms(&self) -> u64 {
        unix_ms()
    }
}

/// Unix time in milliseconds (0 if the clock is before the epoch).
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// `usage_usec`, `user_usec` and `system_usec` of `cpu.stat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuStat {
    pub usage_usec: Option<u64>,
    pub user_usec: Option<u64>,
    pub system_usec: Option<u64>,
}

/// Parse `cpu.stat` (`key value` per line). Unknown keys, blank lines and
/// non-numeric values are skipped; the first valid occurrence of a key wins.
pub fn parse_cpu_stat(text: &str) -> CpuStat {
    let mut out = CpuStat::default();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(val)) = (it.next(), it.next()) else {
            continue;
        };
        let slot = match key {
            "usage_usec" => &mut out.usage_usec,
            "user_usec" => &mut out.user_usec,
            "system_usec" => &mut out.system_usec,
            _ => continue,
        };
        if slot.is_none() {
            *slot = val.parse().ok();
        }
    }
    out
}

/// Parse `memory.current` (one number). `max` or anything else is `None`.
pub fn parse_memory_current(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// Parse `io.stat` into `(read_bytes, write_bytes)`, each summed over all
/// device lines (`8:0 rbytes=1 wbytes=2 rios=3 ...`). A side is `None` when no
/// line carried a valid value for it; unparsable tokens are ignored.
pub fn parse_io_stat(text: &str) -> (Option<u64>, Option<u64>) {
    let (mut r, mut w): (Option<u64>, Option<u64>) = (None, None);
    let add = |acc: &mut Option<u64>, v: u64| *acc = Some(acc.unwrap_or(0).saturating_add(v));
    for line in text.lines() {
        // The first token is the device (`major:minor`); the rest are `key=value`.
        for tok in line.split_whitespace().skip(1) {
            let Some((k, v)) = tok.split_once('=') else {
                continue;
            };
            let Ok(v) = v.parse::<u64>() else { continue };
            match k {
                "rbytes" => add(&mut r, v),
                "wbytes" => add(&mut w, v),
                _ => {}
            }
        }
    }
    (r, w)
}

/// Read one snapshot. Never fails: whatever cannot be read is absent.
pub fn snapshot(src: &dyn StatsSource) -> SandboxStats {
    let cpu = src
        .read_file("cpu.stat")
        .map(|t| parse_cpu_stat(&t))
        .unwrap_or_default();
    let (io_read_bytes, io_write_bytes) = src
        .read_file("io.stat")
        .map(|t| parse_io_stat(&t))
        .unwrap_or_default();
    let storage = src.statfs_used();
    SandboxStats {
        ts_ms: src.now_ms(),
        cpu_usage_usec: cpu.usage_usec,
        cpu_user_usec: cpu.user_usec,
        cpu_system_usec: cpu.system_usec,
        memory_bytes: src
            .read_file("memory.current")
            .and_then(|t| parse_memory_current(&t)),
        io_read_bytes,
        io_write_bytes,
        storage_used_bytes: storage.map(|(used, _)| used),
        storage_total_bytes: storage.map(|(_, total)| total),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Fake {
        files: HashMap<&'static str, &'static str>,
        statfs: Option<(u64, u64)>,
    }

    impl StatsSource for Fake {
        fn read_file(&self, rel: &str) -> Option<String> {
            self.files.get(rel).map(|s| s.to_string())
        }
        fn statfs_used(&self) -> Option<(u64, u64)> {
            self.statfs
        }
        fn now_ms(&self) -> u64 {
            42
        }
    }

    const CPU: &str = "usage_usec 1000\nuser_usec 700\nsystem_usec 300\nnr_periods 0\nnr_throttled 0\nthrottled_usec 0\n";

    #[test]
    fn cpu_stat_typical() {
        let c = parse_cpu_stat(CPU);
        assert_eq!(c.usage_usec, Some(1000));
        assert_eq!(c.user_usec, Some(700));
        assert_eq!(c.system_usec, Some(300));
    }

    #[test]
    fn cpu_stat_empty_missing_and_garbage() {
        assert_eq!(parse_cpu_stat(""), CpuStat::default());
        let c = parse_cpu_stat("usage_usec 5\n\n  \nuser_usec abc\nsystem_usec\nbogus\n");
        assert_eq!(c.usage_usec, Some(5));
        assert_eq!(c.user_usec, None);
        assert_eq!(c.system_usec, None);
        assert_eq!(
            parse_cpu_stat("usage_usec -1\nuser_usec 1.5\n"),
            CpuStat::default()
        );
    }

    #[test]
    fn memory_current_forms() {
        assert_eq!(parse_memory_current("12345\n"), Some(12345));
        assert_eq!(parse_memory_current("  7  "), Some(7));
        assert_eq!(parse_memory_current("max\n"), None);
        assert_eq!(parse_memory_current(""), None);
        assert_eq!(parse_memory_current("12 34"), None);
    }

    #[test]
    fn io_stat_sums_devices() {
        let t = "8:0 rbytes=100 wbytes=200 rios=1 wios=2 dbytes=0 dios=0\n\
                 259:0 rbytes=1 wbytes=2 rios=1 wios=1 dbytes=0 dios=0\n";
        assert_eq!(parse_io_stat(t), (Some(101), Some(202)));
    }

    #[test]
    fn io_stat_missing_and_odd() {
        assert_eq!(parse_io_stat(""), (None, None));
        // One device lacks rbytes; a bad token and a bare word are ignored.
        let t = "8:0 wbytes=5 junk wbytes2=9 rbytes=xx\n\n259:0 rbytes=3 wbytes=4\n";
        assert_eq!(parse_io_stat(t), (Some(3), Some(9)));
        assert_eq!(parse_io_stat("8:0 rios=1\n"), (None, None));
        assert_eq!(
            parse_io_stat("a rbytes=18446744073709551615\nb rbytes=5\n").0,
            Some(u64::MAX)
        );
    }

    #[test]
    fn snapshot_all_present() {
        let mut f = Fake::default();
        f.files.insert("cpu.stat", CPU);
        f.files.insert("memory.current", "2048\n");
        f.files.insert("io.stat", "8:0 rbytes=10 wbytes=20\n");
        f.statfs = Some((30, 100));
        assert_eq!(
            snapshot(&f),
            SandboxStats {
                ts_ms: 42,
                cpu_usage_usec: Some(1000),
                cpu_user_usec: Some(700),
                cpu_system_usec: Some(300),
                memory_bytes: Some(2048),
                io_read_bytes: Some(10),
                io_write_bytes: Some(20),
                storage_used_bytes: Some(30),
                storage_total_bytes: Some(100),
            }
        );
    }

    #[test]
    fn snapshot_some_absent() {
        let mut f = Fake::default();
        f.files.insert("memory.current", "max\n");
        f.files.insert("cpu.stat", "usage_usec 9\n");
        f.statfs = Some((1, 2));
        let s = snapshot(&f);
        assert_eq!(s.cpu_usage_usec, Some(9));
        assert_eq!(s.cpu_user_usec, None);
        assert_eq!(s.memory_bytes, None);
        assert_eq!(s.io_read_bytes, None);
        assert_eq!(s.storage_total_bytes, Some(2));
    }

    #[test]
    fn snapshot_all_absent_still_has_timestamp() {
        let s = snapshot(&Fake::default());
        assert_eq!(
            s,
            SandboxStats {
                ts_ms: 42,
                ..Default::default()
            }
        );
        // Absent fields are omitted on the wire.
        assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"ts_ms":42}"#);
    }

    #[test]
    fn system_source_reads_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("memory.current"), "99\n").unwrap();
        let src = SystemSource {
            cgroup_root: dir.path().to_path_buf(),
            data_path: dir.path().to_path_buf(),
        };
        assert_eq!(src.read_file("memory.current").as_deref(), Some("99\n"));
        assert_eq!(src.read_file("cpu.stat"), None);
        let (used, total) = src.statfs_used().expect("statvfs of a tempdir");
        assert!(total > 0 && used <= total);
        assert!(src.now_ms() > 1_700_000_000_000);
        let missing = SystemSource {
            cgroup_root: dir.path().join("nope"),
            data_path: dir.path().join("nope"),
        };
        assert_eq!(missing.statfs_used(), None);
        assert_eq!(snapshot(&missing).memory_bytes, None);
    }
}
