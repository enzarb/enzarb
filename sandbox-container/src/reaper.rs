//! Zombie reaping and child-exit delivery.
//!
//! As pid 1, sandbox-init inherits every orphan and must `waitpid` them. A
//! blanket `waitpid(-1)` would also steal the exit status of children other
//! code waits for, so the reaper is the *only* place that waits: code that
//! spawns a child registers it with [`Reaper::spawn_with`] and receives the
//! exit status on a oneshot channel (PTY children today, `claude`/`git` runs in
//! the next issue).
//!
//! Two modes:
//! * reap-all (pid 1, or `ENZARB_REAP_ALL=1`): `waitpid(-1)` on every SIGCHLD;
//!   statuses of registered pids are delivered, orphans are discarded;
//! * registered-only (everything else, e.g. tests): only registered pids are
//!   waited on, so the host process's other children are left alone.

use std::collections::HashMap;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, kill, killpg};
use nix::sys::wait::{Id, WaitPidFlag, WaitStatus, waitid, waitpid};
use nix::unistd::Pid;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Exited(i32),
    Signaled(i32),
}

impl ExitStatus {
    /// The exit code, `None` if killed by a signal.
    pub fn code(self) -> Option<i32> {
        match self {
            Self::Exited(c) => Some(c),
            Self::Signaled(_) => None,
        }
    }
}

pub struct Reaper {
    reap_all: bool,
    children: Mutex<HashMap<i32, oneshot::Sender<ExitStatus>>>,
}

impl Reaper {
    pub fn new(reap_all: bool) -> Arc<Self> {
        Arc::new(Self {
            reap_all,
            children: Mutex::new(HashMap::new()),
        })
    }

    /// Reap-all when we are pid 1 (or `ENZARB_REAP_ALL=1`).
    pub fn for_current_process() -> Arc<Self> {
        let forced = std::env::var("ENZARB_REAP_ALL").is_ok_and(|v| v == "1");
        Self::new(std::process::id() == 1 || forced)
    }

    pub fn reaps_all(&self) -> bool {
        self.reap_all
    }

    /// Spawn a child through `f` (which returns the child's pid and any
    /// payload) and register it. The registry lock is held across `f`, so the
    /// reaper cannot observe the child's exit before it is registered.
    pub fn spawn_with<T>(
        &self,
        f: impl FnOnce() -> io::Result<(T, u32)>,
    ) -> io::Result<(T, oneshot::Receiver<ExitStatus>)> {
        let mut children = self.children.lock().unwrap();
        let (value, pid) = f()?;
        let (tx, rx) = oneshot::channel();
        children.insert(pid as i32, tx);
        Ok((value, rx))
    }

    /// Number of registered, not yet exited children.
    pub fn registered(&self) -> usize {
        self.children.lock().unwrap().len()
    }

    /// One reaping pass; returns how many children were reaped.
    pub fn reap(&self) -> usize {
        let mut children = self.children.lock().unwrap();
        let mut n = 0;
        if self.reap_all {
            loop {
                match waitpid(None, Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive) | Err(Errno::ECHILD) => break,
                    Err(Errno::EINTR) => continue,
                    Err(e) => {
                        tracing::warn!(error = %e, "waitpid failed");
                        break;
                    }
                    Ok(st) => {
                        if let Some((pid, status)) = decode(st) {
                            n += 1;
                            deliver(&mut children, pid, status);
                        }
                    }
                }
            }
        } else {
            let pids: Vec<i32> = children.keys().copied().collect();
            for pid in pids {
                match waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)) {
                    Ok(WaitStatus::StillAlive) => {}
                    Err(Errno::ECHILD) => {
                        // Somebody else reaped it; report as lost.
                        n += 1;
                        deliver(&mut children, pid, ExitStatus::Signaled(0));
                    }
                    Err(_) => {}
                    Ok(st) => {
                        if let Some((pid, status)) = decode(st) {
                            n += 1;
                            deliver(&mut children, pid, status);
                        }
                    }
                }
            }
        }
        n
    }

    /// Reap on every SIGCHLD (with a periodic backstop). Run for the life of the process.
    pub async fn run(self: Arc<Self>) {
        let mut sigchld = match signal(SignalKind::child()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!(error = %e, "cannot install SIGCHLD handler; polling");
                None
            }
        };
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                _ = async {
                    match sigchld.as_mut() {
                        Some(s) => { s.recv().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => {}
                _ = tick.tick() => {}
            }
            self.reap();
        }
    }

    /// Send `sig` to the process group of every registered child (they are
    /// session leaders); as pid 1 also to every other process.
    pub fn signal_all(&self, sig: Signal) {
        let pids: Vec<i32> = self.children.lock().unwrap().keys().copied().collect();
        for pid in pids {
            let pid = Pid::from_raw(pid);
            if killpg(pid, sig).is_err() {
                let _ = kill(pid, sig);
            }
        }
        if self.reap_all && std::process::id() == 1 {
            let _ = kill(Pid::from_raw(-1), sig);
        }
    }

    fn any_children(&self) -> bool {
        if self.reap_all {
            !matches!(
                waitid(
                    Id::All,
                    WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT
                ),
                Err(Errno::ECHILD)
            ) || self.registered() > 0
        } else {
            self.registered() > 0
        }
    }

    /// Graceful shutdown: SIGTERM to children, wait up to `grace` for them to
    /// exit (the reaper task must be running), then SIGKILL the rest.
    /// Returns `true` if everything exited within the grace period.
    pub async fn terminate_all(&self, grace: Duration) -> bool {
        self.signal_all(Signal::SIGTERM);
        let deadline = tokio::time::Instant::now() + grace;
        while self.any_children() {
            self.reap();
            if !self.any_children() {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::warn!("grace period over; sending SIGKILL");
                self.signal_all(Signal::SIGKILL);
                tokio::time::sleep(Duration::from_millis(200)).await;
                self.reap();
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }
}

fn decode(st: WaitStatus) -> Option<(i32, ExitStatus)> {
    match st {
        WaitStatus::Exited(pid, code) => Some((pid.as_raw(), ExitStatus::Exited(code))),
        WaitStatus::Signaled(pid, sig, _) => Some((pid.as_raw(), ExitStatus::Signaled(sig as i32))),
        _ => None,
    }
}

fn deliver(children: &mut HashMap<i32, oneshot::Sender<ExitStatus>>, pid: i32, st: ExitStatus) {
    match children.remove(&pid) {
        Some(tx) => {
            let _ = tx.send(st);
        }
        None => tracing::debug!(pid, ?st, "reaped orphan"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // Reap-all mode steals every child's status, so tests that spawn children
    // in this binary take this lock.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn gone(pid: u32) -> bool {
        // A zombie still answers kill(pid, 0); a reaped pid does not.
        matches!(kill(Pid::from_raw(pid as i32), None), Err(Errno::ESRCH))
    }

    #[test]
    fn reap_all_reaps_unregistered_orphans() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let reaper = Reaper::new(true);
        let child = Command::new("true").spawn().unwrap();
        let pid = child.id();
        std::mem::forget(child); // never waited: becomes a zombie
        for _ in 0..100 {
            reaper.reap();
            if gone(pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(gone(pid), "zombie {pid} was not reaped");
    }

    #[test]
    fn delivers_exit_status_of_registered_children() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        for reap_all in [false, true] {
            let reaper = Reaper::new(reap_all);
            let (child, mut rx) = reaper
                .spawn_with(|| {
                    let c = Command::new("sh").args(["-c", "exit 7"]).spawn()?;
                    let pid = c.id();
                    Ok((c, pid))
                })
                .unwrap();
            assert_eq!(reaper.registered(), 1);
            let mut got = None;
            for _ in 0..100 {
                reaper.reap();
                if let Ok(st) = rx.try_recv() {
                    got = Some(st);
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(got, Some(ExitStatus::Exited(7)), "reap_all={reap_all}");
            assert_eq!(reaper.registered(), 0);
            drop(child);
        }
    }

    #[test]
    fn terminate_all_signals_registered_children() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let reaper = Reaper::new(false);
            // Not a session leader: killpg fails and signal_all falls back to kill(pid).
            let (_c, rx) = reaper
                .spawn_with(|| {
                    let c = Command::new("sleep").arg("30").spawn()?;
                    let pid = c.id();
                    Ok((c, pid))
                })
                .unwrap();
            assert!(reaper.terminate_all(Duration::from_secs(5)).await);
            assert_eq!(rx.await.unwrap(), ExitStatus::Signaled(15));
        });
    }
}
