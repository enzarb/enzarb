//! Smoke tests running the built `ez` binary against the demo mock model.

use std::process::{Command, Output};

fn project() -> (tempfile::TempDir, tempfile::TempDir) {
    let proj = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
    std::fs::write(
        proj.path().join(".ez/config.toml"),
        "default_profile = \"demo\"\n[profiles.demo]\nprovider = \"mock\"\nmodel = \"mock\"\n",
    )
    .unwrap();
    (proj, home)
}

fn rc(proj: &std::path::Path, home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ez"))
        .args(args)
        .current_dir(proj)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env_remove("EZ_PROFILE")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn write_with_yolo_creates_file() {
    let (proj, home) = project();
    let out = rc(
        proj.path(),
        home.path(),
        &["-p", "mock:write a.txt hello", "--yolo"],
    );
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        std::fs::read_to_string(proj.path().join("a.txt")).unwrap(),
        "hello"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("Tool `Write` returned"));
}

#[test]
fn write_without_permission_is_denied() {
    let (proj, home) = project();
    let out = rc(proj.path(), home.path(), &["-p", "mock:write b.txt no"]);
    assert!(out.status.success(), "{out:?}");
    assert!(!proj.path().join("b.txt").exists());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Tool failed"));
}

#[test]
fn ask_is_cancelled_in_headless_mode() {
    let (proj, home) = project();
    let out = rc(proj.path(), home.path(), &["-p", "mock:ask"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Tool failed"), "{stdout}");
}

#[test]
fn plain_prompt_lists_commands() {
    let (proj, home) = project();
    let out = rc(proj.path(), home.path(), &["-p", "hi there"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("You said: hi there") && stdout.contains("mock:ask"));
}
