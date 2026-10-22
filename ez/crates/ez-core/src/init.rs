//! `ez init`: generate `.ez/modes.toml` from embedded templates and the project's mise tasks.

use anyhow::{Context, Result, bail};
use ez_mise::MiseTask;
use std::collections::{BTreeSet, HashSet};

pub(crate) const FULL: &str = include_str!("../templates/modes.toml");
pub(crate) const MINIMAL: &str = include_str!("../templates/modes-minimal.toml");

#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    pub minimal: bool,
    /// Subset of mode ids to keep.
    pub only: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    Test,
    Lint,
    Build,
    Release,
    Docs,
}

fn categorize(task: &str) -> Option<Category> {
    let first = task.split(':').next().unwrap_or(task).to_lowercase();
    Some(match first.as_str() {
        "test" | "tests" | "spec" | "e2e" | "coverage" | "bench" => Category::Test,
        "lint" | "fmt" | "format" | "check" | "clippy" | "audit" | "typecheck" => Category::Lint,
        "build" | "gen" | "generate" | "codegen" | "compile" | "dev" | "run" | "serve"
        | "start" => Category::Build,
        "release" | "publish" | "deploy" | "changelog" | "version" | "dist" => Category::Release,
        "doc" | "docs" => Category::Docs,
        _ => return None,
    })
}

fn target_modes(cat: Category, minimal: bool) -> &'static [&'static str] {
    match (minimal, cat) {
        (false, Category::Test) => &["test"],
        (false, Category::Lint) => &["review"],
        (false, Category::Build) => &["implement"],
        (false, Category::Release) => &["release"],
        (false, Category::Docs) => &["explore", "design"],
        (true, Category::Docs) => &["plan"],
        (true, Category::Build) => &["build"],
        (true, Category::Test | Category::Lint) => &["verify"],
        (true, Category::Release) => &[],
    }
}

fn find_task<'a>(tasks: &'a [MiseTask], names: &[&str]) -> Option<&'a MiseTask> {
    let prefixed = |t: &&MiseTask| names.iter().any(|n| t.name.starts_with(&format!("{n}:")));
    names
        .iter()
        .find_map(|n| tasks.iter().find(|t| t.name == *n))
        .or_else(|| tasks.iter().find(prefixed))
}

fn lint_task(tasks: &[MiseTask]) -> Option<&MiseTask> {
    let is_lint = |t: &&MiseTask| t.name == "lint" || t.name.starts_with("lint:");
    tasks
        .iter()
        .filter(is_lint)
        .find(|t| t.name.contains("fix"))
        .or_else(|| find_task(tasks, &["lint"]))
}

fn hook(task: &MiseTask) -> toml::Value {
    let mut t = toml::Table::new();
    t.insert("run".into(), format!("mise run {}", task.name).into());
    t.insert("on_failure".into(), "fix_turn".into());
    toml::Value::Table(t)
}

fn strings(v: &toml::Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Kept modes reachable from `start` by walking through removed ones.
fn bridge(
    start: &[String],
    kept: &HashSet<String>,
    all: &toml::Table,
    visited: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    for n in start {
        if kept.contains(n) {
            if !out.contains(n) {
                out.push(n.clone());
            }
        } else if visited.insert(n.clone()) {
            let next = all.get(n).and_then(|m| m.get("next")).map(strings);
            bridge(&next.unwrap_or_default(), kept, all, visited, out);
        }
    }
}

/// Render a `modes.toml` for the project's tasks.
pub fn generate(tasks: &[MiseTask], opts: &InitOptions) -> Result<String> {
    let template = if opts.minimal { MINIMAL } else { FULL };
    let mut doc: toml::Table = template.parse().context("parsing template")?;
    let Some(toml::Value::Table(all)) = doc.remove("modes") else {
        bail!("template has no modes");
    };

    let kept: Vec<String> = match &opts.only {
        None => all.keys().cloned().collect(),
        Some(only) => {
            if let Some(bad) = only.iter().find(|o| !all.contains_key(*o)) {
                bail!(
                    "unknown mode `{bad}`; available: {}",
                    all.keys().cloned().collect::<Vec<_>>().join(", ")
                );
            }
            all.keys().filter(|k| only.contains(k)).cloned().collect()
        }
    };
    if kept.is_empty() {
        bail!("no modes selected");
    }
    let kept_set: HashSet<String> = kept.iter().cloned().collect();

    let mut modes = toml::Table::new();
    for id in &kept {
        let mut m = all[id].as_table().cloned().unwrap_or_default();
        for key in ["next", "auto_advance_to"] {
            if let Some(v) = m.get(key) {
                let mut out = Vec::new();
                let mut visited = HashSet::from([id.clone()]);
                bridge(&strings(v), &kept_set, &all, &mut visited, &mut out);
                out.retain(|o| o != id);
                if out.is_empty() {
                    m.remove(key);
                } else {
                    m.insert(key.into(), out.into());
                }
            }
        }
        modes.insert(id.clone(), toml::Value::Table(m));
    }

    let mut assigned: BTreeSet<&str> = BTreeSet::new();
    let mut per_mode: Vec<(String, Vec<String>)> =
        kept.iter().map(|k| (k.clone(), vec![])).collect();
    for t in tasks {
        let Some(cat) = categorize(&t.name) else {
            continue;
        };
        for target in target_modes(cat, opts.minimal) {
            if let Some((_, list)) = per_mode.iter_mut().find(|(k, _)| k == target) {
                list.push(t.name.clone());
                assigned.insert(&t.name);
            }
        }
    }
    for (id, list) in per_mode {
        modes[&id]
            .as_table_mut()
            .expect("mode table")
            .insert("mise_tasks".into(), list.into());
    }

    let (impl_mode, fmt_modes): (&str, &[&str]) = if opts.minimal {
        ("build", &["build"])
    } else {
        ("implement", &["implement", "test"])
    };
    let mut add_hook = |mode: &str, task: &MiseTask| {
        if let Some(toml::Value::Table(m)) = modes.get_mut(mode) {
            let hooks = m
                .entry("hooks")
                .or_insert_with(|| toml::Value::Table(Default::default()));
            if let Some(h) = hooks.as_table_mut() {
                let arr = h
                    .entry("post_edit")
                    .or_insert_with(|| toml::Value::Array(vec![]));
                if let Some(a) = arr.as_array_mut() {
                    a.push(hook(task));
                }
            }
        }
    };
    if let Some(fmt) = find_task(tasks, &["fmt", "format"]) {
        for m in fmt_modes {
            add_hook(m, fmt);
        }
    }
    if let Some(lint) = lint_task(tasks) {
        add_hook(impl_mode, lint);
    }

    let default_mode = doc
        .get("default_mode")
        .and_then(|v| v.as_str())
        .filter(|d| kept_set.contains(*d))
        .unwrap_or(&kept[0])
        .to_string();
    let mut out_doc = toml::Table::new();
    out_doc.insert("default_mode".into(), default_mode.into());
    out_doc.insert("modes".into(), toml::Value::Table(modes));

    let mut out = String::from(
        "# Generated by `ez init`. Edit freely; run `ez init --diff` to compare with the current templates.\n\n",
    );
    let mut body = toml::to_string(&out_doc)?;
    // Comments do not survive the TOML round trip; re-add the model/profile hints.
    for (mode, hint) in [
        (
            "explore",
            concat!(
                "# model = \"claude-haiku-5-5\"   # model id for this mode (overrides the profile's model)\n",
                "# mcp_tools = [\"mcp__*\"]   # expose MCP server tools to this mode (globs over mcp__<server>__<tool>)\n",
            ),
        ),
        (
            "design",
            "# profile = \"openai\"   # use another configured profile (provider) for this mode\n",
        ),
        (
            "implement",
            "# mcp_tools = [\"mcp__github__*\"]   # expose only some MCP tools to this mode\n",
        ),
    ] {
        let header = format!("[modes.{mode}]\n");
        if let Some(pos) = body.find(&header) {
            body.insert_str(pos + header.len(), hint);
        }
    }
    out.push_str(&body);
    let unassigned: Vec<&str> = tasks
        .iter()
        .map(|t| t.name.as_str())
        .filter(|n| !assigned.contains(n))
        .collect();
    if !unassigned.is_empty() {
        out.push_str(&format!(
            "\n# Unassigned mise tasks: {}\n",
            unassigned.join(", ")
        ));
    }
    Ok(out)
}

/// Unified diff from `existing` to `generated`.
pub fn diff(existing: &str, generated: &str) -> String {
    similar::TextDiff::from_lines(existing, generated)
        .unified_diff()
        .context_radius(3)
        .header("existing", "generated")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::ModeSet;

    fn task(name: &str) -> MiseTask {
        serde_json::from_value(serde_json::json!({"name": name})).unwrap()
    }

    fn fixture() -> Vec<MiseTask> {
        [
            "test",
            "test:unit",
            "lint",
            "fmt",
            "build",
            "release",
            "docs",
            "weird",
        ]
        .map(task)
        .to_vec()
    }

    #[test]
    fn full_template_assigns_tasks() {
        let out = generate(&fixture(), &InitOptions::default()).unwrap();
        assert!(out.starts_with("# Generated by `ez init`"));
        assert!(out.contains("# mcp_tools = [\"mcp__*\"]"));
        assert!(out.contains("# mcp_tools = [\"mcp__github__*\"]"));
        assert!(out.trim_end().ends_with("# Unassigned mise tasks: weird"));
        let set = ModeSet::from_toml_str(&out).unwrap();
        assert_eq!(
            set.ids(),
            [
                "explore",
                "design",
                "implement",
                "test",
                "review",
                "release"
            ]
        );
        let tasks = |m: &str| set.get(m).unwrap().mise_tasks.clone();
        assert_eq!(tasks("test"), ["test", "test:unit"]);
        assert_eq!(tasks("review"), ["lint", "fmt"]);
        assert_eq!(tasks("implement"), ["build"]);
        assert_eq!(tasks("release"), ["release"]);
        assert_eq!(tasks("explore"), ["docs"]);
        assert_eq!(tasks("design"), ["docs"]);
        let hooks = |m: &str| -> Vec<String> {
            set.get(m)
                .unwrap()
                .hooks
                .post_edit
                .iter()
                .map(|h| h.run.clone())
                .collect()
        };
        assert_eq!(hooks("implement"), ["mise run fmt", "mise run lint"]);
        assert_eq!(hooks("test"), ["mise run fmt"]);
        assert!(hooks("review").is_empty());
        let h = &set.get("test").unwrap().hooks.post_edit[0];
        assert_eq!(h.on_failure, ez_hooks::OnFailure::FixTurn);
        assert_eq!(set.get("test").unwrap().next, ["implement", "review"]);
    }

    #[test]
    fn minimal_template() {
        let opts = InitOptions {
            minimal: true,
            only: None,
        };
        let set = ModeSet::from_toml_str(&generate(&fixture(), &opts).unwrap()).unwrap();
        assert_eq!(set.ids(), ["plan", "build", "verify"]);
        assert_eq!(set.get("plan").unwrap().mise_tasks, ["docs"]);
        assert_eq!(set.get("build").unwrap().mise_tasks, ["build"]);
        assert_eq!(
            set.get("verify").unwrap().mise_tasks,
            ["test", "test:unit", "lint", "fmt"]
        );
        assert_eq!(
            set.get("build").unwrap().hooks.post_edit[0].run,
            "mise run fmt"
        );
    }

    #[test]
    fn subset_bridges_transitions() {
        let opts = InitOptions {
            minimal: false,
            only: Some(vec!["explore".into(), "implement".into(), "review".into()]),
        };
        let out = generate(&fixture(), &opts).unwrap();
        let set = ModeSet::from_toml_str(&out).unwrap();
        assert_eq!(set.ids(), ["explore", "implement", "review"]);
        assert_eq!(set.get("explore").unwrap().next, ["implement"]);
        assert_eq!(set.get("implement").unwrap().next, ["review"]);
        assert!(out.contains("# Unassigned mise tasks: test, test:unit, release, weird"));
        assert!(
            generate(
                &[],
                &InitOptions {
                    minimal: false,
                    only: Some(vec!["x".into()])
                }
            )
            .is_err()
        );
    }

    #[test]
    fn prefers_lint_fix_and_diff() {
        let tasks = [task("lint"), task("lint:fix"), task("format:check")]
            .map(|t| t)
            .to_vec();
        let out = generate(&tasks, &InitOptions::default()).unwrap();
        let set = ModeSet::from_toml_str(&out).unwrap();
        let runs: Vec<_> = set
            .get("implement")
            .unwrap()
            .hooks
            .post_edit
            .iter()
            .map(|h| h.run.as_str())
            .collect();
        assert_eq!(runs, ["mise run format:check", "mise run lint:fix"]);
        let d = diff("a\nb\n", "a\nc\n");
        assert!(d.contains("-b") && d.contains("+c"));
        assert!(
            generate(&[], &InitOptions::default())
                .unwrap()
                .find("Unassigned")
                .is_none()
        );
    }
}
