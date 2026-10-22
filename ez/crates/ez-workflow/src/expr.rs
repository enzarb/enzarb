//! Templates and expressions: MiniJinja, one language for both (ADR-0029 §2).
//!
//! Strict undefined (an undefined variable is an error, not an empty string), bounded fuel,
//! and shell quoting for `command.run` templates.

use minijinja::value::Value;
use minijinja::{AutoEscape, Environment, Error, ErrorKind, UndefinedBehavior};
use std::collections::BTreeSet;

/// Instructions one render or evaluation may execute.
pub const FUEL: u64 = 100_000;
/// Longest rendered template, in bytes.
pub const MAX_RENDERED_BYTES: usize = 256 * 1024;

/// Context roots every node template sees.
pub const NODE_ROOTS: &[&str] = &["event", "subject", "nodes", "run"];
/// Context roots a trigger `if` sees (no run exists yet).
pub const TRIGGER_ROOTS: &[&str] = &["event", "subject"];
/// MiniJinja globals that are allowed as bare names.
const GLOBALS: &[&str] = &["range", "dict", "namespace", "debug"];

const SHELL: &str = "shell";
const SHELL_SUFFIX: &str = ".sh";

/// What a template renders into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateKind {
    /// Plain text (instructions, comment bodies, titles, labels, mise args).
    Text,
    /// A shell script: values are single-quoted unless marked `| safe`.
    Shell,
}

impl TemplateKind {
    fn name(self) -> &'static str {
        match self {
            Self::Text => "template",
            Self::Shell => "run.sh",
        }
    }
}

/// The template engine. Cheap to share; build once.
pub struct Engine {
    env: Environment<'static>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Engine")
    }
}

impl Engine {
    pub fn new() -> Self {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        env.set_fuel(Some(FUEL));
        env.set_recursion_limit(64);
        env.set_auto_escape_callback(|name| {
            if name.ends_with(SHELL_SUFFIX) {
                AutoEscape::Custom(SHELL)
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(|out, state, value| {
            if state.auto_escape() == AutoEscape::Custom(SHELL) && !value.is_safe() {
                if value.is_undefined() {
                    return Err(Error::new(ErrorKind::UndefinedError, "undefined value"));
                }
                let s = match value.as_str() {
                    Some(s) => s.to_string(),
                    None => value.to_string(),
                };
                out.write_str(&shell_quote(&s)).map_err(Error::from)
            } else {
                minijinja::escape_formatter(out, state, value)
            }
        });
        Self { env }
    }

    /// Renders `src` with `ctx`.
    pub fn render(
        &self,
        kind: TemplateKind,
        src: &str,
        ctx: &serde_json::Value,
    ) -> Result<String, String> {
        let tmpl = self
            .env
            .template_from_named_str(kind.name(), src)
            .map_err(describe)?;
        let out = tmpl.render(Value::from_serialize(ctx)).map_err(describe)?;
        if out.len() > MAX_RENDERED_BYTES {
            return Err(format!(
                "rendered text is {} bytes; the limit is {MAX_RENDERED_BYTES}",
                out.len()
            ));
        }
        Ok(out)
    }

    /// Evaluates an `if` expression. The result must be a boolean (or none/undefined-free
    /// truthiness of a defined value).
    pub fn eval_bool(&self, src: &str, ctx: &serde_json::Value) -> Result<bool, String> {
        let expr = self.env.compile_expression(src).map_err(describe)?;
        let v = expr.eval(Value::from_serialize(ctx)).map_err(describe)?;
        if v.is_undefined() {
            return Err(format!("`{src}` is undefined"));
        }
        Ok(v.is_true())
    }

    /// Parses a template and returns the variables it reads from the context, as dotted
    /// paths (`event.issue.title`, `nodes.triage.result`).
    pub fn template_vars(&self, kind: TemplateKind, src: &str) -> Result<BTreeSet<String>, String> {
        let tmpl = self
            .env
            .template_from_named_str(kind.name(), src)
            .map_err(describe)?;
        Ok(filter_globals(tmpl.undeclared_variables(true)))
    }

    /// Parses an expression and returns the variables it reads, as dotted paths.
    pub fn expr_vars(&self, src: &str) -> Result<BTreeSet<String>, String> {
        if src.trim().is_empty() {
            return Err("empty expression".into());
        }
        let expr = self.env.compile_expression(src).map_err(describe)?;
        Ok(filter_globals(expr.undeclared_variables(true)))
    }
}

fn filter_globals(vars: impl IntoIterator<Item = String>) -> BTreeSet<String> {
    vars.into_iter()
        .filter(|v| !GLOBALS.contains(&root_of(v)))
        .collect()
}

/// The first segment of a dotted path.
pub fn root_of(path: &str) -> &str {
    path.split('.').next().unwrap_or(path)
}

/// One-line error text: kind, detail and the line, without the debug source dump.
fn describe(e: Error) -> String {
    let mut s = match e.detail() {
        Some(d) => format!("{}: {d}", e.kind()),
        None => e.kind().to_string(),
    };
    if let Some(line) = e.line() {
        s.push_str(&format!(" (line {line})"));
    }
    s
}

/// POSIX single-quoting.
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_text_with_strict_undefined() {
        let e = Engine::new();
        let ctx = json!({"event": {"issue": {"title": "Crash", "number": 7}}});
        assert_eq!(
            e.render(
                TemplateKind::Text,
                "#{{ event.issue.number }} {{ event.issue.title }}",
                &ctx
            )
            .unwrap(),
            "#7 Crash"
        );
        let err = e
            .render(TemplateKind::Text, "{{ event.issue.body }}", &ctx)
            .unwrap_err();
        assert!(err.contains("undefined"), "{err}");
        let err = e
            .render(TemplateKind::Text, "{{ nope }}", &ctx)
            .unwrap_err();
        assert!(err.contains("undefined"), "{err}");
    }

    #[test]
    fn shell_templates_quote_values() {
        let e = Engine::new();
        let ctx = json!({"event": {"title": "it's $(rm -rf /)", "n": 3}});
        assert_eq!(
            e.render(
                TemplateKind::Shell,
                "echo {{ event.title }} {{ event.n }}",
                &ctx
            )
            .unwrap(),
            r#"echo 'it'\''s $(rm -rf /)' '3'"#
        );
        assert_eq!(
            e.render(TemplateKind::Shell, "echo {{ 'a b' | safe }}", &ctx)
                .unwrap(),
            "echo a b"
        );
        // Text templates don't quote.
        assert_eq!(
            e.render(TemplateKind::Text, "{{ event.title }}", &ctx)
                .unwrap(),
            "it's $(rm -rf /)"
        );
    }

    #[test]
    fn evaluates_conditions() {
        let e = Engine::new();
        let ctx = json!({"result": {"decision": "fix"}, "git": {"pushed": false}});
        assert!(e.eval_bool("result.decision == 'fix'", &ctx).unwrap());
        assert!(!e.eval_bool("git.pushed", &ctx).unwrap());
        assert!(!e.eval_bool("result.decision in ['x', 'y']", &ctx).unwrap());
        assert!(e.eval_bool("missing", &ctx).is_err());
        assert!(e.eval_bool("result.decision ==", &ctx).is_err());
    }

    #[test]
    fn fuel_bounds_loops() {
        let e = Engine::new();
        let err = e
            .render(
                TemplateKind::Text,
                "{% for a in range(10000) %}{% for b in range(10000) %}x{% endfor %}{% endfor %}",
                &json!({}),
            )
            .unwrap_err();
        assert!(err.contains("fuel"), "{err}");
    }

    #[test]
    fn lists_variables_without_globals_or_locals() {
        let e = Engine::new();
        let vars = e
            .template_vars(
                TemplateKind::Text,
                "{% set x = event.a %}{{ x }}{% for l in event.labels %}{{ l.name }}{{ loop.index }}{% endfor %}{{ nodes.triage.result.summary }}{{ range(2) }}",
            )
            .unwrap();
        let roots: BTreeSet<&str> = vars.iter().map(|v| root_of(v)).collect();
        assert_eq!(roots, BTreeSet::from(["event", "nodes"]));
        assert!(vars.iter().any(|v| v.starts_with("nodes.triage")));
        let vars = e
            .expr_vars("result.decision == 'fix' and timed_out")
            .unwrap();
        let roots: BTreeSet<&str> = vars.iter().map(|v| root_of(v)).collect();
        assert_eq!(roots, BTreeSet::from(["result", "timed_out"]));
        assert!(e.expr_vars("  ").is_err());
        assert!(e.template_vars(TemplateKind::Text, "{{ x").is_err());
    }

    #[test]
    fn quotes_shell_strings() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
    }
}
