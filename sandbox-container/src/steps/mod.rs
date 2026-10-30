//! The generic step executor (ADR-0022, `step_*` frames).
//!
//! sandbox-init knows nothing about workflows: each `step_start` runs one
//! command or agent step in the task's persistent workspaces
//! (`<ws>/tasks/<task_id>/<owner>/<repo>`), streams its session as
//! `session_event`s, and reports `step_finished` with exit, outputs, result
//! and git facts (which the engine re-verifies).
//!
//! On disk, per run (`<ws>/.enzarb/steps/<run_id>/`, root-only when root):
//! * `start.json`: the `step_start` with every secret blanked;
//! * `state.json`: phase, ACP session id and the push leases;
//! * `events.jsonl`: the redacted session events (`seq` dense from 1);
//! * `finished.json`: the last `step_finished`.
//!
//! Secrets live in memory only. After a sandbox restart a running step is
//! `interrupted`; `step_resume` then answers `error {code: "secrets_required"}`
//! and the engine re-sends `step_start` with the same `run_id`, which resumes
//! the step with the fresh secrets (agent: `session/load` of the saved
//! session plus a short "continue" turn; command: the command runs again).
//! A step stopped by `step_stop` in the same process lifetime resumes with
//! `step_resume` alone. A finished run whose `step_finished` carried an
//! `environment` failure (`disk_full`, `oom`) also resumes on `step_start` with
//! the same `run_id`, in the same workspace and agent session; any other
//! finished run is replayed.

pub mod agent;
pub mod environment;
pub mod git;

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use enzarb_proto::session::Session;
use enzarb_proto::step::{exit, session_kind};
use enzarb_proto::{Ctrl, Secret, SessionEvent, StepExec, StepFinished, StepPhase, StepStart};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::chat::filter::{redact_value, token_forms};
use crate::config::Config;
use crate::docker_prune::ActivityGate;
use crate::exec::{self, CmdSpec, Ctl, LineSink};
use crate::reaper::{ExitStatus, Reaper};
use crate::repos::{RepoLocks, repo_dir_name, shared_target_dir, target_dir_env};

/// Largest serialized payload of one session event; bigger ones keep head and tail.
pub const MAX_EVENT_PAYLOAD: usize = 1024 * 1024;
/// `step_finished` must fit one CTRL frame; outputs are dropped beyond this.
const MAX_FINISHED: usize = 900 * 1024;
/// The user turn sent when an interrupted agent session is resumed.
pub const CONTINUE_PROMPT: &str = "The sandbox restarted and your previous turn was interrupted. \
    Continue the task where you left off.";
/// The user turn sent when a turn ends without the step's result.
pub const NO_RESULT_PROMPT: &str = "Your turn ended but this step has no result yet. \
    If you started work in the background, wait for it in the foreground until it finishes. \
    Then submit the result with `submit_result`, or report that you are blocked and why, \
    as the schema allows.";
/// Error code of `step_resume` when the secrets are gone (sandbox restart).
pub const SECRETS_REQUIRED: &str = "secrets_required";

/// Where chat (and anything else outside steps) puts captured session events.
pub trait SessionSink: Send + Sync {
    /// Record an event of `run_id` (the payload is redacted again).
    fn event(&self, run_id: &str, kind: &str, payload: Value);
    /// The session ended for now (resumable).
    fn ended(&self, run_id: &str);
}

#[derive(Serialize, Deserialize)]
struct Persisted {
    phase: StepPhase,
    #[serde(default)]
    acp_session_id: Option<String>,
    #[serde(default)]
    leases: Vec<Option<String>>,
}

struct Control {
    steer: Option<mpsc::UnboundedSender<String>>,
    stop: watch::Sender<bool>,
}

struct RunInner {
    phase: StepPhase,
    /// With secrets; only in memory.
    start: Option<Arc<StepStart>>,
    /// Without secrets (from disk), for resumes.
    stored: Option<StepStart>,
    finished: Option<StepFinished>,
    acp_session: Option<String>,
    leases: Vec<Option<String>>,
    seq: u64,
    log: Option<File>,
    subs: HashMap<u64, Session>,
    secrets: Vec<String>,
    control: Option<Control>,
}

struct Run {
    id: String,
    dir: PathBuf,
    inner: Mutex<RunInner>,
    /// What the step left behind for the environment classifier.
    evidence: Mutex<environment::Evidence>,
}

/// A safe id (`run_id`, `task_id`).
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('.')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn now_rfc3339() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    // civil from days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_millis()
    )
}

/// Cap a payload at [`MAX_EVENT_PAYLOAD`], keeping head and tail.
pub fn cap_payload(payload: Value) -> (Value, bool) {
    let s = payload.to_string();
    if s.len() <= MAX_EVENT_PAYLOAD {
        return (payload, false);
    }
    let half = MAX_EVENT_PAYLOAD / 2 - 1024;
    let mut h = half;
    while !s.is_char_boundary(h) {
        h -= 1;
    }
    let mut t = s.len() - half;
    while !s.is_char_boundary(t) {
        t += 1;
    }
    (
        json!({"head": &s[..h], "tail": &s[t..], "bytes": s.len()}),
        true,
    )
}

/// Every form of the step's secrets that is redacted.
fn secret_forms(values: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = values
        .iter()
        .filter(|s| s.len() >= 4)
        .flat_map(|s| {
            let mut f = token_forms(s);
            f.push(s.to_string());
            f.push(git::basic_b64(s));
            f
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// The `step_start` without secrets (persisted).
fn blank(s: &StepStart) -> StepStart {
    let mut s = s.clone();
    for w in &mut s.workspaces {
        w.token = Secret::new("");
    }
    for v in s.env.values_mut() {
        *v = Secret::new("");
    }
    for v in s.files.values_mut() {
        *v = Secret::new("");
    }
    s.github_token = None;
    s
}

fn write_json(path: &Path, v: &impl Serialize) {
    if let Ok(b) = serde_json::to_vec(v)
        && let Err(e) = crate::capture::write_atomic(path, &b, 0o600)
    {
        tracing::warn!(path = %path.display(), error = %e, "cannot persist step state");
    }
}

fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn mkdir_private(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::create_dir_all(p);
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700));
}

impl Run {
    fn new(id: &str, dir: PathBuf) -> Self {
        Self {
            id: id.into(),
            dir,
            evidence: Mutex::new(environment::Evidence::default()),
            inner: Mutex::new(RunInner {
                phase: StepPhase::Running,
                start: None,
                stored: None,
                finished: None,
                acp_session: None,
                leases: Vec::new(),
                seq: 0,
                log: None,
                subs: HashMap::new(),
                secrets: Vec::new(),
                control: None,
            }),
        }
    }

    fn persist(&self, g: &RunInner) {
        write_json(
            &self.dir.join("state.json"),
            &Persisted {
                phase: g.phase,
                acp_session_id: g.acp_session.clone(),
                leases: g.leases.clone(),
            },
        );
    }

    fn open_log(&self, g: &mut RunInner) {
        if g.log.is_none() {
            use std::os::unix::fs::OpenOptionsExt;
            g.log = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(self.dir.join("events.jsonl"))
                .map_err(
                    |e| tracing::warn!(run = %self.id, error = %e, "cannot open the event log"),
                )
                .ok();
        }
    }

    /// Redact, cap, number, persist and push one event.
    fn emit(&self, kind: &str, payload: Value) {
        let mut g = self.inner.lock().unwrap();
        let payload = redact_value(payload, &g.secrets);
        let (payload, truncated) = cap_payload(payload);
        g.seq += 1;
        let ev = SessionEvent {
            run_id: self.id.clone(),
            seq: g.seq,
            at: now_rfc3339(),
            kind: kind.into(),
            payload,
            truncated,
        };
        self.open_log(&mut g);
        if let Some(f) = g.log.as_mut()
            && let Ok(mut line) = serde_json::to_vec(&ev)
        {
            line.push(b'\n');
            let _ = f.write_all(&line);
        }
        let m = Ctrl::SessionEvent(ev);
        g.subs.retain(|_, s| s.send_ctrl(&m).is_ok());
    }

    fn state_msg(&self, g: &RunInner) -> Ctrl {
        Ctrl::StepState {
            run_id: self.id.clone(),
            state: g.phase,
            result: g.finished.clone(),
        }
    }
}

struct Inner {
    cfg: Config,
    reaper: Arc<Reaper>,
    locks: RepoLocks,
    activity: ActivityGate,
    self_exe: PathBuf,
    runs: Mutex<HashMap<String, Arc<Run>>>,
    /// Set by [`StepManager::interrupt_all`]: steps that end now are interrupted by the
    /// sandbox's own shutdown and stay resumable (no `step_finished` is reported).
    shutting_down: std::sync::atomic::AtomicBool,
}

/// The step executor. Cheap to clone.
#[derive(Clone)]
pub struct StepManager {
    inner: Arc<Inner>,
}

/// A run `step_start` continues rather than replays: interrupted, or finished by an
/// environment failure.
fn resumable(g: &RunInner) -> bool {
    g.phase == StepPhase::Interrupted
        || (g.phase == StepPhase::Finished
            && g.finished.as_ref().is_some_and(|f| f.environment.is_some()))
}

impl StepManager {
    pub fn new(cfg: Config, reaper: Arc<Reaper>, locks: RepoLocks) -> Self {
        Self {
            inner: Arc::new(Inner {
                cfg,
                reaper,
                locks,
                activity: ActivityGate::default(),
                self_exe: std::env::current_exe().unwrap_or_else(|_| "sandbox-init".into()),
                runs: Mutex::default(),
                shutting_down: Default::default(),
            }),
        }
    }

    fn with(mut self, f: impl FnOnce(&mut Inner)) -> Self {
        f(Arc::get_mut(&mut self.inner).expect("configure before use"));
        self
    }

    /// Report running steps to the Docker pruner.
    pub fn with_activity(self, gate: ActivityGate) -> Self {
        self.with(|i| i.activity = gate)
    }

    /// The binary that serves `submit_result` (`sandbox-init` itself by default).
    pub fn with_self_exe(self, exe: PathBuf) -> Self {
        self.with(|i| i.self_exe = exe)
    }

    /// The sink chat writes its captured session to.
    pub fn sink(&self) -> Arc<dyn SessionSink> {
        self.inner.clone()
    }

    fn steps_dir(&self) -> PathBuf {
        self.inner.cfg.workspace.join(".enzarb/steps")
    }

    /// `/workspace/.enzarb` belongs to the sandbox user (0700): step git, clones and agent
    /// homes run as that user and must be able to enter it. Only `steps/` (per-run state with
    /// secrets) is root's. A fresh volume would otherwise get a root-owned `.enzarb` from the
    /// first `steps/` mkdir; an existing root-owned one is repaired.
    fn ensure_state_dir(&self) {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = self.inner.cfg.workspace.join(".enzarb");
        if let Err(e) = crate::privs::create_dir_all_mode(&dir, Some(0o700)) {
            tracing::warn!(path = %dir.display(), error = %e, "cannot create the state directory");
            return;
        }
        if crate::privs::is_root()
            && std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && m.uid() == 0)
        {
            match crate::privs::own(&dir) {
                Ok(()) => {
                    tracing::info!(path = %dir.display(), "gave the state directory to the sandbox user")
                }
                Err(e) => {
                    tracing::warn!(path = %dir.display(), error = %e, "cannot chown the state directory")
                }
            }
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
    }

    /// Load persisted runs; a step that was running is now `interrupted`.
    pub fn recover(&self) {
        self.ensure_state_dir();
        let Ok(rd) = std::fs::read_dir(self.steps_dir()) else {
            return;
        };
        let mut runs = self.inner.runs.lock().unwrap();
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) {
                continue;
            }
            let dir = e.path();
            let run = Run::new(&id, dir.clone());
            {
                let mut g = run.inner.lock().unwrap();
                g.stored = read_json(&dir.join("start.json"));
                g.finished = read_json(&dir.join("finished.json"));
                if let Some(p) = read_json::<Persisted>(&dir.join("state.json")) {
                    g.phase = p.phase;
                    g.acp_session = p.acp_session_id;
                    g.leases = p.leases;
                }
                g.seq = std::fs::File::open(dir.join("events.jsonl"))
                    .map(|f| std::io::BufReader::new(f).lines().count() as u64)
                    .unwrap_or(0);
                if g.phase == StepPhase::Running {
                    g.phase = StepPhase::Interrupted;
                    g.finished = Some(StepFinished {
                        run_id: id.clone(),
                        exit: exit::INTERRUPTED.into(),
                        outputs: Map::new(),
                        result: None,
                        git: vec![],
                        usage: None,
                        environment: None,
                        error: Some("the sandbox restarted".into()),
                    });
                    run.persist(&g);
                }
            }
            runs.insert(id, Arc::new(run));
        }
        tracing::info!(runs = runs.len(), "steps recovered");
    }

    fn get(&self, id: &str) -> Option<Arc<Run>> {
        self.inner.runs.lock().unwrap().get(id).cloned()
    }

    fn reply_error(session: &Session, run_id: &str, code: &str, msg: &str) {
        let _ = session.send_run_error(run_id, code, msg);
    }

    /// CTRL `step_start`. For a new run the starting session is subscribed to its frames
    /// (`session_event`, `step_state`, `step_finished`); an existing run is subscribed (after
    /// a replay) by `step_status`.
    pub fn start(&self, session_id: u64, session: &Session, start: StepStart) {
        let id = start.run_id.clone();
        if let Err(e) = validate(&start) {
            return Self::reply_error(session, &id, "invalid_request", &e);
        }
        let existing = self.get(&id);
        let run = match existing {
            Some(run) => {
                let (phase, resumable) = {
                    let g = run.inner.lock().unwrap();
                    (g.phase, resumable(&g))
                };
                // An existing run is subscribed by `step_status`, which replays the logged events
                // first, so the starting session sees them in order.
                if !resumable {
                    let _ = session.accept_step(&id, phase, true);
                    return;
                }
                // The engine's resume after a restart: same run, fresh secrets. Marked running
                // under the registry lock, so the workspace pruner sees it as busy.
                {
                    let _runs = self.inner.runs.lock().unwrap();
                    run.inner.lock().unwrap().phase = StepPhase::Running;
                }
                let _ = session.accept_step(&id, StepPhase::Running, true);
                self.launch(run, Arc::new(start), true);
                return;
            }
            None => {
                let dir = self.steps_dir().join(&id);
                mkdir_private(&self.steps_dir());
                mkdir_private(&dir);
                let run = Arc::new(Run::new(&id, dir));
                write_json(&run.dir.join("start.json"), &blank(&start));
                {
                    // Known from registration on, so the workspace pruner sees the task as busy.
                    let mut g = run.inner.lock().unwrap();
                    g.stored = Some(blank(&start));
                    run.persist(&g);
                }
                self.inner
                    .runs
                    .lock()
                    .unwrap()
                    .insert(id.clone(), run.clone());
                run
            }
        };
        run.inner
            .lock()
            .unwrap()
            .subs
            .insert(session_id, session.clone());
        let _ = session.accept_step(&id, StepPhase::Running, false);
        self.launch(run, Arc::new(start), false);
    }

    fn launch(&self, run: Arc<Run>, start: Arc<StepStart>, resume: bool) {
        let (stop_tx, stop_rx) = watch::channel(false);
        let (steer_tx, steer_rx) = mpsc::unbounded_channel();
        {
            let mut g = run.inner.lock().unwrap();
            g.phase = StepPhase::Running;
            if g.finished.take().is_some() || resume {
                // The old result must not be served again, not even after a restart.
                let _ = std::fs::remove_file(run.dir.join("finished.json"));
            }
            g.secrets = secret_forms(&start.secret_values());
            g.start = Some(start.clone());
            g.control = Some(Control {
                steer: matches!(start.exec, StepExec::Agent { .. }).then_some(steer_tx),
                stop: stop_tx,
            });
            if g.stored.is_none() {
                g.stored = Some(blank(&start));
            }
            run.persist(&g);
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _busy = inner.activity.begin();
            let fin = inner.work(&run, &start, resume, steer_rx, stop_rx).await;
            inner.finalize(&run, fin);
        });
    }

    /// CTRL `step_status`: replay, subscribe, answer `step_state`.
    pub fn status(&self, session_id: u64, session: &Session, run_id: &str, after_seq: u64) {
        let Some(run) = self.get(run_id) else {
            let _ = session.send_ctrl(&Ctrl::StepState {
                run_id: run_id.into(),
                state: StepPhase::Unknown,
                result: None,
            });
            return;
        };
        let mut g = run.inner.lock().unwrap();
        if after_seq < g.seq
            && let Ok(f) = File::open(run.dir.join("events.jsonl"))
        {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(ev) = serde_json::from_str::<SessionEvent>(&line)
                    && ev.seq > after_seq
                    && session.send_ctrl(&Ctrl::SessionEvent(ev)).is_err()
                {
                    return;
                }
            }
        }
        g.subs.insert(session_id, session.clone());
        let _ = session.send_ctrl(&run.state_msg(&g));
    }

    /// CTRL `step_steer`.
    pub fn steer(&self, session: &Session, run_id: &str, text: String) {
        let Some(run) = self.get(run_id) else {
            return Self::reply_error(session, run_id, "unknown_run", "no such step");
        };
        let tx = {
            let g = run.inner.lock().unwrap();
            g.control.as_ref().and_then(|c| c.steer.clone())
        };
        match tx {
            Some(tx) if tx.send(text.clone()).is_ok() => {
                run.emit(session_kind::STEER, json!({"text": text}));
            }
            _ => Self::reply_error(
                session,
                run_id,
                "invalid_request",
                "the step is not a running agent step",
            ),
        }
    }

    /// CTRL `step_stop`.
    pub fn stop(&self, session: &Session, run_id: &str) {
        let Some(run) = self.get(run_id) else {
            return Self::reply_error(session, run_id, "unknown_run", "no such step");
        };
        let g = run.inner.lock().unwrap();
        if let Some(c) = &g.control {
            let _ = c.stop.send(true);
        }
    }

    /// Sandbox shutdown (SIGTERM, a pod roll): stop every running step at its next checkpoint
    /// so it ends `interrupted` and stays resumable, waiting up to `grace`. Their result is
    /// not reported to the engine; after the restart the engine's `step_status` sees
    /// `interrupted` and re-sends `step_start`, which resumes the same agent session.
    pub async fn interrupt_all(&self, grace: Duration) {
        use std::sync::atomic::Ordering;
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        let running: Vec<Arc<Run>> = self
            .inner
            .runs
            .lock()
            .unwrap()
            .values()
            .filter(|r| r.inner.lock().unwrap().phase == StepPhase::Running)
            .cloned()
            .collect();
        if running.is_empty() {
            return;
        }
        tracing::info!(
            steps = running.len(),
            "interrupting running steps for shutdown"
        );
        for r in &running {
            if let Some(c) = &r.inner.lock().unwrap().control {
                let _ = c.stop.send(true);
            }
        }
        let deadline = tokio::time::Instant::now() + grace;
        while running
            .iter()
            .any(|r| r.inner.lock().unwrap().phase == StepPhase::Running)
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Runs `f` with the task ids of running steps while no step can start or end (the run
    /// registry is locked). Runs without a task (chat sessions reported through the sink)
    /// do not count; a new step's task is known from its registration on.
    pub fn with_busy_tasks<R>(&self, f: impl FnOnce(&std::collections::HashSet<String>) -> R) -> R {
        let runs = self.inner.runs.lock().unwrap();
        let mut busy = std::collections::HashSet::new();
        for r in runs.values() {
            let g = r.inner.lock().unwrap();
            if g.phase != StepPhase::Running {
                continue;
            }
            if let Some(s) = g.start.as_deref().or(g.stored.as_ref()) {
                busy.insert(s.task_id.clone());
            }
        }
        f(&busy)
    }

    /// Runs `f` with the shared build dirs (see [`shared_target_dir`]) of the repos of running
    /// steps while no step can start or end.
    pub fn with_busy_repos<R>(
        &self,
        workspace: &Path,
        f: impl FnOnce(&std::collections::HashSet<PathBuf>) -> R,
    ) -> R {
        let runs = self.inner.runs.lock().unwrap();
        let mut busy = std::collections::HashSet::new();
        for r in runs.values() {
            let g = r.inner.lock().unwrap();
            if g.phase != StepPhase::Running {
                continue;
            }
            if let Some(s) = g.start.as_deref().or(g.stored.as_ref()) {
                busy.extend(
                    s.workspaces
                        .iter()
                        .filter_map(|w| shared_target_dir(workspace, &w.repo)),
                );
            }
        }
        f(&busy)
    }

    /// CTRL `step_resume`.
    pub fn resume(&self, session: &Session, run_id: &str) {
        let Some(run) = self.get(run_id) else {
            return Self::reply_error(session, run_id, "unknown_run", "no such step");
        };
        let (phase, start) = {
            let g = run.inner.lock().unwrap();
            (g.phase, g.start.clone())
        };
        match (phase, start) {
            (StepPhase::Interrupted, Some(start)) => self.launch(run, start, true),
            (StepPhase::Interrupted, None) => Self::reply_error(
                session,
                run_id,
                SECRETS_REQUIRED,
                "the sandbox restarted: send step_start with the same run_id to resume",
            ),
            _ => {}
        }
    }

    /// A mux session ended: drop its subscriptions.
    pub fn session_closed(&self, session_id: u64) {
        for run in self.inner.runs.lock().unwrap().values() {
            run.inner.lock().unwrap().subs.remove(&session_id);
        }
    }
}

fn validate(s: &StepStart) -> Result<(), String> {
    if !valid_id(&s.run_id) || !valid_id(&s.task_id) {
        return Err("run_id and task_id must be [A-Za-z0-9._-]{1,128}".into());
    }
    for w in &s.workspaces {
        if !git::valid_repo(&w.repo) {
            return Err(format!("invalid repo {:?}", w.repo));
        }
        if !git::valid_branch(&w.branch) || !git::valid_branch(&w.base) {
            return Err(format!("invalid branch of {}", w.repo));
        }
        if w.start_ref
            .as_deref()
            .is_some_and(|r| !git::valid_start_ref(r))
        {
            return Err(format!("invalid start_ref of {}", w.repo));
        }
        crate::netgit::scheme(&w.url)?;
    }
    for name in s.env.keys() {
        let ok = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !ok {
            return Err(format!("invalid env name {name:?}"));
        }
    }
    for f in s.files.keys() {
        safe_rel(f).ok_or_else(|| format!("invalid secret file path {f:?}"))?;
    }
    for o in s.outputs.values() {
        safe_rel(&o.file).ok_or_else(|| format!("invalid output path {:?}", o.file))?;
    }
    if let StepExec::Agent { runtime, ez, .. } = &s.exec
        && ez.is_none()
        && (runtime.command.is_empty() || runtime.protocol != "acp")
    {
        return Err(format!(
            "unsupported runtime {:?} (protocol {:?}): only acp with a command",
            runtime.name, runtime.protocol
        ));
    }
    if let StepExec::Agent { ez: Some(ez), .. } = &s.exec {
        for f in ez.instruction_files.keys() {
            let ok = safe_rel(f).is_some_and(|p| {
                !p.components()
                    .any(|c| c.as_os_str() == ".git" || c.as_os_str() == ".enzarb")
            });
            if !ok {
                return Err(format!("invalid ez instruction file path {f:?}"));
            }
        }
    }
    Ok(())
}

/// A relative path without `..`.
/// Writes `step_start.files` under `cwd` and lists them in the repository's local exclude file.
fn write_secret_files(cwd: &Path, start: &StepStart) -> Vec<std::path::PathBuf> {
    let mut written = Vec::new();
    for (rel, v) in &start.files {
        let Some(r) = safe_rel(rel) else { continue };
        let p = cwd.join(r);
        if let Err(e) = crate::capture::write_atomic(&p, v.expose().as_bytes(), 0o600) {
            tracing::warn!(path = %rel, error = %e, "cannot write secret file");
            continue;
        }
        let exclude = cwd.join(".git/info/exclude");
        if exclude.parent().is_some_and(Path::is_dir) {
            let line = format!("/{}", r.display());
            let cur = std::fs::read_to_string(&exclude).unwrap_or_default();
            if !cur.lines().any(|l| l == line) {
                use std::io::Write as _;
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&exclude)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
        written.push(p);
    }
    written
}

fn safe_rel(p: &str) -> Option<&Path> {
    let path = Path::new(p);
    (!p.is_empty()
        && path.is_relative()
        && path.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }))
    .then_some(path)
}

/// Batches stdout/stderr lines into events (one per kind run, at most every 250 ms).
struct LineBatcher {
    run: Arc<Run>,
    buf: Mutex<Option<(&'static str, String, Instant)>>,
}

impl LineBatcher {
    fn push(&self, kind: &'static str, line: &str) {
        let mut b = self.buf.lock().unwrap();
        let flush = match &*b {
            Some((k, s, at)) => {
                *k != kind || s.len() > 16 * 1024 || at.elapsed() > Duration::from_millis(250)
            }
            None => false,
        };
        if flush && let Some((k, s, _)) = b.take() {
            self.run.emit(k, json!({"text": s}));
        }
        match &mut *b {
            Some((_, s, _)) => {
                s.push_str(line);
                s.push('\n');
            }
            None => *b = Some((kind, format!("{line}\n"), Instant::now())),
        }
    }

    fn flush(&self) {
        if let Some((k, s, _)) = self.buf.lock().unwrap().take() {
            self.run.emit(k, json!({"text": s}));
        }
    }
}

fn finished(run_id: &str, exit: &str, error: Option<String>) -> StepFinished {
    StepFinished {
        run_id: run_id.into(),
        exit: exit.into(),
        outputs: Map::new(),
        result: None,
        git: vec![],
        usage: None,
        environment: None,
        error,
    }
}

/// A git credential helper answering `get` with `$GH_TOKEN` (evaluated by git's shell, never
/// stored).
const GH_TOKEN_HELPER: &str =
    "!f() { test \"$1\" = get && echo username=x-access-token && echo \"password=$GH_TOKEN\"; }; f";

impl Inner {
    fn base_env(&self, start: &StepStart) -> Vec<(String, String)> {
        let mut env: Vec<(String, String)> = start
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.expose().to_string()))
            .collect();
        if let Some(t) = &start.github_token {
            env.push(("GH_TOKEN".into(), t.expose().into()));
            env.push(("GH_NO_UPDATE_NOTIFIER".into(), "1".into()));
            env.push(("GH_PROMPT_DISABLED".into(), "1".into()));
            // The step's own git calls (`git fetch` in a workspace) authenticate with the same
            // token through an env-only credential helper: it reads `$GH_TOKEN` when git asks,
            // so the token is never written to git config or disk.
            if !env.iter().any(|(k, _)| k == "GIT_CONFIG_COUNT") {
                env.push(("GIT_CONFIG_COUNT".into(), "1".into()));
                env.push((
                    "GIT_CONFIG_KEY_0".into(),
                    "credential.https://github.com.helper".into(),
                ));
                env.push(("GIT_CONFIG_VALUE_0".into(), GH_TOKEN_HELPER.into()));
            }
        }
        env.push(("GIT_AUTHOR_NAME".into(), self.cfg.git_name.clone()));
        env.push(("GIT_AUTHOR_EMAIL".into(), self.cfg.git_email.clone()));
        env.push(("GIT_COMMITTER_NAME".into(), self.cfg.git_name.clone()));
        env.push(("GIT_COMMITTER_EMAIL".into(), self.cfg.git_email.clone()));
        // One Cargo build dir per repo (the first workspace's, like `cwd`), shared by all tasks.
        let url = start.workspaces.first().map(|w| w.repo.as_str());
        env.extend(target_dir_env(&self.cfg.workspace, url, &env));
        env
    }

    fn log_sink(run: &Arc<Run>, source: &'static str) -> LineSink {
        let run = run.clone();
        Arc::new(move |line: &str| {
            run.emit(
                session_kind::SYSTEM,
                json!({"source": source, "line": line}),
            );
        })
    }

    async fn mise_install(
        &self,
        run: &Arc<Run>,
        ctl: &mut Ctl,
        cwd: &Path,
        tools: &[String],
    ) -> Result<(), String> {
        if tools.is_empty() || self.cfg.mise_bin.is_empty() {
            return Ok(());
        }
        let mut args = vec!["install".to_string()];
        args.extend(tools.iter().cloned());
        let mut spec = CmdSpec {
            program: self.cfg.mise_bin.clone(),
            args,
            ..CmdSpec::default()
        };
        spec.cwd = Some(cwd.to_path_buf());
        let out = exec::run(&self.reaper, spec, ctl, Some(Self::log_sink(run, "mise")))
            .await
            .map_err(|e| format!("cannot run mise: {e}"))?;
        if out.success() {
            Ok(())
        } else {
            Err(format!("mise install failed: {}", out.stderr.trim()))
        }
    }

    /// argv through `mise exec <tools> --` when tools and mise are set.
    fn with_mise(&self, tools: &[String], argv: Vec<String>) -> (String, Vec<String>) {
        if tools.is_empty() || self.cfg.mise_bin.is_empty() {
            let mut it = argv.into_iter();
            let bin = it.next().unwrap_or_default();
            return (bin, it.collect());
        }
        let mut args = vec!["exec".to_string()];
        args.extend(tools.iter().cloned());
        args.push("--".into());
        args.extend(argv);
        (self.cfg.mise_bin.clone(), args)
    }

    fn memory_events(&self) -> PathBuf {
        self.cfg.cgroup_root.join("memory.events")
    }

    async fn work(
        self: &Arc<Self>,
        run: &Arc<Run>,
        start: &StepStart,
        resume: bool,
        mut steer: mpsc::UnboundedReceiver<String>,
        stop: watch::Receiver<bool>,
    ) -> StepFinished {
        let mut ctl = Ctl {
            cancel: stop,
            deadline: None,
        };
        let id = &start.run_id;
        *run.evidence.lock().unwrap() = environment::Evidence {
            oom_before: environment::oom_kill_count(&self.memory_events()),
            pressure_start: environment::system_pressure(),
            ..environment::Evidence::default()
        };
        run.emit(
            session_kind::SYSTEM,
            json!({"event": if resume { "resumed" } else { "started" }, "attempt": start.attempt}),
        );
        // Workspaces.
        let g = git::Git {
            cfg: &self.cfg,
            reaper: &self.reaper,
            log: Self::log_sink(run, "git"),
        };
        let old_leases = run.inner.lock().unwrap().leases.clone();
        let mut dirs = Vec::new();
        let mut leases = Vec::new();
        for (i, w) in start.workspaces.iter().enumerate() {
            let dir = git::workspace_dir(&self.cfg.workspace, &start.task_id, &w.repo);
            let lock = self.locks.get(&dir);
            let _held = lock.lock().await;
            let prev = old_leases.get(i).cloned().flatten();
            let prev = if resume { prev } else { None };
            match g.prepare(&mut ctl, w, &dir, prev.as_deref()).await {
                Ok(l) => leases.push(Some(l)),
                Err(e) => {
                    let e = redact_value(json!(e), &run.inner.lock().unwrap().secrets);
                    if *ctl.cancel.borrow() {
                        return finished(id, exit::INTERRUPTED, None);
                    }
                    run.evidence.lock().unwrap().phase = Some(environment::Phase::Fetch);
                    return finished(id, exit::FAILURE, e.as_str().map(String::from));
                }
            }
            self.link_repo(&w.url, &dir);
            dirs.push(dir);
        }
        {
            let mut gi = run.inner.lock().unwrap();
            gi.leases = leases.clone();
            run.persist(&gi);
        }
        let cwd = match dirs.first() {
            Some(d) => d.clone(),
            None => {
                let d = self.cfg.workspace.join("tasks").join(&start.task_id);
                let _ = crate::privs::create_dir_all(&d);
                d
            }
        };
        // Secret files: 0600, kept out of commits, removed when the step ends.
        let secret_files = write_secret_files(&cwd, start);
        // ez steps: the pinned modes and instruction files win over the branch's own.
        if let StepExec::Agent { ez: Some(ez), .. } = &start.exec {
            let mut files: Vec<(String, &str)> =
                vec![(".ez/modes.toml".into(), ez.modes_toml.as_str())];
            files.extend(
                ez.instruction_files
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str())),
            );
            if let Err(e) = g.pin_files(&mut ctl, &cwd, &files).await {
                return finished(id, exit::FAILURE, Some(e));
            }
        }
        // Execute.
        let fin = match &start.exec {
            StepExec::Command { run: cmd, tools } => {
                self.command(run, start, &cwd, cmd, tools, &mut ctl).await
            }
            StepExec::Agent { .. } => {
                self.agent(run, start, &cwd, resume, &mut steer, &mut ctl)
                    .await
            }
        };
        for p in &secret_files {
            let _ = std::fs::remove_file(p);
        }
        let mut fin = fin;
        if fin.exit == exit::INTERRUPTED {
            return fin;
        }
        // Outputs.
        for (name, o) in &start.outputs {
            let Some(rel) = safe_rel(&o.file) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(cwd.join(rel)) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let v = if o.format.as_deref() == Some("json") {
                match serde_json::from_str::<Value>(&text) {
                    Ok(v) => v,
                    Err(_) => continue,
                }
            } else {
                Value::String(text)
            };
            fin.outputs.insert(name.clone(), v);
        }
        // Git.
        let mut ctl = Ctl {
            cancel: watch::channel(false).1,
            // Each network attempt has its own cap (git.rs); the finalization as a whole has none.
            deadline: None,
        };
        for (i, w) in start.workspaces.iter().enumerate() {
            if w.mode != enzarb_proto::Mode::Write {
                continue;
            }
            let dir = &dirs[i];
            let lock = self.locks.get(dir);
            let _held = lock.lock().await;
            let lease = leases[i].clone().unwrap_or_default();
            match g.finish(&mut ctl, w, dir, &lease, id).await {
                Ok(f) => fin.git.push(f),
                Err((f, e)) => {
                    fin.git.push(f);
                    fin.exit = exit::FAILURE.into();
                    let e = redact_value(json!(e), &run.inner.lock().unwrap().secrets);
                    let e = e.as_str().unwrap_or_default().to_string();
                    fin.error = Some(match fin.error.take() {
                        Some(prev) => format!("{prev}; {e}"),
                        None => e,
                    });
                }
            }
        }
        fin
    }

    /// Keep `<ws>/repos/<name>` (what chat opens) pointing at the latest task
    /// workspace of the repo. A real clone there is left alone.
    fn link_repo(&self, url: &str, dir: &Path) {
        let repos = self.cfg.workspace.join("repos");
        let _ = crate::privs::create_dir_all(&repos);
        let link = repos.join(repo_dir_name(url));
        let is_link = std::fs::symlink_metadata(&link).map(|m| m.file_type().is_symlink());
        if matches!(is_link, Ok(false)) {
            return;
        }
        let tmp = repos.join(format!(".{}.tmp", repo_dir_name(url)));
        let _ = std::fs::remove_file(&tmp);
        if std::os::unix::fs::symlink(dir, &tmp).is_ok() {
            let _ = std::fs::rename(&tmp, &link);
        }
    }

    async fn command(
        &self,
        run: &Arc<Run>,
        start: &StepStart,
        cwd: &Path,
        cmd: &str,
        tools: &[String],
        ctl: &mut Ctl,
    ) -> StepFinished {
        let id = &start.run_id;
        run.emit(session_kind::SYSTEM, json!({"command": cmd}));
        if let Err(e) = self.mise_install(run, ctl, cwd, tools).await {
            let ex = if *ctl.cancel.borrow() {
                exit::INTERRUPTED
            } else {
                run.evidence.lock().unwrap().phase = Some(environment::Phase::Install);
                exit::FAILURE
            };
            return finished(id, ex, Some(e));
        }
        let (bin, args) = self.with_mise(tools, vec!["sh".into(), "-c".into(), cmd.to_string()]);
        let spec = CmdSpec {
            program: bin,
            args,
            cwd: Some(cwd.to_path_buf()),
            env: self.base_env(start),
            ..CmdSpec::default()
        };
        let batch = Arc::new(LineBatcher {
            run: run.clone(),
            buf: Mutex::new(None),
        });
        let (b1, b2) = (batch.clone(), batch.clone());
        let ticker = {
            let b = batch.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    b.flush();
                }
            })
        };
        let out = exec::run_lines(
            &self.reaper,
            spec,
            ctl,
            Some(Arc::new(move |l: &str| b1.push(session_kind::STDOUT, l))),
            Some(Arc::new(move |l: &str| b2.push(session_kind::STDERR, l))),
        )
        .await;
        ticker.abort();
        batch.flush();
        {
            let mut ev = run.evidence.lock().unwrap();
            match &out {
                Err(e) => ev.text = e.to_string(),
                Ok(o) => {
                    ev.text = o.stderr.clone();
                    ev.we_killed = o.stop.is_some();
                    ev.timed_out = o.stop == Some(exec::Stop::TimedOut);
                    if ev.timed_out {
                        ev.pressure_kill = environment::system_pressure();
                    }
                    ev.sigkilled = matches!(
                        o.status,
                        Some(ExitStatus::Signaled(9) | ExitStatus::Exited(137))
                    );
                }
            }
        }
        match out {
            Err(e) => finished(
                id,
                exit::FAILURE,
                Some(format!("cannot run the command: {e}")),
            ),
            Ok(o) if o.stop == Some(exec::Stop::Cancelled) => finished(id, exit::INTERRUPTED, None),
            Ok(o) if o.stop == Some(exec::Stop::TimedOut) => {
                finished(id, exit::FAILURE, Some("the step timed out".into()))
            }
            Ok(o) if o.success() => finished(id, exit::SUCCESS, None),
            Ok(o) => finished(
                id,
                exit::FAILURE,
                Some(match o.code() {
                    Some(c) => format!("the command exited with code {c}"),
                    None => "the command was killed".into(),
                }),
            ),
        }
    }

    async fn agent(
        &self,
        run: &Arc<Run>,
        start: &StepStart,
        cwd: &Path,
        resume: bool,
        steer: &mut mpsc::UnboundedReceiver<String>,
        ctl: &mut Ctl,
    ) -> StepFinished {
        let id = &start.run_id;
        let StepExec::Agent {
            runtime,
            model,
            prompt,
            result_schema,
            mode,
            ez,
        } = &start.exec
        else {
            unreachable!()
        };
        let install: &[String] = if ez.is_some() { &[] } else { &runtime.install };
        if let Err(e) = self.mise_install(run, ctl, cwd, install).await {
            let ex = if *ctl.cancel.borrow() {
                exit::INTERRUPTED
            } else {
                run.evidence.lock().unwrap().phase = Some(environment::Phase::Install);
                exit::FAILURE
            };
            return finished(id, ex, Some(e));
        }
        let mut env = self.base_env(start);
        env.extend(runtime.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        if let Some(ez) = ez {
            match self.ez_env(id, model.as_deref(), ez) {
                Ok(e) => env.extend(e),
                Err(e) => return finished(id, exit::FAILURE, Some(e)),
            }
        }
        let home = self.cfg.workspace.join(".enzarb/step-home/.claude");
        let _ = crate::privs::create_dir_all_mode(&home, Some(0o700));
        env.push((
            "CLAUDE_CONFIG_DIR".into(),
            home.to_string_lossy().into_owned(),
        ));
        let has = |k: &str| env.iter().any(|(a, _)| a == k);
        if !has("CLAUDE_CODE_OAUTH_TOKEN")
            && !has("ANTHROPIC_API_KEY")
            && let Some(t) = crate::capture::read_legacy_valid(&self.cfg.claude_token_file)
        {
            run.inner
                .lock()
                .unwrap()
                .secrets
                .extend(secret_forms(&[&t]));
            env.push(("CLAUDE_CODE_OAUTH_TOKEN".into(), t));
        }
        if let Some(m) = model {
            env.push(("ANTHROPIC_MODEL".into(), m.clone()));
        }
        // submit_result
        let mut mcp_servers = Vec::new();
        let result_file = self
            .cfg
            .workspace
            .join(".enzarb/step-mcp")
            .join(id)
            .join("result.json");
        if let Some(schema) = result_schema {
            let dir = result_file.parent().unwrap().to_path_buf();
            let _ = crate::privs::create_dir_all_mode(&dir, Some(0o700));
            let schema_file = dir.join("schema.json");
            let _ = std::fs::write(&schema_file, schema.to_string());
            let _ = crate::privs::own(&schema_file);
            if !resume {
                let _ = std::fs::remove_file(&result_file);
            }
            mcp_servers.push(json!({
                "name": crate::mcp::SERVER_NAME,
                "command": self.self_exe,
                "args": [crate::mcp::SUBCOMMAND, schema_file, result_file],
                "env": [],
            }));
        }
        let saved = run.inner.lock().unwrap().acp_session.clone();
        let resume_sid = if resume { saved } else { None };
        let first = if resume_sid.is_some() {
            CONTINUE_PROMPT.to_string()
        } else {
            prompt.clone()
        };
        let (bin, args) = if ez.is_some() {
            let mut c = self.cfg.clone();
            if c.chat_agent != crate::config::ChatAgent::Ez {
                c.chat_agent = crate::config::ChatAgent::Ez;
                c.acp_bin = "ez".into();
                c.chat_runtime_tool.clear();
            }
            match crate::chat::chat_command(&c) {
                Some(b) => b,
                None => {
                    return finished(id, exit::FAILURE, Some("ez is not installed".into()));
                }
            }
        } else {
            self.with_mise(&runtime.install, runtime.command.clone())
        };
        let spec = agent::AgentSpec {
            mode: mode.clone(),
            bin,
            args,
            cwd: cwd.to_path_buf(),
            env,
            prompt: first,
            resume: resume_sid,
            mcp_servers,
            cancel_grace: self.cfg.cancel_grace,
            result_ready: result_schema.as_ref().map(|schema| {
                let schema = schema.clone();
                let file = result_file.clone();
                Box::new(move || {
                    read_json::<Value>(&file)
                        .is_some_and(|v| crate::mcp::validate(&schema, &v).is_empty())
                }) as Box<dyn Fn() -> bool + Send + Sync>
            }),
            continuation: NO_RESULT_PROMPT.to_string(),
        };
        let r = run.clone();
        let emit = move |kind: &str, payload: Value| r.emit(kind, payload);
        let r = run.clone();
        let on_session = move |sid: &str| {
            let mut g = r.inner.lock().unwrap();
            g.acp_session = Some(sid.to_string());
            r.persist(&g);
        };
        let end = agent::run(
            &self.reaper,
            spec,
            &emit,
            &on_session,
            steer,
            ctl,
            Self::log_sink(run, "agent"),
        )
        .await;
        let mut fin = finished(id, end.exit, end.error);
        fin.usage = end.usage;
        if let Some(schema) = result_schema
            && let Some(v) = read_json::<Value>(&result_file)
        {
            if crate::mcp::validate(schema, &v).is_empty() {
                fin.result = Some(v);
            } else {
                run.emit(
                    session_kind::SYSTEM,
                    json!({"event": "invalid_result_file"}),
                );
            }
        }
        // A node that declares a `result` must deliver one (ADR-0029): fail here, clearly,
        // instead of succeeding with `result: null` and failing at a later node.
        if result_schema.is_some() && fin.result.is_none() && fin.exit == exit::SUCCESS {
            fin.exit = exit::FAILURE.into();
            fin.error = Some("agent finished without submitting a result".into());
        }
        fin
    }

    /// Environment of an `ez_steps` agent: a per-node `EZ_CONFIG_DIR` (the profile like
    /// chats get, plus the pinned `modes.toml`), `EZ_SESSIONS_DIR` (kept across resumes of the
    /// node) and `EZ_UNATTENDED` (no AskUserQuestion, no SwitchMode).
    fn ez_env(
        &self,
        run_id: &str,
        model: Option<&str>,
        ez: &enzarb_proto::EzSpec,
    ) -> Result<Vec<(String, String)>, String> {
        let base = self.cfg.workspace.join(".enzarb/ez").join(run_id);
        let (conf, sessions) = (base.join("config"), base.join("sessions"));
        for d in [&conf, &sessions] {
            crate::privs::create_dir_all_mode(d, Some(0o700))
                .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        let model = model.unwrap_or(&self.cfg.chat_model);
        let write = |name: &str, text: &str| {
            crate::capture::write_atomic(&conf.join(name), text.as_bytes(), 0o600)
                .map_err(|e| format!("cannot write the ez {name}: {e}"))
        };
        write("config.toml", &crate::chat::ez_config(model))?;
        write("modes.toml", &ez.modes_toml)?;
        Ok(vec![
            ("EZ_CONFIG_DIR".into(), conf.to_string_lossy().into_owned()),
            (
                "EZ_SESSIONS_DIR".into(),
                sessions.to_string_lossy().into_owned(),
            ),
            ("EZ_UNATTENDED".into(), "1".into()),
        ])
    }

    /// Disk full, OOM, toolchain/network or load behind a failed step (`environment::classify`).
    fn classify(
        &self,
        run: &Arc<Run>,
        fin: &StepFinished,
    ) -> Option<enzarb_proto::StepEnvironment> {
        let mut ev = run.evidence.lock().unwrap().clone();
        ev.failed = true;
        if let Some(e) = &fin.error {
            ev.text.push('\n');
            ev.text.push_str(e);
        }
        ev.oom_after = environment::oom_kill_count(&self.memory_events());
        ev.workspace_full = environment::workspace_full(&self.cfg.workspace);
        environment::classify(&ev)
    }

    fn finalize(&self, run: &Arc<Run>, mut fin: StepFinished) {
        if fin.exit == exit::FAILURE && fin.environment.is_none() {
            fin.environment = self.classify(run, &fin);
        }
        fit(&mut fin);
        run.emit(
            session_kind::SYSTEM,
            json!({"event": "finished", "exit": fin.exit, "error": fin.error, "environment": fin.environment}),
        );
        let mut g = run.inner.lock().unwrap();
        g.phase = if fin.exit == exit::INTERRUPTED {
            StepPhase::Interrupted
        } else {
            StepPhase::Finished
        };
        g.control = None;
        if g.phase == StepPhase::Finished {
            // Done for good: the secrets go.
            g.start = None;
        }
        if g.phase == StepPhase::Interrupted
            && self.shutting_down.load(std::sync::atomic::Ordering::SeqCst)
        {
            // Interrupted by our own shutdown: resumable, so neither a result nor
            // `step_finished` (the engine resumes it after the restart).
            run.persist(&g);
            tracing::info!(run = %run.id, "step interrupted by shutdown");
            return;
        }
        write_json(&run.dir.join("finished.json"), &fin);
        run.persist(&g);
        let m = Ctrl::StepFinished(fin.clone());
        g.finished = Some(fin);
        g.subs.retain(|_, s| s.send_ctrl(&m).is_ok());
        tracing::info!(run = %run.id, phase = ?g.phase, "step ended");
    }
}

/// Drop the largest outputs until `step_finished` fits one frame.
fn fit(fin: &mut StepFinished) {
    let size = |f: &StepFinished| serde_json::to_vec(f).map(|v| v.len()).unwrap_or(0);
    while size(fin) > MAX_FINISHED {
        let Some(big) = fin
            .outputs
            .iter()
            .max_by_key(|(_, v)| v.to_string().len())
            .map(|(k, _)| k.clone())
        else {
            fin.result = None;
            break;
        };
        fin.outputs.remove(&big);
        let note = format!("output {big:?} was too large to report");
        fin.error = Some(match fin.error.take() {
            Some(e) => format!("{e}; {note}"),
            None => note,
        });
    }
}

impl SessionSink for Inner {
    fn event(&self, run_id: &str, kind: &str, payload: Value) {
        if !valid_id(run_id) {
            return;
        }
        let run = {
            let mut runs = self.runs.lock().unwrap();
            runs.entry(run_id.to_string())
                .or_insert_with(|| {
                    let dir = self.cfg.workspace.join(".enzarb/steps").join(run_id);
                    mkdir_private(&self.cfg.workspace.join(".enzarb/steps"));
                    mkdir_private(&dir);
                    Arc::new(Run::new(run_id, dir))
                })
                .clone()
        };
        {
            let mut g = run.inner.lock().unwrap();
            if g.phase != StepPhase::Running {
                g.phase = StepPhase::Running;
                run.persist(&g);
            }
        }
        run.emit(kind, payload);
    }

    fn ended(&self, run_id: &str) {
        if let Some(run) = self.runs.lock().unwrap().get(run_id) {
            let mut g = run.inner.lock().unwrap();
            g.phase = StepPhase::Interrupted;
            run.persist(&g);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caps_keep_head_and_tail() {
        let big = json!({"text": "a".repeat(3 * MAX_EVENT_PAYLOAD)});
        let (v, t) = cap_payload(big);
        assert!(t);
        assert!(v.to_string().len() <= MAX_EVENT_PAYLOAD);
        assert!(v["head"].as_str().unwrap().starts_with("{\"text\""));
        let (v, t) = cap_payload(json!({"x": 1}));
        assert!(!t);
        assert_eq!(v, json!({"x": 1}));
    }

    #[test]
    fn ids_and_paths() {
        assert!(valid_id("run-1.a_b"));
        assert!(!valid_id("../x") && !valid_id("") && !valid_id(".x") && !valid_id("a/b"));
        assert!(safe_rel("out/plan.md").is_some());
        assert!(safe_rel("../x").is_none() && safe_rel("/etc/passwd").is_none());
    }

    #[test]
    fn timestamps_are_rfc3339() {
        let t = now_rfc3339();
        assert_eq!(t.len(), 24, "{t}");
        assert!(t.starts_with("20") && t.ends_with('Z'));
    }
}
