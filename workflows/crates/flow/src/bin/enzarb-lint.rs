//! `enzarb-lint`: validates workflow definitions and runs scenario tests.
//!
//! ```text
//! enzarb-lint check <dir> [--library <alias|owner/repo>=<path>]... [--repo owner/repo] [--format text|github]
//! enzarb-lint test  <dir> [same options]
//! enzarb-lint schemas <docs/workflows dir>
//! enzarb-lint contracts <library dir> --registry <contract-registry.yaml> [--tests-dir <dir>] [--format text|github]
//! ```
//!
//! Exit codes: 0 clean, 1 errors or failing scenarios, 2 usage or IO error.

use enzarb_flow::Diagnostic;
use enzarb_flow::lint::{Config, Workspace};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage:
  enzarb-lint check <dir> [--library <alias|owner/repo>=<path>]... [--repo <owner/repo>] [--format text|github]
  enzarb-lint test  <dir> [--library ...] [--repo ...] [--format ...]
  enzarb-lint schemas <docs/workflows dir>
  enzarb-lint contracts <library dir> --registry <contract-registry.yaml> [--tests-dir <dir>] [--format text|github]";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("enzarb-lint: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    let mut it = args.into_iter();
    let cmd = it.next().ok_or("missing command")?;
    if cmd == "-h" || cmd == "--help" {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut dir = None;
    let mut libraries = Vec::new();
    let mut repo = "local/repo".to_string();
    let mut github = false;
    let mut registry = None;
    let mut tests_dir = None;
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--library" => {
                let v = val("--library")?;
                let (k, p) = v.split_once('=').ok_or("--library takes <alias>=<path>")?;
                libraries.push((k.to_string(), PathBuf::from(p)));
            }
            "--registry" => registry = Some(PathBuf::from(val("--registry")?)),
            "--tests-dir" => tests_dir = Some(PathBuf::from(val("--tests-dir")?)),
            "--repo" => repo = val("--repo")?,
            "--format" => match val("--format")?.as_str() {
                "github" => github = true,
                "text" => github = false,
                f => return Err(format!("unknown format `{f}`")),
            },
            s if s.starts_with("--") => return Err(format!("unknown option `{s}`")),
            _ if dir.is_none() => dir = Some(PathBuf::from(a)),
            _ => return Err(format!("unexpected argument `{a}`")),
        }
    }
    let dir = dir.ok_or("missing <dir>")?;
    if cmd == "schemas" {
        for (rel, text) in enzarb_flow::schema::generated() {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().expect("has parent")).map_err(|e| e.to_string())?;
            std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if cmd == "contracts" {
        let registry = registry.ok_or("contracts needs --registry <contract-registry.yaml>")?;
        let ds = enzarb_flow::contracts::check(&dir, &registry, tests_dir.as_deref())?;
        for d in &ds {
            if github {
                println!("{}", d.github());
            } else {
                eprintln!("{d}");
            }
        }
        let errors = ds.iter().filter(|d| d.is_error()).count();
        eprintln!("{errors} error(s), {} warning(s)", ds.len() - errors);
        return Ok(if errors > 0 {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
    let ws = Workspace::new(Config {
        dir,
        repo,
        libraries,
    })?;
    let print = |ds: &[Diagnostic]| {
        for d in ds {
            if github {
                println!("{}", d.github());
            } else {
                eprintln!("{d}");
            }
        }
    };
    match cmd.as_str() {
        "check" => {
            let ds = ws.check();
            print(&ds);
            let errors = ds.iter().filter(|d| d.is_error()).count();
            eprintln!("{errors} error(s), {} warning(s)", ds.len() - errors);
            Ok(if errors > 0 {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            })
        }
        "test" => {
            let (n, ds) = ws.test();
            print(&ds);
            eprintln!("{n} scenario file(s), {} failure(s)", ds.len());
            Ok(if ds.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        c => Err(format!("unknown command `{c}`")),
    }
}
