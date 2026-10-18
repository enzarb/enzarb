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

fn contracts(case: &str, extra: &[&str]) -> (i32, String, String) {
    let reg = format!("contracts/{case}/registry.yaml");
    let lib = format!("contracts/{case}/lib");
    let mut args = vec![
        "contracts",
        lib.as_str(),
        "--registry",
        reg.as_str(),
        "--tests-dir",
        "contracts/tests",
    ];
    args.extend_from_slice(extra);
    lint(&args)
}

#[test]
fn contracts_pass() {
    let (code, _, err) = contracts("ok", &[]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("0 error(s), 0 warning(s)"), "{err}");
}

#[test]
fn contracts_unknown_action_arg_and_result_field() {
    let (code, _, err) = contracts("unknown_action", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("action `frobnicate.frobnicate` has no `action` entry"),
        "{err}"
    );
    assert!(err.contains("p.yaml:4:"), "{err}");
    let (code, _, err) = contracts("unknown_arg", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("argument `comment.colour` has no `arg` entry"),
        "{err}"
    );
    let (code, _, err) = contracts("unknown_result", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("result field `comment.nope`"), "{err}");
}

#[test]
fn contracts_entry_without_or_with_dangling_test() {
    let (code, _, err) = contracts("no_test", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("has no contract test"), "{err}");
    let (code, _, err) = contracts("dangling_test", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("`contract_tests::a::gone` is not a test function"),
        "{err}"
    );
}

#[test]
fn contracts_prompt_promises_both_ways() {
    let (code, _, err) = contracts("missing_promise", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("prompt promise `comment.farewell` has no `prompt_promise` entry"),
        "{err}"
    );
    assert!(err.contains("p.md:2:"), "{err}");
    let (code, _, err) = contracts("dangling_promise", &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("`comment.greeting` is not carried by `prompts/p.md`"),
        "{err}"
    );
}

#[test]
fn contracts_gap_warns_and_passes() {
    let (code, _, err) = contracts("gap", &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("warning: `comment.colour`: documented gap (colour is not tested yet)"),
        "{err}"
    );
}

#[test]
fn contracts_github_format_and_usage() {
    let (code, out, _) = contracts("gap", &["--format", "github"]);
    assert_eq!(code, 0);
    assert!(out.contains("::warning file="), "{out}");
    let (code, out, _) = contracts("unknown_arg", &["--format", "github"]);
    assert_eq!(code, 1);
    assert!(out.contains("::error file="), "{out}");
    assert_eq!(lint(&["contracts", "contracts/ok/lib"]).0, 2);
}
