//! Shared validator fixtures: `tests/fixtures/validate.json` is also run by the UI's TypeScript
//! port (`ui/src/lib/workflow/definition.spec.ts`), so the two validators cannot drift.

use enzarb_graph::{Definition, Error, expr, validate};
use serde_json::Value;

const FIXTURES: &str = include_str!("fixtures/validate.json");

#[test]
fn shared_validation_cases() {
    let f: Value = serde_json::from_str(FIXTURES).unwrap();
    for case in f["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let yaml = case["yaml"].as_str().unwrap();
        let want: Vec<&str> = case["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let parsed = Definition::from_yaml(yaml);
        if case["parse_error"].as_bool().unwrap() {
            assert!(
                matches!(parsed, Err(Error::Parse(_))),
                "{name}: expected a parse error, got {parsed:?}"
            );
            continue;
        }
        let d = parsed.unwrap_or_else(|e| panic!("{name}: {e}"));
        let problems = match validate(&d) {
            Ok(()) => vec![],
            Err(Error::Invalid(p)) => p,
            Err(e) => panic!("{name}: {e}"),
        };
        assert_eq!(
            problems.len(),
            want.len(),
            "{name}: wanted {want:?}, got {problems:?}"
        );
        for w in &want {
            assert!(
                problems.iter().any(|p| p.contains(w)),
                "{name}: wanted `{w}` in {problems:?}"
            );
        }
    }
}

#[test]
fn shared_expression_cases() {
    let f: Value = serde_json::from_str(FIXTURES).unwrap();
    let ex = &f["expressions"];
    let ctx = &ex["ctx"];
    for pair in ex["eval"].as_array().unwrap() {
        let src = pair[0].as_str().unwrap();
        let e = expr::parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
        assert_eq!(expr::eval(&e, ctx), pair[1].as_bool().unwrap(), "{src}");
    }
    for src in ex["reject"].as_array().unwrap() {
        let src = src.as_str().unwrap();
        assert!(expr::parse(src).is_err(), "{src:?} should not parse");
    }
}
