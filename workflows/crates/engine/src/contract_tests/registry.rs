//! Self-check of `tests/contract-registry.yaml` (`docs/workflows/contract-tests.md`).

use std::collections::HashSet;

use serde::Deserialize;

const KINDS: [&str; 4] = ["action", "arg", "result_field", "prompt_promise"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    version: u32,
    entries: Vec<Entry>,
    #[serde(default)]
    gaps: Vec<Gap>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    kind: String,
    action: String,
    name: String,
    source: String,
    tests: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Gap {
    kind: String,
    action: String,
    name: String,
    reason: String,
}

fn load() -> Registry {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/contract-registry.yaml");
    serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Whether `module_path` (`contract_tests::<file>::<test>`) names a test function.
fn test_exists(module_path: &str) -> bool {
    let parts: Vec<&str> = module_path.split("::").collect();
    let [first, file, name] = parts[..] else {
        return false;
    };
    if first != "contract_tests" {
        return false;
    }
    let path = format!(
        "{}/src/contract_tests/{file}.rs",
        env!("CARGO_MANIFEST_DIR")
    );
    let Ok(src) = std::fs::read_to_string(path) else {
        return false;
    };
    let lines: Vec<&str> = src.lines().map(str::trim).collect();
    lines.iter().enumerate().any(|(i, l)| {
        (l.starts_with(&format!("async fn {name}(")) || l.starts_with(&format!("fn {name}(")))
            && i > 0
            && lines[i - 1].starts_with("#[")
    })
}

#[test]
fn registry_is_well_formed() {
    let r = load();
    assert_eq!(r.version, 1);
    let mut seen = HashSet::new();
    let mut prev: Option<(String, String, String)> = None;
    for e in &r.entries {
        assert!(KINDS.contains(&e.kind.as_str()), "bad kind {}", e.kind);
        assert!(!e.source.is_empty() && !e.name.is_empty(), "{}", e.name);
        assert!(!e.tests.is_empty(), "{} has no test", e.name);
        assert!(
            seen.insert((e.kind.clone(), e.action.clone(), e.name.clone())),
            "duplicate entry {} {} {}",
            e.kind,
            e.action,
            e.name
        );
        let k = (e.action.clone(), e.kind.clone(), e.name.clone());
        assert!(prev.as_ref().is_none_or(|p| *p < k), "unsorted at {k:?}");
        prev = Some(k);
    }
    for g in &r.gaps {
        assert!(KINDS.contains(&g.kind.as_str()) && !g.reason.is_empty() && !g.action.is_empty());
        assert!(!g.name.is_empty());
    }
}

#[test]
fn every_registry_test_exists() {
    for e in load().entries {
        for t in &e.tests {
            assert!(
                test_exists(t),
                "{} {}: no test fn behind {t}",
                e.action,
                e.name
            );
        }
    }
}

#[test]
fn create_issues_is_registered_with_all_documented_parts() {
    let r = load();
    let has = |kind: &str, name: &str| {
        r.entries
            .iter()
            .any(|e| e.action == "create_issues" && e.kind == kind && e.name == name)
    };
    assert!(has("action", "create_issues"));
    for a in [
        "issues",
        "key",
        "title",
        "body",
        "labels",
        "depends_on",
        "parent",
        "footer",
    ] {
        assert!(has("arg", a), "arg {a}");
    }
    for f in ["kind", "key", "repo", "number", "title", "url"] {
        assert!(has("result_field", &format!("issues[].{f}")), "field {f}");
    }
}
