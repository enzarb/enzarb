//! Running helper processes (`git`) under the reaper.
//!
//! Children are spawned with std `Command` plus `setsid` (so they lead their
//! own process group) through [`Reaper::spawn_with`]; their exit status comes
//! from the reaper, never from `wait`. Cancellation and timeouts kill the
//! whole process group.

use std::io::{self, BufRead, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

use crate::reaper::{ExitStatus, Reaper};

/// Max bytes of stdout kept.
const STDOUT_CAP: u64 = 16 * 1024 * 1024;
/// Max bytes of stderr kept (the tail).
const STDERR_TAIL: usize = 16 * 1024;
/// How long to wait for the pipes to drain after the process ended.
const DRAIN_WAIT: Duration = Duration::from_secs(3);
/// Time between SIGTERM and SIGKILL when stopping a process.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// A process to run.
#[derive(Default)]
pub struct CmdSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// Start from an empty environment (only `env`), not sandbox-init's.
    pub env_clear: bool,
    /// Written to stdin, which is then closed (stdin is `/dev/null` when `None`).
    pub stdin: Option<Vec<u8>>,
}

impl CmdSpec {
    pub fn new(program: &str, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            ..Self::default()
        }
    }
}

/// Why a process was stopped before it exited on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Cancelled,
    TimedOut,
}

/// Cancellation and deadline of a run.
#[derive(Clone)]
pub struct Ctl {
    pub cancel: watch::Receiver<bool>,
    pub deadline: Instant,
}

impl Ctl {
    pub async fn cancelled(&mut self) {
        loop {
            if *self.cancel.borrow() {
                return;
            }
            if self.cancel.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }
}

impl Ctl {
    /// Resolves when the run is cancelled or past its deadline.
    pub async fn stopped(&mut self) -> Stop {
        let deadline = self.deadline;
        tokio::select! {
            () = self.cancelled() => Stop::Cancelled,
            () = tokio::time::sleep_until(deadline) => Stop::TimedOut,
        }
    }
}

/// Result of [`run`].
#[derive(Debug)]
pub struct Output {
    pub status: Option<ExitStatus>,
    pub stop: Option<Stop>,
    pub stdout: Vec<u8>,
    /// Tail of stderr.
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.stop.is_none() && self.status == Some(ExitStatus::Exited(0))
    }
    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }
}

/// Receives each stderr line as it is produced.
pub type LineSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Run `spec` to completion (or until cancelled / past the deadline).
pub async fn run(
    reaper: &Reaper,
    spec: CmdSpec,
    ctl: &mut Ctl,
    on_stderr: Option<LineSink>,
) -> io::Result<Output> {
    run_lines(reaper, spec, ctl, None, on_stderr).await
}

/// Like [`run`], and with `on_stdout` set, each stdout line goes to it as it is
/// produced (and only the last [`STDERR_TAIL`] bytes of stdout are kept).
pub async fn run_lines(
    reaper: &Reaper,
    spec: CmdSpec,
    ctl: &mut Ctl,
    on_stdout: Option<LineSink>,
    on_stderr: Option<LineSink>,
) -> io::Result<Output> {
    let mut cmd = Command::new(&spec.program);
    if spec.env_clear {
        cmd.env_clear();
    }
    crate::privs::as_sandbox_user(&mut cmd);
    cmd.args(&spec.args)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(if spec.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    // SAFETY: `setsid` is async-signal-safe and the closure does nothing else.
    unsafe {
        cmd.pre_exec(|| nix::unistd::setsid().map(|_| ()).map_err(io::Error::from));
    }
    let (mut child, mut exit) = reaper.spawn_with(|| {
        let c = cmd.spawn()?;
        let pid = c.id();
        Ok((c, pid))
    })?;
    let pid = child.id();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // The reaper owns waiting; the handle only carried the pipes.
    drop(child);

    if let (Some(mut w), Some(data)) = (stdin, spec.stdin) {
        std::thread::spawn(move || {
            let _ = w.write_all(&data);
        });
    }
    let out_task = tokio::task::spawn_blocking(move || {
        let mut buf = Vec::new();
        let Some(r) = stdout else { return buf };
        match on_stdout {
            None => {
                let _ = r.take(STDOUT_CAP).read_to_end(&mut buf);
            }
            Some(f) => {
                for line in io::BufReader::new(r).split(b'\n') {
                    let Ok(line) = line else { break };
                    f(String::from_utf8_lossy(&line).trim_end_matches('\r'));
                    buf.extend_from_slice(&line);
                    buf.push(b'\n');
                    if buf.len() > STDERR_TAIL * 2 {
                        buf.drain(..buf.len() - STDERR_TAIL);
                    }
                }
            }
        }
        buf
    });
    let err_task = tokio::task::spawn_blocking(move || {
        let mut tail = String::new();
        if let Some(r) = stderr {
            for line in io::BufReader::new(r).split(b'\n') {
                let Ok(line) = line else { break };
                let line = String::from_utf8_lossy(&line);
                let line = line.trim_end_matches('\r');
                if let Some(f) = &on_stderr {
                    f(line);
                }
                tail.push_str(line);
                tail.push('\n');
                if tail.len() > STDERR_TAIL * 2 {
                    let cut = tail.len() - STDERR_TAIL;
                    let cut = (cut..tail.len())
                        .find(|i| tail.is_char_boundary(*i))
                        .unwrap_or(tail.len());
                    tail.drain(..cut);
                }
            }
        }
        tail
    });

    let mut stop = None;
    let deadline = ctl.deadline;
    let status = tokio::select! {
        r = &mut exit => r.ok(),
        _ = ctl.cancelled() => { stop = Some(Stop::Cancelled); terminate(pid, &mut exit).await }
        _ = tokio::time::sleep_until(deadline) => { stop = Some(Stop::TimedOut); terminate(pid, &mut exit).await }
    };
    // Whatever the child left behind in its group goes too (this also closes the pipes).
    let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    let stdout = tokio::time::timeout(DRAIN_WAIT, out_task)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let stderr = tokio::time::timeout(DRAIN_WAIT, err_task)
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    Ok(Output {
        status,
        stop,
        stdout,
        stderr,
    })
}

/// SIGTERM the process group, SIGKILL after [`TERM_GRACE`]; returns the exit status.
pub(crate) async fn terminate(
    pid: u32,
    exit: &mut oneshot::Receiver<ExitStatus>,
) -> Option<ExitStatus> {
    let pg = Pid::from_raw(pid as i32);
    let _ = killpg(pg, Signal::SIGTERM);
    match tokio::time::timeout(TERM_GRACE, &mut *exit).await {
        Ok(r) => r.ok(),
        Err(_) => {
            let _ = killpg(pg, Signal::SIGKILL);
            exit.await.ok()
        }
    }
}
