//! Automatic Docker pruning (ADR-0016, addendum).
//!
//! Nothing else deletes the images, build cache and stopped containers that runs and chats leave in
//! the dockerd data root, so a sandbox eventually fills its `docker-data` PVC. The [`Pruner`] runs
//! `docker container|image|builder|volume prune` in two situations:
//!
//! * [`Mode::Aged`] at startup (once dockerd runs) and whenever the last active run or chat ends:
//!   images and build cache older than `ENZARB_DOCKER_PRUNE_AGE` go.
//! * [`Mode::Aggressive`] when the periodic free-space check finds less than
//!   `ENZARB_DOCKER_PRUNE_MIN_FREE` percent free: no age filter.
//!
//! The docker CLI never removes a running container, nor the images and volumes it uses. Builds are
//! the race the CLI cannot see, so image and build-cache pruning only happens while no run or chat
//! is active (the [`ActivityGate`]). Pruning is best-effort: failures are logged and never reach
//! runs, chats or `/healthz`. Bytes freed are the free-space delta of the data root around the
//! sequence.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::dockerd::{DockerdConfig, State};
use crate::exec::{self, CmdSpec, Ctl};
use crate::reaper::Reaper;

pub const DEFAULT_AGE: &str = "24h";
pub const DEFAULT_MIN_FREE_PERCENT: u8 = 15;
/// How often free space is checked.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Ceiling for one docker prune command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
pub struct PruneConfig {
    /// `ENZARB_DOCKER_PRUNE=off|0|false|no` disables pruning (default on). Disk usage is still
    /// reported.
    pub enabled: bool,
    /// `ENZARB_DOCKER_PRUNE_AGE`: Go duration; images and build cache newer than this survive an
    /// aged prune.
    pub age: String,
    /// `ENZARB_DOCKER_PRUNE_MIN_FREE`: percent of the data root that must stay free (1..=99).
    pub min_free_percent: u8,
    /// `ENZARB_DOCKER_BIN`, default `docker`.
    pub docker_bin: String,
    /// The dockerd data root (`ENZARB_DOCKERD_DATA_ROOT`), where free space is measured.
    pub data_root: PathBuf,
    /// The dockerd socket (`ENZARB_DOCKERD_SOCKET`), exported to the CLI as `DOCKER_HOST`.
    pub socket: PathBuf,
    /// How often free space is checked.
    pub interval: Duration,
}

impl Default for PruneConfig {
    fn default() -> Self {
        let d = DockerdConfig::default();
        Self {
            enabled: true,
            age: DEFAULT_AGE.into(),
            min_free_percent: DEFAULT_MIN_FREE_PERCENT,
            docker_bin: "docker".into(),
            data_root: d.data_root,
            socket: d.socket,
            interval: CHECK_INTERVAL,
        }
    }
}

impl PruneConfig {
    pub fn from_lookup(
        get: impl Fn(&str) -> Option<String>,
        dockerd: &DockerdConfig,
    ) -> anyhow::Result<Self> {
        let mut c = Self {
            data_root: dockerd.data_root.clone(),
            socket: dockerd.socket.clone(),
            ..Self::default()
        };
        if let Some(v) = get("ENZARB_DOCKER_PRUNE") {
            c.enabled = match v.trim().to_ascii_lowercase().as_str() {
                "" | "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => anyhow::bail!("ENZARB_DOCKER_PRUNE: must be on or off"),
            };
        }
        if let Some(v) = get("ENZARB_DOCKER_PRUNE_AGE").filter(|s| !s.trim().is_empty()) {
            let v = v.trim();
            if !valid_age(v) {
                anyhow::bail!(
                    "ENZARB_DOCKER_PRUNE_AGE={v:?}: must be a positive duration such as 24h or 90m"
                );
            }
            c.age = v.into();
        }
        if let Some(v) = get("ENZARB_DOCKER_PRUNE_MIN_FREE").filter(|s| !s.trim().is_empty()) {
            c.min_free_percent = parse_percent(&v).ok_or_else(|| {
                anyhow::anyhow!(
                    "ENZARB_DOCKER_PRUNE_MIN_FREE={v:?}: must be a percentage from 1 to 99, such as 15%"
                )
            })?;
        }
        if let Some(v) = get("ENZARB_DOCKER_BIN").filter(|s| !s.is_empty()) {
            c.docker_bin = v;
        }
        Ok(c)
    }
}

/// `15` or `15%` (1..=99).
fn parse_percent(s: &str) -> Option<u8> {
    let n: u8 = s
        .trim()
        .strip_suffix('%')
        .unwrap_or(s.trim())
        .trim()
        .parse()
        .ok()?;
    (1..=99).contains(&n).then_some(n)
}

/// A Go duration (`24h`, `1h30m`, `90m`) that is not zero, the form `--filter until=` takes.
fn valid_age(s: &str) -> bool {
    let mut rest = s;
    let mut nonzero = false;
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let (num, tail) = rest.split_at(digits);
        let Ok(n) = num.parse::<f64>() else {
            return false;
        };
        nonzero |= n > 0.0;
        let Some(unit) = ["ns", "us", "µs", "ms", "s", "m", "h"]
            .into_iter()
            .find(|u| tail.starts_with(u))
        else {
            return false;
        };
        // `m` must not swallow the `m` of `ms`.
        let unit = if unit == "m" && tail.starts_with("ms") {
            "ms"
        } else {
            unit
        };
        rest = &tail[unit.len()..];
    }
    nonzero
}

/// Used and free space of the data root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Disk {
    pub total: u64,
    pub free: u64,
}

impl Disk {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    /// Free space as a percentage (100 when the size is unknown).
    pub fn free_percent(&self) -> f64 {
        if self.total == 0 {
            100.0
        } else {
            self.free as f64 * 100.0 / self.total as f64
        }
    }

    /// Strictly under `min_percent` free.
    pub fn is_low(&self, min_percent: u8) -> bool {
        self.total > 0
            && u128::from(self.free) * 100 < u128::from(self.total) * u128::from(min_percent)
    }
}

/// `statvfs` of `path`; `None` when it cannot be read.
pub fn disk_usage(path: &Path) -> Option<Disk> {
    let s = nix::sys::statvfs::statvfs(path).ok()?;
    #[allow(clippy::unnecessary_cast)]
    let (frsize, blocks, avail) = (
        s.fragment_size() as u64,
        s.blocks() as u64,
        s.blocks_available() as u64,
    );
    Some(Disk {
        total: blocks.saturating_mul(frsize),
        free: avail.saturating_mul(frsize),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Images and build cache older than the configured age.
    Aged,
    /// Everything unused, whatever its age (low disk).
    Aggressive,
}

/// Counts the runs and chats that may be using Docker.
#[derive(Clone, Default)]
pub struct ActivityGate(Arc<GateInner>);

#[derive(Default)]
struct GateInner {
    active: AtomicUsize,
    ended: Notify,
}

/// Held for the duration of a run or chat.
pub struct ActivityGuard(Arc<GateInner>);

impl ActivityGate {
    pub fn begin(&self) -> ActivityGuard {
        self.0.active.fetch_add(1, Ordering::SeqCst);
        ActivityGuard(self.0.clone())
    }

    pub fn active(&self) -> usize {
        self.0.active.load(Ordering::SeqCst)
    }

    pub fn is_idle(&self) -> bool {
        self.active() == 0
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.ended.notify_one();
        }
    }
}

/// Why [`Pruner::run`] did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skipped {
    Disabled,
    /// dockerd is not running.
    NotRunning,
    /// Another prune is in progress.
    Busy,
    /// Runs or chats are active and the mode would touch images or build cache.
    Active,
    /// The docker CLI is not installed.
    NoBinary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ran { freed_bytes: u64 },
    Skipped(Skipped),
}

#[derive(Debug, Clone, Serialize)]
pub struct LastPrune {
    /// Unix seconds.
    pub at: u64,
    pub mode: Mode,
    pub freed_bytes: u64,
}

/// What `/healthz` reports under `docker_disk`.
#[derive(Debug, Clone, Serialize)]
pub struct DiskReport {
    pub path: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub free_percent: f64,
    /// Under the `ENZARB_DOCKER_PRUNE_MIN_FREE` threshold, also after pruning.
    pub low_disk: bool,
}

/// What `/healthz` reports under `docker_prune`.
#[derive(Debug, Clone, Serialize)]
pub struct PruneReport {
    pub enabled: bool,
    pub last: Option<LastPrune>,
}

pub struct Pruner {
    cfg: PruneConfig,
    dockerd: watch::Receiver<State>,
    gate: ActivityGate,
    reaper: Arc<Reaper>,
    running: tokio::sync::Mutex<()>,
    last: Mutex<Option<LastPrune>>,
}

impl Pruner {
    pub fn new(
        cfg: PruneConfig,
        dockerd: watch::Receiver<State>,
        gate: ActivityGate,
        reaper: Arc<Reaper>,
    ) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            dockerd,
            gate,
            reaper,
            running: tokio::sync::Mutex::new(()),
            last: Mutex::new(None),
        })
    }

    pub fn gate(&self) -> &ActivityGate {
        &self.gate
    }

    /// The docker commands of a prune, in order (arguments after `docker`).
    pub fn plan(&self, mode: Mode, with_images: bool) -> Vec<Vec<String>> {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let until = format!("until={}", self.cfg.age);
        let mut steps = vec![s(&["container", "prune", "-f"])];
        if with_images {
            for kind in ["image", "builder"] {
                let mut a = s(&[kind, "prune", "-af"]);
                if mode == Mode::Aged {
                    a.extend(s(&["--filter", &until]));
                }
                steps.push(a);
            }
        }
        steps.push(s(&["volume", "prune", "-f"]));
        steps
    }

    pub fn disk(&self) -> Option<Disk> {
        disk_usage(&self.cfg.data_root)
    }

    pub fn disk_report(&self) -> Option<DiskReport> {
        let d = self.disk()?;
        Some(DiskReport {
            path: self.cfg.data_root.display().to_string(),
            total_bytes: d.total,
            used_bytes: d.used(),
            free_bytes: d.free,
            free_percent: (d.free_percent() * 10.0).round() / 10.0,
            low_disk: d.is_low(self.cfg.min_free_percent),
        })
    }

    pub fn prune_report(&self) -> PruneReport {
        PruneReport {
            enabled: self.cfg.enabled,
            last: self.last.lock().unwrap().clone(),
        }
    }

    /// One prune. Never fails: problems are logged and reported as [`Skipped`] or ignored.
    pub async fn run(&self, mode: Mode) -> Outcome {
        if !self.cfg.enabled {
            return Outcome::Skipped(Skipped::Disabled);
        }
        if *self.dockerd.borrow() != State::Running {
            return Outcome::Skipped(Skipped::NotRunning);
        }
        // Image and build-cache pruning could break a build in flight, which cannot be seen:
        // only while nothing runs. An aged prune has nothing else to do, so it waits entirely.
        let with_images = self.gate.is_idle();
        if !with_images && mode == Mode::Aged {
            return Outcome::Skipped(Skipped::Active);
        }
        let Ok(_guard) = self.running.try_lock() else {
            return Outcome::Skipped(Skipped::Busy);
        };
        let before = self.disk();
        for args in self.plan(mode, with_images) {
            match self.docker(&args).await {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::warn!(bin = %self.cfg.docker_bin, "docker prune skipped: docker CLI not found");
                    return Outcome::Skipped(Skipped::NoBinary);
                }
                Err(e) => {
                    tracing::warn!(cmd = %args.join(" "), error = %e, "docker prune: cannot run")
                }
            }
        }
        let freed_bytes = match (before, self.disk()) {
            (Some(b), Some(a)) => a.free.saturating_sub(b.free),
            _ => 0,
        };
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        *self.last.lock().unwrap() = Some(LastPrune {
            at,
            mode,
            freed_bytes,
        });
        tracing::info!(?mode, freed_bytes, images = with_images, "docker prune");
        if let Some(d) = self.disk()
            && d.is_low(self.cfg.min_free_percent)
        {
            tracing::warn!(
                free_percent = d.free_percent(),
                min_free_percent = self.cfg.min_free_percent,
                "low_disk: docker data root is still under the free-space threshold after pruning"
            );
        }
        Outcome::Ran { freed_bytes }
    }

    /// `Ok(false)` when the command exited non-zero (logged).
    async fn docker(&self, args: &[String]) -> std::io::Result<bool> {
        let mut spec = CmdSpec::new(&self.cfg.docker_bin, &[]);
        spec.args = args.to_vec();
        spec.env = vec![(
            "DOCKER_HOST".into(),
            format!("unix://{}", self.cfg.socket.display()),
        )];
        let (_keep, cancel) = watch::channel(false);
        let mut ctl = Ctl {
            cancel,
            deadline: Instant::now() + COMMAND_TIMEOUT,
        };
        let out = exec::run(&self.reaper, spec, &mut ctl, None).await?;
        if !out.success() {
            tracing::warn!(
                cmd = %args.join(" "),
                code = ?out.code(),
                stop = ?out.stop,
                stderr = %out.stderr.trim(),
                "docker prune: command failed"
            );
        }
        Ok(out.success())
    }

    /// One free-space check: an aggressive prune when the data root is under the threshold.
    pub async fn check(&self) -> Option<Outcome> {
        let low = self
            .disk()
            .is_some_and(|d| d.is_low(self.cfg.min_free_percent));
        if low {
            tracing::warn!(
                min_free_percent = self.cfg.min_free_percent,
                "docker data root is low on space"
            );
            Some(self.run(Mode::Aggressive).await)
        } else {
            None
        }
    }

    /// Background task: a prune once dockerd runs, one whenever the last run or chat ends, and the
    /// periodic free-space check. Ends when `stop` is cancelled.
    pub fn spawn(self: &Arc<Self>, stop: CancellationToken) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut state = me.dockerd.clone();
            tokio::select! {
                _ = stop.cancelled() => return,
                r = state.wait_for(|s| *s == State::Running) => {
                    if r.is_err() {
                        return;
                    }
                }
            }
            me.run(Mode::Aged).await;
            let mut tick =
                tokio::time::interval_at(Instant::now() + me.cfg.interval, me.cfg.interval);
            loop {
                tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = me.gate.0.ended.notified() => {
                        me.run(Mode::Aged).await;
                    }
                    _ = tick.tick() => {
                        me.check().await;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn lookup<'a>(kv: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| kv.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    fn cfg(kv: &[(&str, &str)]) -> anyhow::Result<PruneConfig> {
        PruneConfig::from_lookup(lookup(kv), &DockerdConfig::default())
    }

    #[test]
    fn defaults() {
        let c = cfg(&[]).unwrap();
        assert!(c.enabled);
        assert_eq!(c.age, "24h");
        assert_eq!(c.min_free_percent, 15);
        assert_eq!(c.docker_bin, "docker");
        assert_eq!(c.data_root, PathBuf::from("/var/lib/docker"));
    }

    #[test]
    fn enabled_values() {
        for v in ["off", "0", "false", "No", " OFF "] {
            assert!(!cfg(&[("ENZARB_DOCKER_PRUNE", v)]).unwrap().enabled, "{v}");
        }
        for v in ["on", "1", "true", "yes", ""] {
            assert!(cfg(&[("ENZARB_DOCKER_PRUNE", v)]).unwrap().enabled, "{v}");
        }
        assert!(cfg(&[("ENZARB_DOCKER_PRUNE", "maybe")]).is_err());
    }

    #[test]
    fn min_free_values() {
        for (v, n) in [("15", 15), ("20%", 20), (" 5 % ", 5), ("99", 99), ("1%", 1)] {
            let c = cfg(&[("ENZARB_DOCKER_PRUNE_MIN_FREE", v)]).unwrap();
            assert_eq!(c.min_free_percent, n, "{v}");
        }
        for v in ["0", "100", "-3", "abc", "15.5", "150%"] {
            assert!(cfg(&[("ENZARB_DOCKER_PRUNE_MIN_FREE", v)]).is_err(), "{v}");
        }
    }

    #[test]
    fn age_values() {
        for v in ["24h", "90m", "1h30m", "36h", "30s", "1.5h"] {
            assert_eq!(cfg(&[("ENZARB_DOCKER_PRUNE_AGE", v)]).unwrap().age, v);
        }
        for v in ["0", "0h", "24", "h", "1d", "abc", "-1h", "24 h"] {
            assert!(cfg(&[("ENZARB_DOCKER_PRUNE_AGE", v)]).is_err(), "{v}");
        }
    }

    #[test]
    fn threshold_boundary() {
        let d = |free| Disk { total: 1000, free };
        assert!(!d(150).is_low(15), "exactly the threshold is not low");
        assert!(d(149).is_low(15));
        assert!(!d(151).is_low(15));
        assert!(d(0).is_low(15));
        // an unknown size is never low
        assert!(!Disk { total: 0, free: 0 }.is_low(15));
        assert_eq!(d(250).free_percent(), 25.0);
        assert_eq!(d(250).used(), 750);
        // no overflow on huge volumes
        assert!(
            Disk {
                total: u64::MAX,
                free: 1
            }
            .is_low(99)
        );
    }

    #[test]
    fn disk_usage_of_a_real_path() {
        let d = disk_usage(Path::new("/tmp")).unwrap();
        assert!(d.total > 0 && d.free <= d.total);
        assert!(disk_usage(Path::new("/nonexistent/x")).is_none());
    }

    #[test]
    fn gate_counts_and_notifies_on_idle() {
        let g = ActivityGate::default();
        assert!(g.is_idle());
        let a = g.begin();
        let b = g.begin();
        assert_eq!(g.active(), 2);
        drop(a);
        assert!(!g.is_idle());
        drop(b);
        assert!(g.is_idle());
    }

    fn pruner_with(
        bin: &str,
        enabled: bool,
        state: State,
    ) -> (Arc<Pruner>, watch::Sender<State>, ActivityGate) {
        let reaper = Reaper::new(false);
        tokio::spawn(reaper.clone().run());
        let (tx, rx) = watch::channel(state);
        let gate = ActivityGate::default();
        let c = PruneConfig {
            enabled,
            docker_bin: bin.into(),
            data_root: "/tmp".into(),
            ..PruneConfig::default()
        };
        (Pruner::new(c, rx, gate.clone(), reaper), tx, gate)
    }

    /// A `docker` that logs its arguments; `fail` is a prefix of arguments that makes it exit 1.
    fn fake_docker(dir: &Path, fail: &str) -> (String, PathBuf) {
        let log = dir.join("calls.log");
        let script = dir.join("docker");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo \"$*\" >> {log}\n\
                 case \"$*\" in\n\
                 \"ps\"*) echo running-container-abc123; exit 0;;\n\
                 esac\n\
                 if [ -n \"{fail}\" ]; then case \"$*\" in \"{fail}\"*) echo boom >&2; exit 1;; esac; fi\n\
                 exit 0\n",
                log = log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (script.display().to_string(), log)
    }

    fn calls(log: &Path) -> Vec<String> {
        std::fs::read_to_string(log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn plan_filters_per_mode() {
        let (p, _tx, _g) = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _e = rt.enter();
            pruner_with("docker", true, State::Running)
        };
        assert_eq!(
            p.plan(Mode::Aged, true),
            [
                vec!["container", "prune", "-f"],
                vec!["image", "prune", "-af", "--filter", "until=24h"],
                vec!["builder", "prune", "-af", "--filter", "until=24h"],
                vec!["volume", "prune", "-f"],
            ]
        );
        assert_eq!(
            p.plan(Mode::Aggressive, true),
            [
                vec!["container", "prune", "-f"],
                vec!["image", "prune", "-af"],
                vec!["builder", "prune", "-af"],
                vec!["volume", "prune", "-f"],
            ]
        );
        assert_eq!(
            p.plan(Mode::Aggressive, false),
            [
                vec!["container", "prune", "-f"],
                vec!["volume", "prune", "-f"]
            ]
        );
    }

    #[tokio::test]
    async fn runs_the_sequence_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, _tx, _g) = pruner_with(&bin, true, State::Running);
        assert!(matches!(p.run(Mode::Aged).await, Outcome::Ran { .. }));
        assert_eq!(
            calls(&log),
            [
                "container prune -f",
                "image prune -af --filter until=24h",
                "builder prune -af --filter until=24h",
                "volume prune -f",
            ]
        );
        let r = p.prune_report();
        assert!(r.enabled);
        assert_eq!(r.last.unwrap().mode, Mode::Aged);
    }

    #[tokio::test]
    async fn failing_step_does_not_stop_the_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "image prune");
        let (p, _tx, _g) = pruner_with(&bin, true, State::Running);
        assert!(matches!(p.run(Mode::Aggressive).await, Outcome::Ran { .. }));
        assert_eq!(
            calls(&log),
            [
                "container prune -f",
                "image prune -af",
                "builder prune -af",
                "volume prune -f"
            ]
        );
    }

    #[tokio::test]
    async fn missing_binary_is_skipped() {
        let (p, _tx, _g) = pruner_with("/nonexistent/docker", true, State::Running);
        assert_eq!(p.run(Mode::Aged).await, Outcome::Skipped(Skipped::NoBinary));
        assert!(p.prune_report().last.is_none());
    }

    #[tokio::test]
    async fn disabled_runs_nothing_but_reports_disk() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, _tx, _g) = pruner_with(&bin, false, State::Running);
        assert_eq!(
            p.run(Mode::Aggressive).await,
            Outcome::Skipped(Skipped::Disabled)
        );
        assert!(calls(&log).is_empty());
        assert!(p.disk_report().is_some());
        assert!(!p.prune_report().enabled);
    }

    #[tokio::test]
    async fn needs_a_running_dockerd() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        for s in [State::Starting, State::Disabled, State::Stopped] {
            let (p, _tx, _g) = pruner_with(&bin, true, s);
            assert_eq!(
                p.run(Mode::Aged).await,
                Outcome::Skipped(Skipped::NotRunning)
            );
        }
        assert!(calls(&log).is_empty());
    }

    #[tokio::test]
    async fn active_runs_defer_image_and_builder_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, _tx, gate) = pruner_with(&bin, true, State::Running);
        let _busy = gate.begin();
        // an event-driven prune is skipped entirely
        assert_eq!(p.run(Mode::Aged).await, Outcome::Skipped(Skipped::Active));
        assert!(calls(&log).is_empty());
        // a low-disk prune only does what docker itself keeps safe for running containers
        assert!(matches!(p.run(Mode::Aggressive).await, Outcome::Ran { .. }));
        assert_eq!(calls(&log), ["container prune -f", "volume prune -f"]);
    }

    #[tokio::test]
    async fn a_container_in_use_is_never_named() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, _tx, gate) = pruner_with(&bin, true, State::Running);
        // the fake daemon reports a running container
        let ps = std::process::Command::new(&bin).arg("ps").output().unwrap();
        let running = String::from_utf8(ps.stdout).unwrap().trim().to_string();
        let _busy = gate.begin();
        p.run(Mode::Aggressive).await;
        drop(_busy);
        p.run(Mode::Aged).await;
        for call in calls(&log).iter().filter(|c| *c != "ps") {
            assert!(!call.contains(&running), "{call}");
            // only prune subcommands: never rm/kill/stop/rmi
            assert!(call.contains(" prune "), "{call}");
        }
    }

    #[tokio::test]
    async fn concurrent_runs_produce_one_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, _tx, _g) = pruner_with(&bin, true, State::Running);
        let (a, b) = tokio::join!(p.run(Mode::Aged), p.run(Mode::Aged));
        let ran = [&a, &b]
            .iter()
            .filter(|o| matches!(o, Outcome::Ran { .. }))
            .count();
        assert_eq!(ran, 1, "{a:?} {b:?}");
        assert_eq!(calls(&log).len(), 4);
    }

    #[tokio::test]
    async fn spawned_task_prunes_at_startup_and_after_the_last_activity() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, tx, gate) = pruner_with(&bin, true, State::Starting);
        let stop = CancellationToken::new();
        let h = p.spawn(stop.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(calls(&log).is_empty(), "waits for dockerd");
        tx.send_replace(State::Running);
        wait_for_calls(&log, 4).await;
        let a = gate.begin();
        let b = gate.begin();
        drop(a);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(calls(&log).len(), 4, "one run is still active");
        drop(b);
        wait_for_calls(&log, 8).await;
        stop.cancel();
        h.await.unwrap();
    }

    async fn wait_for_calls(log: &Path, n: usize) {
        for _ in 0..100 {
            if calls(log).len() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("expected {n} docker calls, got {:?}", calls(log));
    }

    #[tokio::test]
    async fn check_prunes_only_when_low() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_docker(dir.path(), "");
        let (p, tx, _g) = pruner_with(&bin, true, State::Running);
        // /tmp has more than 1% free in any sane environment
        let mut c = p.cfg.clone();
        c.min_free_percent = 1;
        let fine = Pruner::new(
            c.clone(),
            tx.subscribe(),
            ActivityGate::default(),
            p.reaper.clone(),
        );
        if fine.disk().is_some_and(|d| !d.is_low(1)) {
            assert!(fine.check().await.is_none());
            assert!(calls(&log).is_empty());
        }
        // 99% free required: any real disk is under that
        c.min_free_percent = 99;
        let low = Pruner::new(c, tx.subscribe(), ActivityGate::default(), p.reaper.clone());
        if low.disk().is_some_and(|d| d.is_low(99)) {
            assert!(matches!(low.check().await, Some(Outcome::Ran { .. })));
            assert_eq!(calls(&log)[1], "image prune -af");
            assert!(low.disk_report().unwrap().low_disk);
        }
    }
}
