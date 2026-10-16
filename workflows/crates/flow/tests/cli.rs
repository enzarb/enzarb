//! `enzarb-lint` end to end over checked-in fixtures: exit codes, output
//! formats and scenario tests.

use std::path::PathBuf;
use std::process::Command;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn lint(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_enzarb-lint"))
        .args(args)
        .current_dir(fixtures())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

#[test]
fn library_checks_clean() {
    let (code, _, err) = lint(&["check", "lib", "--repo", "lib/std"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn repo_checks_clean_with_library() {
    let (code, _, err) = lint(&["check", "app", "--repo", "acme/app", "--library", "std=lib"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn missing_library_is_an_error() {
    let (code, _, err) = lint(&["check", "app", "--repo", "acme/app"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("not available locally"), "{err}");
}

#[test]
fn scenarios_pass() {
    let (code, _, err) = lint(&["test", "app", "--repo", "acme/app", "--library", "std=lib"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("2 scenario file(s), 0 failure(s)"), "{err}");
}

#[test]
fn errors_with_locations_and_github_format() {
    let (code, _, err) = lint(&["check", "bad"]);
    assert_eq!(code, 1);
    assert!(err.contains("bad/.enzarb/workflows/w.yaml:4:"), "{err}");
    assert!(err.contains("unknown param `colour`"), "{err}");
    assert!(err.contains("bad/.enzarb/phases/p.yaml:5:"), "{err}");
    assert!(
        err.contains("`subjects.isue` is not a task document field"),
        "{err}"
    );
    assert!(err.contains("unknown node `b`"), "{err}");
    let (code, out, _) = lint(&["check", "bad", "--format", "github"]);
    assert_eq!(code, 1);
    assert!(
        out.lines()
            .all(|l| l.starts_with("::error file=") || l.starts_with("::warning file=")),
        "{out}"
    );
    assert!(
        out.contains("::error file=bad/.enzarb/workflows/w.yaml,line=4,col="),
        "{out}"
    );
}

#[test]
fn usage_errors_exit_2() {
    assert_eq!(lint(&[]).0, 2);
    assert_eq!(lint(&["frobnicate", "app"]).0, 2);
    assert_eq!(lint(&["check", "app", "--format", "xml"]).0, 2);
    assert_eq!(lint(&["check", "does-not-exist"]).0, 2);
}
