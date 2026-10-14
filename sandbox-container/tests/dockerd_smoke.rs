//! Real-dockerd smoke test for the overlay2 default (issue #119). Run it inside a gVisor sandbox
//! with network access to pull `busybox`:
//!
//! ```text
//! ENZARB_SMOKE_DOCKERD=1 cargo test --test dockerd_smoke -- --ignored --nocapture
//! ```
//!
//! It starts dockerd with `DEFAULT_ARGS` against a temp data root, builds a multi-stage
//! Dockerfile, runs the image and checks that a file deleted in a later layer is gone. It skips
//! when `ENZARB_SMOKE_DOCKERD` is unset or `dockerd`/`docker` are missing.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use sandbox_init::dockerd::{Dockerd, DockerdConfig, State};
use sandbox_init::reaper::Reaper;

fn on_path(bin: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {bin}")])
        .output()
        .is_ok_and(|o| o.status.success())
}

fn docker(sock: &Path, args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .env("DOCKER_HOST", format!("unix://{}", sock.display()))
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
#[ignore = "needs dockerd, docker and network; set ENZARB_SMOKE_DOCKERD=1"]
async fn overlay2_builds_runs_and_deletes_across_layers() {
    if std::env::var_os("ENZARB_SMOKE_DOCKERD").is_none()
        || !on_path("dockerd")
        || !on_path("docker")
    {
        eprintln!("skipped: ENZARB_SMOKE_DOCKERD unset or dockerd/docker missing");
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let cfg = DockerdConfig {
        socket: t.path().join("docker.sock"),
        data_root: t.path().join("data"),
        exec_root: t.path().join("exec"),
        ..DockerdConfig::default()
    };
    let sock = cfg.socket.clone();
    let reaper = Reaper::new(false);
    tokio::spawn(reaper.clone().run());
    let d: Arc<Dockerd> = Dockerd::new(cfg, reaper);
    let task = d.start();
    for _ in 0..120 {
        if d.state() == State::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(
        d.state(),
        State::Running,
        "dockerd must start with overlay2"
    );

    let ctx = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/smoke");
    let build = docker(
        &sock,
        &["build", "-t", "enzarb-smoke", ctx.to_str().unwrap()],
    );
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    let info = docker(&sock, &["info", "--format", "{{.Driver}}"]);
    assert_eq!(String::from_utf8_lossy(&info.stdout).trim(), "overlay2");
    let run = docker(&sock, &["run", "--rm", "enzarb-smoke"]);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = String::from_utf8_lossy(&run.stdout);
    assert!(out.contains("keep"), "{out}");
    assert!(!out.contains("gone"), "{out}");

    d.shutdown(Duration::from_secs(20)).await;
    task.await.unwrap();
}
