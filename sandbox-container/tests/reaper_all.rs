//! Reap-all reaper tests.
//!
//! Reap-all mode calls `waitpid(-1)`, which steals the exit status of every
//! child of the process. This file must stay its own test binary: sharing a
//! process with tests that spawn children (`Command::output()`) makes those
//! fail with ECHILD under plain `cargo test`.

use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use sandbox_init::reaper::{ExitStatus, Reaper};

// The tests below run in parallel by default and would steal each other's
// children, so they take this lock.
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
fn reap_all_delivers_exit_status_of_registered_children() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let reaper = Reaper::new(true);
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
    assert_eq!(got, Some(ExitStatus::Exited(7)));
    assert_eq!(reaper.registered(), 0);
    drop(child);
}
