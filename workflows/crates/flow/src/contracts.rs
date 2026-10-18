//! `enzarb-lint contracts`: ties a library's workflow YAML and prompts to the
//! contract-test registry (`docs/workflows/contract-tests.md`, "Library check").
//!
//! The registry is the manifest of what the kernel implements and tests. This
//! module extracts the actions, args, result fields and prompt promise markers a
//! library uses and diffs them against it. It never runs the tests.

use crate::diag::Diagnostic;
use crate::yaml::{SourceFile, join};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Directories whose YAML may contain `action:` nodes.
const YAML_DIRS: [&str; 6] = [
    "phases",
    "steps",
    "workflows",
    ".enzarb/phases",
    ".enzarb/steps",
    ".enzarb/workflows",
];
const PROMPT_DIRS: [&str; 2] = ["prompts", ".enzarb/prompts"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub version: u32,
    pub entries: Vec<Entry>,
    #[serde(default)]
    pub gaps: Vec<Gap>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub kind: String,
    pub action: String,
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub tests: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gap {
    pub kind: String,
    pub action: String,
    pub name: String,
    pub reason: String,
}

/// `(kind, action, name)`.
type Key = (String, String, String);

fn key(kind: &str, action: &str, name: &str) -> Key {
    (kind.into(), action.into(), name.into())
}

/// One thing the library uses, with where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Used {
    pub kind: &'static str,
    pub action: String,
    pub name: String,
    pub file: String,
    pub pos: (usize, usize),
}

/// What [`extract_yaml`] found in one file.
#[derive(Debug, Default)]
pub struct Extracted {
    pub used: Vec<Used>,
    pub warnings: Vec<Diagnostic>,
}

/// Normalises `a[0].b` to `a[].b`.
fn normalize(path: &str) -> String {
    static_re(r"\[\d+\]").replace_all(path, "[]").into_owned()
}

fn static_re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

/// Result-field references (`result.a[0].b`, `last.result.x`) in `text`, and
/// whether it indexes `result` dynamically.
fn result_refs(text: &str) -> (Vec<String>, bool) {
    let re = static_re(r"(?:^|[^\w.])(?:last\.)?result((?:\.[A-Za-z_]\w*|\[\d+\])+)");
    let refs = re
        .captures_iter(text)
        .map(|c| normalize(c[1].trim_start_matches('.')))
        .collect();
    let dynamic = static_re(r"(?:^|[^\w.])(?:last\.)?result\[\s*[^\d\s\]]").is_match(text);
    (refs, dynamic)
}

/// `outputs.<as>.<path>` references in `text`.
fn output_refs(text: &str) -> Vec<(String, String)> {
    static_re(r"(?:^|[^\w.])outputs\.([A-Za-z_]\w*)((?:\.[A-Za-z_]\w*|\[\d+\])+)")
        .captures_iter(text)
        .map(|c| (c[1].to_string(), normalize(c[2].trim_start_matches('.'))))
        .collect()
}

fn strings<'a>(v: &'a Value, path: &str, out: &mut Vec<(String, &'a str)>) {
    match v {
        Value::String(s) => out.push((path.to_string(), s)),
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                strings(x, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                let mut p = path.to_string();
                join(&mut p, k);
                strings(x, &p, out);
            }
        }
        _ => {}
    }
}

/// Every object with an `action: {name: …}` member, with its path.
fn action_nodes<'a>(v: &'a Value, path: &str, out: &mut Vec<(String, &'a Value)>) {
    match v {
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                action_nodes(x, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(o) => {
            if o.get("action")
                .and_then(|a| a.get("name"))
                .is_some_and(Value::is_string)
            {
                out.push((path.to_string(), v));
            }
            for (k, x) in o {
                let mut p = path.to_string();
                join(&mut p, k);
                action_nodes(x, &p, out);
            }
        }
        _ => {}
    }
}

/// Extracts the actions, args and result fields one workflow/phase/step file uses.
pub fn extract_yaml(sf: &SourceFile) -> Extracted {
    let mut ex = Extracted::default();
    let mut nodes = Vec::new();
    action_nodes(&sf.value, "", &mut nodes);
    // `as:` name → action, for `outputs.<as>.…` references anywhere in the file.
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    let push = |ex: &mut Extracted, kind, action: &str, name: String, path: &str| {
        ex.used.push(Used {
            kind,
            action: action.into(),
            name,
            file: sf.name.clone(),
            pos: sf.pos(path),
        });
    };
    for (path, node) in &nodes {
        let act = &node["action"];
        let name = act["name"].as_str().unwrap_or_default();
        let base = |rest: &str| {
            let mut p = path.clone();
            join(&mut p, "action");
            join(&mut p, rest);
            p
        };
        push(&mut ex, "action", name, name.into(), &base("name"));
        if let Some(a) = act.get("as").and_then(Value::as_str) {
            aliases.insert(a.into(), name.into());
        }
        if let Some(a) = node.get("as").and_then(Value::as_str) {
            aliases.insert(a.into(), name.into());
        }
        if let Some(args) = act.get("args").and_then(Value::as_object) {
            for (k, v) in args {
                let ap = format!("{}.{k}", base("args"));
                push(&mut ex, "arg", name, k.clone(), &ap);
                // Item keys of a literal list of objects: `issues[].title`.
                if let Some(items) = v.as_array() {
                    for item in items.iter().filter_map(Value::as_object) {
                        for ik in item.keys() {
                            push(&mut ex, "arg", name, format!("{k}[].{ik}"), &ap);
                        }
                    }
                }
            }
        }
        // `result.…` read in the node itself (`relate.from`, `next:` conditions, texts).
        let mut leaves = Vec::new();
        strings(node, path, &mut leaves);
        for (lp, text) in leaves {
            let (refs, dynamic) = result_refs(text);
            for r in refs {
                push(&mut ex, "result_field", name, r, &lp);
            }
            if dynamic {
                ex.warnings.push(sf.warning(
                    &lp,
                    format!("dynamic `result[…]` access after `{name}` cannot be checked"),
                ));
            }
        }
    }
    // `outputs.<as>.…` anywhere in the file.
    let mut leaves = Vec::new();
    strings(&sf.value, "", &mut leaves);
    for (lp, text) in leaves {
        for (alias, field) in output_refs(text) {
            if let Some(action) = aliases.get(&alias) {
                push(&mut ex, "result_field", action, field, &lp);
            }
        }
    }
    ex
}

/// `<!-- promise: <action>.<id> -->` markers in a prompt: `(action, id, line)`.
pub fn extract_markers(text: &str) -> Vec<(String, String, usize)> {
    let re = static_re(r"<!--\s*promise:\s*([A-Za-z_]\w*)\.([\w.\[\]-]+)\s*-->");
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for c in re.captures_iter(line) {
            out.push((c[1].to_string(), c[2].to_string(), i + 1));
        }
    }
    out
}

/// Whether `contract_tests::<file>::<fn>` names a test function under `tests_dir`.
fn test_exists(tests_dir: &Path, module_path: &str) -> bool {
    let parts: Vec<&str> = module_path.split("::").collect();
    let [first, file, name] = parts[..] else {
        return false;
    };
    if first != "contract_tests" {
        return false;
    }
    let Ok(src) = std::fs::read_to_string(tests_dir.join(format!("{file}.rs"))) else {
        return false;
    };
    let lines: Vec<&str> = src.lines().map(str::trim).collect();
    lines.iter().enumerate().any(|(i, l)| {
        (l.starts_with(&format!("async fn {name}(")) || l.starts_with(&format!("fn {name}(")))
            && i > 0
            && lines[i - 1].starts_with("#[")
    })
}

fn walk(root: &Path, dir: &Path, ext: &[&str], out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if name != ".git" && name != "target" && name != "node_modules" {
                walk(root, &p, ext, out);
            }
        } else if ext.iter().any(|x| name.ends_with(x)) {
            let rel = p.strip_prefix(root).unwrap_or(&p);
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn under(f: &str, dirs: &[&str]) -> bool {
    dirs.iter().any(|d| f.starts_with(&format!("{d}/")))
}

/// Result of the check: errors fail it, warnings do not.
pub fn check(
    library: &Path,
    registry_path: &Path,
    tests_dir: Option<&Path>,
) -> Result<Vec<Diagnostic>, String> {
    let rname = registry_path.display().to_string();
    let text = std::fs::read_to_string(registry_path).map_err(|e| format!("{rname}: {e}"))?;
    let rsf = SourceFile::parse(&rname, &text).map_err(|d| d.to_string())?;
    let reg: Registry = rsf.typed().map_err(|d| d.to_string())?;
    if reg.version != 1 {
        return Err(format!(
            "{rname}: unsupported registry version {}",
            reg.version
        ));
    }
    let tests_dir: PathBuf = tests_dir.map(Path::to_path_buf).unwrap_or_else(|| {
        registry_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("../src/contract_tests")
    });
    let mut out = Vec::new();

    // Rule 4: every entry has tests that resolve.
    for (i, e) in reg.entries.iter().enumerate() {
        let at = format!("entries[{i}]");
        let id = format!("{} `{}.{}`", e.kind, e.action, e.name);
        if e.tests.is_empty() {
            out.push(rsf.error(&at, format!("{id} has no contract test")));
        }
        for t in &e.tests {
            if !test_exists(&tests_dir, t) {
                out.push(rsf.error(
                    &format!("{at}.tests"),
                    format!(
                        "{id}: test `{t}` is not a test function under {}",
                        tests_dir.display()
                    ),
                ));
            }
        }
    }

    let entries: BTreeMap<Key, usize> = reg
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| (key(&e.kind, &e.action, &e.name), i))
        .collect();
    let gaps: BTreeMap<Key, &Gap> = reg
        .gaps
        .iter()
        .map(|g| (key(&g.kind, &g.action, &g.name), g))
        .collect();
    let covered = |k: &Key| -> bool {
        if entries.contains_key(k) {
            return true;
        }
        // A used `issues` is covered by entries below it (`issues[].number`).
        k.0 == "result_field"
            && entries.keys().any(|(kind, a, n)| {
                kind == "result_field"
                    && *a == k.1
                    && (n.starts_with(&format!("{}[].", k.2))
                        || n.starts_with(&format!("{}.", k.2)))
            })
    };
    let mut gap_seen: BTreeSet<(Key, String, usize)> = BTreeSet::new();
    let mut judge =
        |out: &mut Vec<Diagnostic>, k: Key, file: &str, pos: (usize, usize), what: &str| {
            if let Some(g) = gaps.get(&k) {
                if gap_seen.insert((k.clone(), file.into(), pos.0)) {
                    out.push(Diagnostic::warning(
                        file,
                        pos,
                        format!("`{}.{}`: documented gap ({})", k.1, k.2, g.reason),
                    ));
                }
            } else if !covered(&k) {
                out.push(Diagnostic::error(
                    file,
                    pos,
                    format!(
                        "{what} `{}.{}` has no `{}` entry (and so no contract test) in {rname}",
                        k.1, k.2, k.0
                    ),
                ));
            }
        };

    // The library's YAML.
    let mut files = Vec::new();
    walk(library, library, &[".yaml", ".yml"], &mut files);
    files.sort();
    let mut seen_used: BTreeSet<(Key, String, usize)> = BTreeSet::new();
    for f in files.iter().filter(|f| under(f, &YAML_DIRS)) {
        let disp = library.join(f).display().to_string();
        let sf = match std::fs::read_to_string(library.join(f))
            .map_err(|e| Diagnostic::error(&disp, (0, 0), e.to_string()))
            .and_then(|t| SourceFile::parse(&disp, &t))
        {
            Ok(sf) => sf,
            Err(d) => {
                out.push(d);
                continue;
            }
        };
        let ex = extract_yaml(&sf);
        out.extend(ex.warnings);
        for u in ex.used {
            let k = key(u.kind, &u.action, &u.name);
            if !seen_used.insert((k.clone(), u.file.clone(), u.pos.0)) {
                continue;
            }
            let what = match u.kind {
                "action" => "action",
                "arg" => "argument",
                _ => "result field",
            };
            // An unknown action is reported once; its args and fields follow from it.
            if u.kind != "action" && !entries.contains_key(&key("action", &u.action, &u.action)) {
                continue;
            }
            judge(&mut out, k, &u.file, u.pos, what);
        }
    }

    // Prompt markers.
    let mut prompts = Vec::new();
    walk(library, library, &[".md"], &mut prompts);
    prompts.sort();
    let mut carried: BTreeSet<(String, Key)> = BTreeSet::new();
    let mut library_prompts: BTreeSet<String> = BTreeSet::new();
    for f in prompts.iter().filter(|f| under(f, &PROMPT_DIRS)) {
        library_prompts.insert(f.clone());
        let disp = library.join(f).display().to_string();
        let Ok(text) = std::fs::read_to_string(library.join(f)) else {
            continue;
        };
        for (action, id, line) in extract_markers(&text) {
            let k = key("prompt_promise", &action, &id);
            carried.insert((f.clone(), k.clone()));
            judge(&mut out, k, &disp, (line, 1), "prompt promise");
        }
    }
    // Rule 3: entries sourced from a prompt of this library must be carried by it.
    for (i, e) in reg.entries.iter().enumerate() {
        if e.kind != "prompt_promise" {
            continue;
        }
        let src = e.source.split_whitespace().next().unwrap_or_default();
        if library_prompts.contains(src)
            && !carried.contains(&(src.to_string(), key(&e.kind, &e.action, &e.name)))
        {
            out.push(rsf.error(
                &format!("entries[{i}]"),
                format!(
                    "prompt promise `{}.{}` is not carried by `{src}` (add `<!-- promise: {}.{} -->`)",
                    e.action, e.name, e.action, e.name
                ),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(yaml: &str) -> Extracted {
        extract_yaml(&SourceFile::parse("t.yaml", yaml).unwrap())
    }

    fn has(ex: &Extracted, kind: &str, action: &str, name: &str) -> bool {
        ex.used
            .iter()
            .any(|u| u.kind == kind && u.action == action && u.name == name)
    }

    #[test]
    fn extracts_actions_args_and_result_fields() {
        let ex = used(
            "states:\n  a:\n    action:\n      name: create_issues\n      args: {parent: '{{ x }}'}\n      as: kids\n      relate: {kind: parent, from: 'result.issues[0].number'}\n  b:\n    text: '{{ outputs.kids.issues[2].url }}'\n",
        );
        assert!(has(&ex, "action", "create_issues", "create_issues"));
        assert!(has(&ex, "arg", "create_issues", "parent"));
        assert!(has(&ex, "result_field", "create_issues", "issues[].number"));
        assert!(has(&ex, "result_field", "create_issues", "issues[].url"));
        assert!(ex.warnings.is_empty());
    }

    #[test]
    fn dynamic_result_access_warns() {
        let ex = used("a:\n  action: {name: comment, relate: {kind: k, from: 'result[field]'}}\n");
        assert_eq!(ex.warnings.len(), 1, "{:?}", ex.warnings);
        assert!(!ex.used.iter().any(|u| u.kind == "result_field"));
    }

    #[test]
    fn markers_need_the_exact_comment() {
        let m = extract_markers(
            "x\n<!-- promise: create_issues.title_format -->\nprose says create_issues.foo\n",
        );
        assert_eq!(m, vec![("create_issues".into(), "title_format".into(), 2)]);
    }
}
