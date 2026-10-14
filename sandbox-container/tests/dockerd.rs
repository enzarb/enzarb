//! The dockerd supervisor against a fake `dockerd` script. A separate test binary: these tests
//! run children through a `Reaper`, and the lib's unit tests include reap-all ones.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use sandbox_init::dockerd::{Dockerd, DockerdConfig, State};
use sandbox_init::reaper::Reaper;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

struct Fake {
    dir: tempfile::TempDir,
}

impl Fake {
    /// `body` is the shell script after the shebang; `$LOG` is the log file.
    fn new(body: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("dockerd");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nLOG={}/log\n{body}\n", dir.path().display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self { dir }
    }

    fn cfg(&self) -> DockerdConfig {
        DockerdConfig {
            bin: self
                .dir
                .path()
                .join("dockerd")
                .to_string_lossy()
                .into_owned(),
            socket: self.dir.path().join("docker.sock"),
            exec_root: self.dir.path().join("exec"),
            data_root: self.dir.path().join("data"),
            args: vec!["--fake".into()],
            backoff_initial: Duration::from_millis(50),
            backoff_max: Duration::from_millis(200),
            stable_after: Duration::from_secs(3600),
            probe_interval: Duration::from_millis(20),
            ..DockerdConfig::default()
        }
    }

    fn log(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn starts(&self) -> Vec<u32> {
        self.log()
            .iter()
            .filter_map(|l| l.strip_prefix("pid ").and_then(|p| p.parse().ok()))
            .collect()
    }
}

/// Answers `GET /_ping` like dockerd does, standing in for the daemon's socket.
fn serve_ping(path: &Path) {
    let l = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut b = [0u8; 256];
                let _ = s.read(&mut b).await;
                let _ = s
                    .write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nOK")
                    .await;
            });
        }
    });
}

fn setup(f: &Fake) -> (Arc<Dockerd>, Arc<Reaper>) {
    let reaper = Reaper::new(false);
    tokio::spawn(reaper.clone().run());
    (Dockerd::new(f.cfg(), reaper.clone()), reaper)
}

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    for _ in 0..300 {
        if ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {what}");
}

const SLEEPER: &str = "echo \"pid $$\" >> \"$LOG\"\nexec sleep 300";

#[tokio::test]
async fn disabled_never_spawns() {
    let f = Fake::new(SLEEPER);
    let mut cfg = f.cfg();
    cfg.enabled = false;
    let d = Dockerd::new(cfg, Reaper::new(false));
    assert_eq!(d.state(), State::Disabled);
    d.start().await.unwrap();
    d.shutdown(Duration::from_secs(1)).await;
    assert_eq!(d.state(), State::Disabled);
    assert!(f.starts().is_empty());
}

#[tokio::test]
async fn missing_binary_gives_up_without_retrying() {
    let f = Fake::new(SLEEPER);
    let mut cfg = f.cfg();
    cfg.bin = "/nonexistent/dockerd".into();
    let d = Dockerd::new(cfg, Reaper::new(false));
    // The supervisor task ends by itself.
    tokio::time::timeout(Duration::from_secs(5), d.start())
        .await
        .expect("supervisor should stop")
        .unwrap();
    match d.state() {
        State::Failed {
            reason,
            retry_in_secs: None,
        } => assert!(reason.contains("not found"), "{reason}"),
        s => panic!("{s:?}"),
    }
}

#[tokio::test]
async fn starts_then_reports_running_and_stops_on_shutdown() {
    let f = Fake::new(
        "trap 'echo term >> \"$LOG\"; exit 0' TERM\necho \"pid $$\" >> \"$LOG\"\necho \"args $*\" >> \"$LOG\"\nsleep 300 &\nwait",
    );
    serve_ping(&f.cfg().socket);
    let (d, _r) = setup(&f);
    let task = d.start();
    until("running", || d.state() == State::Running).await;
    let log = f.log();
    let args = log.iter().find(|l| l.starts_with("args ")).unwrap();
    assert!(args.contains("--host=unix://"), "{args}");
    assert!(args.contains("--group=1000"), "{args}");
    assert!(args.ends_with("--fake"), "{args}");
    d.shutdown(Duration::from_secs(5)).await;
    task.await.unwrap();
    assert_eq!(d.state(), State::Stopped);
    // SIGTERM reached the daemon (it logged before exiting) and nothing restarted it.
    assert!(f.log().contains(&"term".to_string()), "{:?}", f.log());
    assert_eq!(f.starts().len(), 1);
}

#[tokio::test]
async fn restarts_with_backoff_after_it_is_killed() {
    let f = Fake::new(SLEEPER);
    serve_ping(&f.cfg().socket);
    let (d, _r) = setup(&f);
    let task = d.start();
    until("running", || d.state() == State::Running).await;
    let first = f.starts()[0];
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(first as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    // Failed (exit 9), then a new process, then running again.
    until("restart", || f.starts().len() == 2).await;
    assert_ne!(f.starts()[1], first);
    until("running again", || d.state() == State::Running).await;
    d.shutdown(Duration::from_secs(5)).await;
    task.await.unwrap();
}

#[tokio::test]
async fn crash_loop_backs_off_exponentially_and_reports_failed() {
    let f = Fake::new("echo \"pid $$\" >> \"$LOG\"\nexit 3");
    let (d, _r) = setup(&f);
    let task = d.start();
    until("several starts", || f.starts().len() >= 3).await;
    // Between attempts the supervisor reports the exit and waits (50, 100, 200 ms, ...).
    match d.state() {
        State::Failed { reason, .. } => assert!(reason.contains("code 3"), "{reason}"),
        State::Starting => {}
        s => panic!("{s:?}"),
    }
    d.shutdown(Duration::from_secs(1)).await;
    task.await.unwrap();
    assert_eq!(d.state(), State::Stopped);
}

#[tokio::test]
async fn sigkill_after_the_grace_period() {
    let f = Fake::new("trap '' TERM\necho \"pid $$\" >> \"$LOG\"\nwhile true; do sleep 1; done");
    let (d, _r) = setup(&f);
    let task = d.start();
    until("started", || !f.starts().is_empty()).await;
    let t = std::time::Instant::now();
    d.shutdown(Duration::from_millis(300)).await;
    task.await.unwrap();
    assert!(t.elapsed() >= Duration::from_millis(250));
    assert!(t.elapsed() < Duration::from_secs(5));
    let pid = PathBuf::from(format!("/proc/{}", f.starts()[0]));
    until("process gone", || !pid.exists()).await;
}
