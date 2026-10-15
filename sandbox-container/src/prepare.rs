//! Run preparation: everything a fresh worktree needs before the agent starts.
//!
//! For each component of the worktree: `mise install` (so the pinned node is
//! on PATH), then `npm ci` unless the lockfile hash matches a cached install.
//! One shared test Postgres per sandbox is started when the repo's suites
//! need it, and its URL is handed to the agent as `ENZARB_TEST_PG_URL`. Every
//! step is timed; the summary goes to the transcript (`prepare` event) and to
//! the log (`prepare: 41.2s (npm ci ui 30.1s, ...)`).
//!
//! Preparation is best-effort: a failed step is logged and recorded as
//! `failed`, and the agent still starts (it can install what it needs itself).
//! Only a stop (cancel, run deadline) aborts the run.
//!
//! The npm cache lives on the workspace volume, next to the worktrees (same
//! filesystem, so a hit is a hardlink copy):
//! `<workspace>/cache/npm/<repo>/<component>/<sha256(package-lock.json)>/node_modules`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;

use crate::agent::RepoLocks;
use crate::config::Config;
use crate::exec::{self, CmdSpec, Ctl, LineSink, Output, Stop};
use crate::reaper::Reaper;

/// Marker written into an installed `node_modules`: the lockfile hash it matches.
pub const MARKER: &str = ".enzarb-lock-hash";
/// Lockfile hashes kept per component in the cache.
const KEEP_HASHES: usize = 2;
/// Readiness probes of the test Postgres (one second apart).
const PG_READY_TRIES: usize = 60;
/// Components that run their tests against the shared Postgres.
const PG_COMPONENTS: [&str; 3] = ["workflows", "connectors", "realtime"];

/// One Postgres start at a time per sandbox.
static PG_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

#[derive(Debug)]
pub enum PrepError {
    Stop(Stop),
    Fail(String),
}

/// What preparation needs from the run.
pub struct Ctx<'a> {
    pub reaper: &'a Reaper,
    pub cfg: &'a Config,
    pub locks: &'a RepoLocks,
    /// Cache key of the repo (`repo_dir_name`).
    pub repo: String,
    /// Environment of the install commands (registry tokens).
    pub env: Vec<(String, String)>,
    /// Log lines of a source (redacted by the caller).
    pub sink: &'a (dyn Fn(&'static str) -> LineSink + Sync),
    /// Transcript events (redacted by the caller).
    pub event: &'a (dyn Fn(&str, Value) + Sync),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub step: String,
    pub component: String,
    /// `ok`, `skipped` or `failed`.
    pub status: &'static str,
    pub ms: u64,
}

/// What to do about one component's `node_modules`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// The worktree already holds the install for this lockfile.
    Skip,
    /// The cache holds it: copy it in.
    Restore,
    Install,
}

/// Hex sha256 of the lockfile.
pub fn lock_hash(lockfile: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(lockfile)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Pure choice: `marker` is the hash recorded in the worktree's `node_modules`.
pub fn decide(marker: Option<&str>, hash: &str, cached: bool) -> Decision {
    if marker.map(str::trim) == Some(hash) {
        Decision::Skip
    } else if cached {
        Decision::Restore
    } else {
        Decision::Install
    }
}

fn fmt_ms(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// `prepare: 41.2s (npm ci ui 30.1s, mise install ui 8.0s)`.
pub fn summary(steps: &[Step], total_ms: u64) -> String {
    let parts: Vec<String> = steps
        .iter()
        .map(|s| {
            let label = if s.component.is_empty() {
                s.step.clone()
            } else {
                format!("{} {}", s.step, s.component)
            };
            match s.status {
                "ok" => format!("{label} {}", fmt_ms(s.ms)),
                other => format!("{label} {other} {}", fmt_ms(s.ms)),
            }
        })
        .collect();
    if parts.is_empty() {
        format!("prepare: {}", fmt_ms(total_ms))
    } else {
        format!("prepare: {} ({})", fmt_ms(total_ms), parts.join(", "))
    }
}

/// Port from `docker port <name> 5432/tcp` (`127.0.0.1:55432`, maybe several lines).
pub fn parse_port(out: &str) -> Option<u16> {
    out.lines().next()?.trim().rsplit(':').next()?.parse().ok()
}

/// The directories to prepare, relative to the worktree: the root plus every
/// top-level directory with a lockfile or a `mise.toml`.
pub fn components(worktree: &Path) -> Vec<String> {
    let mut out = vec![".".to_string()];
    let mut dirs: Vec<String> = std::fs::read_dir(worktree)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "node_modules" && n != "target")
        .filter(|n| {
            let d = worktree.join(n);
            d.join("package-lock.json").is_file() || d.join("mise.toml").is_file()
        })
        .collect();
    dirs.sort();
    out.extend(dirs);
    out
}

fn needs_pg(worktree: &Path) -> bool {
    PG_COMPONENTS.iter().any(|c| worktree.join(c).is_dir())
        || worktree.join(".mise/tasks/test/pg").is_file()
}

fn tail(s: &str) -> String {
    let t = s.trim();
    let skip = t.chars().count().saturating_sub(1000);
    t.chars().skip(skip).collect()
}

/// Prepare `worktree`. Returns the `ENZARB_TEST_PG_URL` when Postgres is up.
/// Fails only on a stop; failed steps are logged and the summary is emitted.
pub async fn run(
    ctx: &Ctx<'_>,
    ctl: &mut Ctl,
    worktree: &Path,
) -> Result<Option<String>, PrepError> {
    let mut p = Prep {
        ctx,
        steps: Vec::new(),
        start: Instant::now(),
    };
    let res = p.all(ctl, worktree).await;
    let total = p.start.elapsed().as_millis() as u64;
    (ctx.event)(
        "prepare",
        json!({"total_ms": total, "steps": p.steps.iter().map(|s| json!({
            "step": s.step, "component": s.component, "status": s.status, "ms": s.ms,
        })).collect::<Vec<_>>(), "ok": res.is_ok() && p.steps.iter().all(|s| s.status != "failed")}),
    );
    (ctx.sink)("prepare")(&summary(&p.steps, total));
    res
}

struct Prep<'a, 'b> {
    ctx: &'a Ctx<'b>,
    steps: Vec<Step>,
    start: Instant,
}

impl Prep<'_, '_> {
    fn record(&mut self, step: &str, component: &str, status: &'static str, t: Instant) {
        self.steps.push(Step {
            step: step.into(),
            component: if component == "." { "root" } else { component }.into(),
            status,
            ms: t.elapsed().as_millis() as u64,
        });
    }

    async fn all(&mut self, ctl: &mut Ctl, worktree: &Path) -> Result<Option<String>, PrepError> {
        let cfg = self.ctx.cfg;
        for comp in components(worktree) {
            let dir = worktree.join(&comp);
            let has_mise = dir.join("mise.toml").is_file();
            if !cfg.mise_bin.is_empty() && (comp == "." || has_mise) {
                let r = self.mise_install(ctl, &comp, &dir).await;
                self.tolerate(r)?;
            }
            if dir.join("package-lock.json").is_file() {
                let r = self.npm_ci(ctl, &comp, &dir).await;
                self.tolerate(r)?;
            }
        }
        if !needs_pg(worktree) {
            return Ok(None);
        }
        let t = Instant::now();
        match self.ensure_pg(ctl).await {
            Ok(url) => {
                self.record("postgres", "", "ok", t);
                Ok(Some(url))
            }
            Err(PrepError::Stop(s)) => Err(PrepError::Stop(s)),
            Err(PrepError::Fail(m)) => {
                (self.ctx.sink)("prepare")(&format!("postgres unavailable: {m}"));
                self.record("postgres", "", "failed", t);
                Ok(None)
            }
        }
    }

    /// Log a failed step and go on; a stop still aborts the run.
    fn tolerate(&self, r: Result<(), PrepError>) -> Result<(), PrepError> {
        match r {
            Err(PrepError::Fail(m)) => {
                (self.ctx.sink)("prepare")(&m);
                Ok(())
            }
            other => other,
        }
    }

    /// A copy of `ctl` whose deadline is the step's cap (or the run's, if earlier).
    fn step_ctl(&self, ctl: &Ctl) -> Ctl {
        let mut c = ctl.clone();
        let cap = tokio::time::Instant::now() + self.ctx.cfg.prepare_step_timeout;
        c.deadline = c.deadline.min(cap);
        c
    }

    /// Run one command under `ctl` (already step-capped); stops become errors.
    async fn exec(
        &self,
        run_ctl: &Ctl,
        step_ctl: &mut Ctl,
        spec: CmdSpec,
        source: &'static str,
        what: &str,
    ) -> Result<Output, PrepError> {
        let sink = (self.ctx.sink)(source);
        let out = exec::run(self.ctx.reaper, spec, step_ctl, Some(sink))
            .await
            .map_err(|e| PrepError::Fail(format!("{what}: cannot run: {e}")))?;
        match out.stop {
            None => Ok(out),
            Some(Stop::TimedOut) if tokio::time::Instant::now() < run_ctl.deadline => {
                Err(PrepError::Fail(format!(
                    "{what} timed out after {}s",
                    self.ctx.cfg.prepare_step_timeout.as_secs()
                )))
            }
            Some(s) => Err(PrepError::Stop(s)),
        }
    }

    async fn mise_install(
        &mut self,
        ctl: &mut Ctl,
        comp: &str,
        dir: &Path,
    ) -> Result<(), PrepError> {
        let t = Instant::now();
        let mut spec = CmdSpec::new(&self.ctx.cfg.mise_bin, &["install"]);
        spec.cwd = Some(dir.to_path_buf());
        spec.env = self.ctx.env.clone();
        let what = format!("mise install {comp}");
        let mut sc = self.step_ctl(ctl);
        let res = match self.exec(ctl, &mut sc, spec, "mise", &what).await {
            Ok(out) if out.success() => Ok(()),
            Ok(out) => Err(PrepError::Fail(format!(
                "{what} failed (exit {:?}): {}",
                out.code(),
                tail(&out.stderr)
            ))),
            Err(e) => Err(e),
        };
        match &res {
            Ok(()) => self.record("mise install", comp, "ok", t),
            Err(PrepError::Fail(_)) => self.record("mise install", comp, "failed", t),
            Err(PrepError::Stop(_)) => {}
        }
        res
    }

    async fn npm_ci(&mut self, ctl: &mut Ctl, comp: &str, dir: &Path) -> Result<(), PrepError> {
        let t = Instant::now();
        let what = format!("npm ci {comp}");
        let hash = lock_hash(&dir.join("package-lock.json"))
            .map_err(|e| PrepError::Fail(format!("{what}: cannot read package-lock.json: {e}")))?;
        let key = if comp == "." {
            "_root".to_string()
        } else {
            comp.replace('/', "_")
        };
        let cache_root = self
            .ctx
            .cfg
            .workspace
            .join("cache/npm")
            .join(&self.ctx.repo)
            .join(key);
        let entry = cache_root.join(&hash);
        // Same lockfile, one install: the second run then hits the cache.
        let lock = self.ctx.locks.get(&entry);
        let _g = lock.lock().await;

        let nm = dir.join("node_modules");
        let marker = std::fs::read_to_string(nm.join(MARKER)).ok();
        let cached = entry.join("node_modules").is_dir();
        match decide(marker.as_deref(), &hash, cached) {
            Decision::Skip => {
                self.record("npm ci", comp, "skipped", t);
                return Ok(());
            }
            Decision::Restore => {
                let _ = std::fs::remove_dir_all(&nm);
                if self.copy(ctl, &entry.join("node_modules"), &nm).await {
                    self.record("npm ci", comp, "skipped", t);
                    return Ok(());
                }
                let _ = std::fs::remove_dir_all(&nm);
            }
            Decision::Install => {}
        }

        let cfg = self.ctx.cfg;
        let mut spec = if cfg.mise_bin.is_empty() {
            CmdSpec::new(&cfg.npm_bin, &["ci"])
        } else {
            CmdSpec::new(&cfg.mise_bin, &["exec", "--", &cfg.npm_bin, "ci"])
        };
        spec.args
            .extend(["--prefer-offline", "--no-audit", "--no-fund"].map(String::from));
        spec.cwd = Some(dir.to_path_buf());
        spec.env = self.ctx.env.clone();
        let mut sc = self.step_ctl(ctl);
        let out = match self.exec(ctl, &mut sc, spec, "npm", &what).await {
            Ok(o) => o,
            Err(e) => {
                self.record("npm ci", comp, "failed", t);
                return Err(e);
            }
        };
        if !out.success() {
            self.record("npm ci", comp, "failed", t);
            return Err(PrepError::Fail(format!(
                "{what} failed (exit {:?}): {}",
                out.code(),
                tail(&out.stderr)
            )));
        }
        self.record("npm ci", comp, "ok", t);
        // Fill the cache; a failure here only costs the next run an install.
        if std::fs::write(nm.join(MARKER), &hash).is_ok() {
            self.fill_cache(ctl, &nm, &cache_root, &hash).await;
        }
        Ok(())
    }

    /// `cp -al` (hardlinks), falling back to a plain copy.
    async fn copy(&self, ctl: &Ctl, src: &Path, dst: &Path) -> bool {
        let (s, d) = (src.to_string_lossy(), dst.to_string_lossy());
        for flags in ["-al", "-a"] {
            let mut sc = self.step_ctl(ctl);
            let spec = CmdSpec::new("cp", &[flags, &s, &d]);
            match self.exec(ctl, &mut sc, spec, "prepare", "cp").await {
                Ok(o) if o.success() => return true,
                _ => {
                    let _ = std::fs::remove_dir_all(dst);
                }
            }
        }
        false
    }

    async fn fill_cache(&self, ctl: &Ctl, nm: &Path, cache_root: &Path, hash: &str) {
        let tmp = cache_root.join(format!("{hash}.tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        if crate::privs::create_dir_all(&tmp).is_err() {
            return;
        }
        let ok = self.copy(ctl, nm, &tmp.join("node_modules")).await
            && std::fs::rename(&tmp, cache_root.join(hash)).is_ok();
        if !ok {
            let _ = std::fs::remove_dir_all(&tmp);
            return;
        }
        let root = cache_root.to_path_buf();
        let keep = hash.to_string();
        let _ = tokio::task::spawn_blocking(move || prune(&root, &keep)).await;
    }

    /// Start (or reuse) the shared test Postgres; its URL on success.
    async fn ensure_pg(&self, ctl: &mut Ctl) -> Result<String, PrepError> {
        let _g = PG_LOCK.lock().await;
        let cfg = self.ctx.cfg;
        let name = cfg.pg_name.as_str();
        let mut sc = self.step_ctl(ctl);
        let mut docker = async |args: &[&str]| -> Result<Output, PrepError> {
            let spec = CmdSpec::new(&cfg.docker_bin, args);
            self.exec(ctl, &mut sc, spec, "docker", "postgres").await
        };

        let state = docker(&["inspect", "-f", "{{.State.Status}}", name]).await?;
        let state = if state.success() {
            String::from_utf8_lossy(&state.stdout).trim().to_string()
        } else {
            String::new()
        };
        let started = match state.as_str() {
            "running" => None,
            "" => {
                let publish = format!("127.0.0.1:{}:5432", cfg.pg_port);
                Some(
                    docker(&[
                        "run",
                        "-d",
                        "--name",
                        name,
                        "-p",
                        &publish,
                        "-e",
                        "POSTGRES_PASSWORD=postgres",
                        "postgres:18",
                        "-c",
                        "max_connections=1000",
                        "-c",
                        "fsync=off",
                        "-c",
                        "synchronous_commit=off",
                        "-c",
                        "full_page_writes=off",
                    ])
                    .await?,
                )
            }
            _ => Some(docker(&["start", name]).await?),
        };
        if let Some(o) = started
            && !o.success()
        {
            return Err(PrepError::Fail(format!(
                "docker could not start {name}: {}",
                tail(&o.stderr)
            )));
        }
        let port = docker(&["port", name, "5432/tcp"]).await?;
        let port = parse_port(&String::from_utf8_lossy(&port.stdout))
            .ok_or_else(|| PrepError::Fail(format!("{name} publishes no port 5432")))?;
        // Over TCP: during first-run init the entrypoint's server only listens on the socket.
        for i in 0..PG_READY_TRIES {
            let o = docker(&[
                "exec",
                name,
                "pg_isready",
                "-h",
                "127.0.0.1",
                "-U",
                "postgres",
                "-q",
            ])
            .await?;
            if o.success() {
                return Ok(format!(
                    "postgres://postgres:postgres@127.0.0.1:{port}/postgres"
                ));
            }
            if i + 1 < PG_READY_TRIES {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        Err(PrepError::Fail(format!(
            "{name} did not become ready in {PG_READY_TRIES}s"
        )))
    }
}

/// Remove all but the newest [`KEEP_HASHES`] entries (and stale temp dirs) of a
/// component's cache; `keep` is never removed.
fn prune(root: &Path, keep: &str) {
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    entries.sort_by_key(|a| std::cmp::Reverse(a.0));
    let mut kept = 0;
    for (_, p) in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.ends_with(".tmp") {
            let _ = std::fs::remove_dir_all(&p);
        } else if name == keep || kept < KEEP_HASHES {
            kept += 1;
        } else {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};
    use tokio::sync::watch;

    struct Fx {
        _d: tempfile::TempDir,
        dir: PathBuf,
        cfg: Config,
        reaper: Arc<Reaper>,
        locks: RepoLocks,
        _tx: watch::Sender<bool>,
        ctl: Ctl,
        logs: Arc<Mutex<Vec<String>>>,
        events: Arc<Mutex<Vec<Value>>>,
    }

    fn script(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    impl Fx {
        /// A fake `npm` that counts its runs and installs one file.
        fn new(npm_body: &str) -> Self {
            let d = tempfile::tempdir().unwrap();
            let dir = d.path().to_path_buf();
            let npm = dir.join("fakenpm");
            script(
                &npm,
                &format!(
                    "echo \"$@\" >> '{c}'\n{npm_body}\nmkdir -p node_modules/pkg\necho hi > node_modules/pkg/index.js",
                    c = dir.join("npm-count").display()
                ),
            );
            let cfg = Config {
                workspace: dir.join("ws"),
                npm_bin: npm.to_string_lossy().into_owned(),
                docker_bin: dir.join("nodocker").to_string_lossy().into_owned(),
                ..Config::default()
            };
            let reaper = Reaper::new(false);
            tokio::spawn(reaper.clone().run());
            let (tx, rx) = watch::channel(false);
            Self {
                _d: d,
                dir,
                cfg,
                reaper,
                locks: RepoLocks::default(),
                _tx: tx,
                ctl: Ctl {
                    cancel: rx,
                    deadline: tokio::time::Instant::now() + Duration::from_secs(60),
                },
                logs: Arc::default(),
                events: Arc::default(),
            }
        }

        /// A worktree `wt/<name>` with a `ui` component locked at `lock`.
        fn worktree(&self, name: &str, lock: &str) -> PathBuf {
            let wt = self.dir.join("wt").join(name);
            std::fs::create_dir_all(wt.join("ui")).unwrap();
            std::fs::write(wt.join("ui/package-lock.json"), lock).unwrap();
            wt
        }

        fn npm_runs(&self) -> usize {
            std::fs::read_to_string(self.dir.join("npm-count")).map_or(0, |s| s.lines().count())
        }

        async fn prepare(&mut self, wt: &Path) -> Result<Option<String>, PrepError> {
            let logs = self.logs.clone();
            let events = self.events.clone();
            let sink = move |_: &'static str| -> LineSink {
                let logs = logs.clone();
                Arc::new(move |l: &str| logs.lock().unwrap().push(l.to_string()))
            };
            let event = move |k: &str, v: Value| {
                events.lock().unwrap().push(json!({"kind": k, "data": v}));
            };
            let ctx = Ctx {
                reaper: &self.reaper,
                cfg: &self.cfg,
                locks: &self.locks,
                repo: "o-r".into(),
                env: vec![],
                sink: &sink,
                event: &event,
            };
            run(&ctx, &mut self.ctl, wt).await
        }
    }

    #[test]
    fn hash_and_decision() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("l.json");
        std::fs::write(&f, "a").unwrap();
        let a = lock_hash(&f).unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(a, lock_hash(&f).unwrap());
        std::fs::write(&f, "b").unwrap();
        assert_ne!(a, lock_hash(&f).unwrap());
        assert_eq!(decide(Some(&a), &a, false), Decision::Skip);
        assert_eq!(decide(Some("old"), &a, true), Decision::Restore);
        assert_eq!(decide(None, &a, false), Decision::Install);
    }

    #[test]
    fn summary_format() {
        let s = |step: &str, c: &str, status, ms| Step {
            step: step.into(),
            component: c.into(),
            status,
            ms,
        };
        let steps = [
            s("mise install", "ui", "ok", 8000),
            s("npm ci", "ui", "skipped", 400),
            s("postgres", "", "ok", 2300),
        ];
        assert_eq!(
            summary(&steps, 41200),
            "prepare: 41.2s (mise install ui 8.0s, npm ci ui skipped 0.4s, postgres 2.3s)"
        );
        assert_eq!(summary(&[], 100), "prepare: 0.1s");
    }

    #[test]
    fn port_parsing() {
        assert_eq!(parse_port("127.0.0.1:55432\n[::]:55432\n"), Some(55432));
        assert_eq!(parse_port("0.0.0.0:6000"), Some(6000));
        assert_eq!(parse_port(""), None);
    }

    #[tokio::test]
    async fn second_run_hits_the_cache_and_a_new_lockfile_reinstalls() {
        let mut fx = Fx::new("");
        let a = fx.worktree("a", "lock-1");
        fx.prepare(&a).await.unwrap();
        assert_eq!(fx.npm_runs(), 1);
        let args = std::fs::read_to_string(fx.dir.join("npm-count")).unwrap();
        assert_eq!(args.trim(), "ci --prefer-offline --no-audit --no-fund");
        assert!(a.join("ui/node_modules").join(MARKER).is_file());

        let b = fx.worktree("b", "lock-1");
        fx.prepare(&b).await.unwrap();
        assert_eq!(fx.npm_runs(), 1, "unchanged lockfile skips npm ci");
        assert!(b.join("ui/node_modules/pkg/index.js").is_file());
        assert!(
            fx.logs
                .lock()
                .unwrap()
                .iter()
                .any(|l| l.contains("npm ci ui skipped"))
        );

        // The same worktree again: the marker matches.
        fx.prepare(&b).await.unwrap();
        assert_eq!(fx.npm_runs(), 1);

        let c = fx.worktree("c", "lock-2");
        fx.prepare(&c).await.unwrap();
        assert_eq!(fx.npm_runs(), 2, "changed lockfile reinstalls");
    }

    #[tokio::test]
    async fn failed_npm_ci_is_logged_writes_no_cache_and_the_run_goes_on() {
        let mut fx = Fx::new("echo boom >&2; exit 1");
        let a = fx.worktree("a", "lock-1");
        fx.prepare(&a).await.unwrap();
        let logs = fx.logs.lock().unwrap().clone();
        assert!(
            logs.iter()
                .any(|l| l.contains("npm ci ui failed") && l.contains("boom")),
            "{logs:?}"
        );
        assert!(!fx.cfg.workspace.join("cache").exists());
        assert!(!a.join("ui/node_modules").join(MARKER).exists());
        let ev = fx.events.lock().unwrap().clone();
        assert_eq!(ev[0]["data"]["ok"], false);
        assert_eq!(ev[0]["data"]["steps"][0]["status"], "failed");
    }

    #[tokio::test]
    async fn component_without_lockfile_skips_npm() {
        let mut fx = Fx::new("");
        let wt = fx.dir.join("wt/x");
        std::fs::create_dir_all(wt.join("svc")).unwrap();
        std::fs::write(wt.join("svc/mise.toml"), "").unwrap();
        let pg = fx.prepare(&wt).await.unwrap();
        assert_eq!(fx.npm_runs(), 0);
        assert!(pg.is_none());
    }

    #[tokio::test]
    async fn concurrent_runs_install_once() {
        let fx = Fx::new("sleep 1");
        let a = fx.worktree("a", "lock-1");
        let b = fx.worktree("b", "lock-1");
        let one = |wt: PathBuf| {
            let (cfg, reaper, locks, ctl) = (
                fx.cfg.clone(),
                fx.reaper.clone(),
                fx.locks.clone(),
                fx.ctl.clone(),
            );
            async move {
                let sink = |_: &'static str| -> LineSink { Arc::new(|_: &str| {}) };
                let event = |_: &str, _: Value| {};
                let ctx = Ctx {
                    reaper: &reaper,
                    cfg: &cfg,
                    locks: &locks,
                    repo: "o-r".into(),
                    env: vec![],
                    sink: &sink,
                    event: &event,
                };
                let mut ctl = ctl;
                run(&ctx, &mut ctl, &wt).await.unwrap();
            }
        };
        tokio::join!(one(a), one(b));
        assert_eq!(fx.npm_runs(), 1);
    }

    #[tokio::test]
    async fn step_timeout_fails_the_step_not_the_run() {
        let mut fx = Fx::new("sleep 30");
        fx.cfg.prepare_step_timeout = Duration::from_millis(300);
        let a = fx.worktree("a", "lock-1");
        fx.prepare(&a).await.unwrap();
        let logs = fx.logs.lock().unwrap().clone();
        assert!(logs.iter().any(|l| l.contains("timed out")), "{logs:?}");
        assert_eq!(fx.events.lock().unwrap()[0]["data"]["ok"], false);
    }

    #[tokio::test]
    async fn timing_output_never_holds_the_token() {
        let mut fx = Fx::new("echo \"token ghp_secret9\" >&2; exit 1");
        let a = fx.worktree("a", "lock-1");
        // The failure line carries stderr (the caller's sink redacts it);
        // the timing summary never does.
        fx.prepare(&a).await.unwrap();
        for l in fx
            .logs
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("prepare:"))
        {
            assert!(!l.contains("ghp_secret9"));
        }
    }

    #[test]
    fn prune_keeps_the_newest_two() {
        let d = tempfile::tempdir().unwrap();
        for (i, n) in ["h1", "h2", "h3", "h4.tmp"].iter().enumerate() {
            std::fs::create_dir(d.path().join(n)).unwrap();
            let t = std::time::SystemTime::now() - Duration::from_secs(100 - i as u64 * 10);
            std::fs::File::open(d.path().join(n))
                .unwrap()
                .set_modified(t)
                .unwrap();
        }
        prune(d.path(), "h3");
        assert!(!d.path().join("h1").exists());
        assert!(d.path().join("h2").exists());
        assert!(d.path().join("h3").exists());
        assert!(!d.path().join("h4.tmp").exists());
    }

    /// A fake `docker` backed by files in `dir`: `state` is the container state
    /// (absent = no container), `calls` logs the commands.
    fn fake_docker(dir: &Path, state: Option<&str>, run_ok: bool) -> PathBuf {
        let d = dir.join("fakedocker");
        if let Some(s) = state {
            std::fs::write(dir.join("state"), s).unwrap();
        }
        script(
            &d,
            &format!(
                "echo \"$@\" >> '{c}'\ncase \"$1\" in\n inspect) [ -f '{s}' ] && cat '{s}' || exit 1;;\n run) {run};;\n start) exit 0;;\n port) echo 127.0.0.1:55432; echo '[::]:55432';;\n exec) exit 0;;\nesac",
                c = dir.join("docker-calls").display(),
                s = dir.join("state").display(),
                run = if run_ok {
                    "exit 0"
                } else {
                    "echo nope >&2; exit 125"
                },
            ),
        );
        d
    }

    fn pg_wt(fx: &Fx, name: &str) -> PathBuf {
        let wt = fx.dir.join("wt").join(name);
        std::fs::create_dir_all(wt.join("workflows")).unwrap();
        wt
    }

    #[tokio::test]
    async fn pg_starts_once_and_reuses_a_running_container() {
        let mut fx = Fx::new("");
        fx.cfg.docker_bin = fake_docker(&fx.dir, None, true)
            .to_string_lossy()
            .into_owned();
        let wt = pg_wt(&fx, "p");
        let url = fx.prepare(&wt).await.unwrap().unwrap();
        assert_eq!(url, "postgres://postgres:postgres@127.0.0.1:55432/postgres");
        let calls = std::fs::read_to_string(fx.dir.join("docker-calls")).unwrap();
        assert!(calls.contains("run -d --name enzarb-test-pg -p 127.0.0.1:55432:5432"));
        assert!(calls.contains("max_connections=1000"));

        fx.cfg.docker_bin = fake_docker(&fx.dir, Some("running\n"), true)
            .to_string_lossy()
            .into_owned();
        std::fs::remove_file(fx.dir.join("docker-calls")).unwrap();
        let url = fx.prepare(&wt).await.unwrap();
        assert!(url.is_some());
        let calls = std::fs::read_to_string(fx.dir.join("docker-calls")).unwrap();
        assert!(!calls.contains("run -d"), "{calls}");
        assert!(!calls.contains("start "), "{calls}");

        fx.cfg.docker_bin = fake_docker(&fx.dir, Some("exited\n"), true)
            .to_string_lossy()
            .into_owned();
        std::fs::remove_file(fx.dir.join("docker-calls")).unwrap();
        fx.prepare(&wt).await.unwrap().unwrap();
        let calls = std::fs::read_to_string(fx.dir.join("docker-calls")).unwrap();
        assert!(calls.contains("start enzarb-test-pg"), "{calls}");
    }

    #[tokio::test]
    async fn pg_failure_is_not_fatal() {
        let mut fx = Fx::new("");
        std::fs::remove_file(fx.dir.join("state")).ok();
        fx.cfg.docker_bin = fake_docker(&fx.dir, None, false)
            .to_string_lossy()
            .into_owned();
        let wt = pg_wt(&fx, "p");
        assert_eq!(fx.prepare(&wt).await.unwrap(), None);
        let ev = fx.events.lock().unwrap().clone();
        assert_eq!(ev[0]["data"]["steps"][0]["step"], "postgres");
        assert_eq!(ev[0]["data"]["steps"][0]["status"], "failed");
    }
}
