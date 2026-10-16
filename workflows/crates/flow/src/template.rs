//! Templates and conditions: one language (MiniJinja) over the task document.

use crate::document::{ROOTS, TaskDocument};
use minijinja::{Environment, UndefinedBehavior, Value as JValue};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

fn env() -> &'static Environment<'static> {
    static ENV: OnceLock<Environment<'static>> = OnceLock::new();
    ENV.get_or_init(|| {
        let mut e = Environment::new();
        // `a.b.c` on a missing `a` is undefined (falsy), not an error: the
        // linter catches misspelt roots statically.
        e.set_undefined_behavior(UndefinedBehavior::Chainable);
        e.set_keep_trailing_newline(true);
        e
    })
}

/// A problem in a template or expression, with the 1-based line inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    pub line: Option<usize>,
    pub message: String,
}

fn err(e: minijinja::Error) -> TemplateError {
    let mut message = e.kind().to_string();
    if let Some(d) = e.detail() {
        message = format!("{message}: {d}");
    }
    TemplateError {
        line: e.line(),
        message,
    }
}

/// Renders a template against a JSON context.
pub fn render(src: &str, ctx: &Value) -> Result<String, TemplateError> {
    env()
        .render_str(src, JValue::from_serialize(ctx))
        .map_err(err)
}

/// Evaluates an expression to JSON.
pub fn eval(expr: &str, ctx: &Value) -> Result<Value, TemplateError> {
    let e = env().compile_expression(expr).map_err(err)?;
    let v = e.eval(JValue::from_serialize(ctx)).map_err(err)?;
    if v.is_undefined() || v.is_none() {
        return Ok(Value::Null);
    }
    serde_json::to_value(&v).map_err(|e| TemplateError {
        line: None,
        message: e.to_string(),
    })
}

/// Evaluates a condition with Jinja truthiness.
pub fn truthy(expr: &str, ctx: &Value) -> Result<bool, TemplateError> {
    let e = env().compile_expression(expr).map_err(err)?;
    Ok(e.eval(JValue::from_serialize(ctx)).map_err(err)?.is_true())
}

/// Dotted variable paths a template reads from its context.
pub fn template_vars(src: &str) -> Result<BTreeSet<String>, TemplateError> {
    let t = env().template_from_str(src).map_err(err)?;
    Ok(t.undeclared_variables(true).into_iter().collect())
}

/// Dotted variable paths an expression reads.
pub fn expr_vars(src: &str) -> Result<BTreeSet<String>, TemplateError> {
    let e = env().compile_expression(src).map_err(err)?;
    Ok(e.undeclared_variables(true).into_iter().collect())
}

/// What a template or expression may refer to besides the document roots.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// Extra top-level names (step inputs, `last`, `exit`, …).
    pub names: BTreeSet<String>,
    /// Declared param names (`params.<name>`).
    pub params: BTreeSet<String>,
}

impl Scope {
    pub fn with_names<I: IntoIterator<Item = S>, S: Into<String>>(mut self, names: I) -> Self {
        self.names.extend(names.into_iter().map(Into::into));
        self
    }
}

/// Second-level field names of the typed document roots, from the schema.
fn doc_fields() -> &'static BTreeMap<String, BTreeSet<String>> {
    static F: OnceLock<BTreeMap<String, BTreeSet<String>>> = OnceLock::new();
    F.get_or_init(|| {
        let schema = serde_json::to_value(schemars::schema_for!(TaskDocument)).unwrap_or_default();
        let mut out = BTreeMap::new();
        for root in ["task", "subjects", "facts"] {
            let r = schema["properties"][root]["$ref"]
                .as_str()
                .unwrap_or_default();
            let name = r.rsplit('/').next().unwrap_or_default();
            let props = schema["$defs"][name]["properties"]
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            out.insert(root.to_string(), props);
        }
        out
    })
}

/// Checks variable paths against the document and scope; returns one
/// message per undefined variable.
pub fn check_vars(vars: &BTreeSet<String>, scope: &Scope) -> Vec<String> {
    let mut out = Vec::new();
    for v in vars {
        let mut segs = v.split('.');
        let root = segs.next().unwrap_or_default();
        let second = segs.next();
        if scope.names.contains(root) {
            continue;
        }
        if root == "params" {
            match second {
                Some(p) if !scope.params.contains(p) => out.push(format!(
                    "undefined param `{p}` (declared: {})",
                    list(&scope.params)
                )),
                None => out.push("use `params.<name>`, not `params`".into()),
                _ => {}
            }
            continue;
        }
        if !ROOTS.contains(&root) {
            out.push(format!(
                "undefined variable `{root}`: not a task document root ({}) or a declared name ({})",
                ROOTS.join(", "),
                list(&scope.names)
            ));
            continue;
        }
        if let (Some(fields), Some(s)) = (doc_fields().get(root), second)
            && !fields.contains(s)
        {
            out.push(format!(
                "`{root}.{s}` is not a task document field ({root} has: {})",
                list(fields)
            ));
        }
    }
    out
}

fn list(s: &BTreeSet<String>) -> String {
    if s.is_empty() {
        "none".into()
    } else {
        s.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

/// 1-based line of the first mention of `var`'s root in `src`.
pub fn find_line(src: &str, var: &str) -> Option<usize> {
    let root = var.split('.').next()?;
    src.lines()
        .position(|l| l.contains(var) || l.contains(root))
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope() -> Scope {
        Scope {
            names: ["plan".to_string(), "last".into()].into(),
            params: ["mode".to_string()].into(),
        }
    }

    #[test]
    fn renders_and_evaluates() {
        let ctx = json!({"subjects": {"issue": {"title": "T"}}, "outputs": {"n": 3}});
        assert_eq!(render("# {{ subjects.issue.title }}", &ctx).unwrap(), "# T");
        assert_eq!(render("{{ outputs.missing.deep }}", &ctx).unwrap(), "");
        assert!(truthy("outputs.n > 2", &ctx).unwrap());
        assert!(!truthy("outputs.missing.deep == 'x'", &ctx).unwrap());
        assert_eq!(eval("outputs.n + 1", &ctx).unwrap(), json!(4));
        assert_eq!(eval("outputs.nope", &ctx).unwrap(), Value::Null);
        assert!(render("{{ x }", &ctx).is_err());
        assert!(truthy("1 +", &ctx).is_err());
    }

    #[test]
    fn undefined_variables() {
        let cases: &[(&str, &[&str])] = &[
            ("{{ subjects.issue.title }} {{ plan }}", &[]),
            ("{{ outputs.anything.goes }}", &[]),
            ("{{ params.mode }}", &[]),
            ("{{ params.other }}", &["undefined param `other`"]),
            ("{{ pln }}", &["undefined variable `pln`"]),
            (
                "{{ subjects.isue }}",
                &["`subjects.isue` is not a task document field"],
            ),
            ("{{ facts.ci.state }}{{ facts.cii }}", &["`facts.cii`"]),
            (
                "{% for c in subjects.pull_requests %}{{ c.title }}{% endfor %}",
                &[],
            ),
            ("{% set x = 1 %}{{ x }}", &[]),
        ];
        for (src, want) in cases {
            let got = check_vars(&template_vars(src).unwrap(), &scope());
            assert_eq!(got.len(), want.len(), "{src}: {got:?}");
            for (g, w) in got.iter().zip(*want) {
                assert!(g.starts_with(w) || g.contains(w), "{src}: {g} !~ {w}");
            }
        }
        let got = check_vars(
            &expr_vars("last.exit == 'success' and nope").unwrap(),
            &scope(),
        );
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn error_lines() {
        let e = render("a\nb\n{{ x | nofilter }}", &json!({})).unwrap_err();
        assert_eq!(e.line, Some(3));
        assert_eq!(find_line("a\n{{ foo.bar }}", "foo.bar"), Some(2));
    }
}
