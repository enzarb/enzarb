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
        e.add_filter("shell_quote", shell_quote);
        e.add_filter("contains", contains);
        e.add_test("contains", contains);
        e.add_filter("regex_match", regex_match);
        e.add_test("regex_match", regex_match);
        e.add_filter("duration", duration);
        e
    })
}

/// `x | shell_quote`: one POSIX shell word (single-quoted).
fn shell_quote(v: JValue) -> String {
    let s = match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string(),
    };
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `haystack | contains(needle)` / `haystack is contains(needle)`: substring
/// for strings, membership for lists, key for maps.
fn contains(hay: JValue, needle: JValue) -> Result<bool, minijinja::Error> {
    if let (Some(h), Some(n)) = (hay.as_str(), needle.as_str()) {
        return Ok(h.contains(n));
    }
    if hay.is_undefined() || hay.is_none() {
        return Ok(false);
    }
    // Lists and maps: `in` semantics.
    let mut it = match hay.try_iter() {
        Ok(it) => it,
        Err(_) => return Ok(false),
    };
    Ok(it.any(|x| x == needle))
}

/// `s | regex_match(pattern)` / `s is regex_match(pattern)`: the pattern
/// (Rust regex syntax) matches somewhere in `s`.
fn regex_match(s: JValue, pattern: &str) -> Result<bool, minijinja::Error> {
    let re = regex::Regex::new(pattern).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("bad regex: {e}"),
        )
    })?;
    Ok(match s.as_str() {
        Some(s) => re.is_match(s),
        None if s.is_undefined() || s.is_none() => false,
        None => re.is_match(&s.to_string()),
    })
}

/// `"14d" | duration` → seconds (integers pass through).
fn duration(v: JValue) -> Result<u64, minijinja::Error> {
    let s = match v.as_str() {
        Some(s) => s.to_string(),
        None => v.to_string(),
    };
    crate::format::parse_duration(&s).ok_or_else(|| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("`{s}` is not a duration (e.g. 90s, 15m, 2h, 14d, 1w)"),
        )
    })
}

/// Whether a string is a template (contains `{{` or `{%`).
pub fn is_template(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

/// The expression of a string that is exactly one `{{ expr }}`.
pub fn sole_expr(s: &str) -> Option<&str> {
    let t = s.trim();
    let inner = t.strip_prefix("{{")?.strip_suffix("}}")?;
    (!inner.contains("{{") && !inner.contains("}}") && !inner.contains("{%")).then(|| inner.trim())
}

/// Renders every string leaf of a YAML value; a leaf that is exactly one
/// `{{ expr }}` evaluates to the expression's JSON value.
pub fn render_value(v: &Value, ctx: &Value) -> Result<Value, TemplateError> {
    Ok(match v {
        Value::String(s) => match sole_expr(s) {
            Some(e) => eval(e, ctx)?,
            None => Value::String(render(s, ctx)?),
        },
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| render_value(x, ctx))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| Ok((k.clone(), render_value(x, ctx)?)))
                .collect::<Result<_, TemplateError>>()?,
        ),
        other => other.clone(),
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
        for root in ["task", "subjects", "facts", "instance"] {
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
    fn filters_and_tests() {
        let ctx = json!({"s": "it's", "l": ["a", "b"], "m": {"k": 1}, "t": "Fix: CRASH"});
        let r = |t: &str| render(t, &ctx).unwrap();
        assert_eq!(r("{{ s | shell_quote }}"), r"'it'\''s'");
        assert_eq!(r("{{ l | tojson }}"), r#"["a","b"]"#);
        assert_eq!(r("{{ t | lower }} {{ s | upper }}"), "fix: crash IT'S");
        assert!(truthy("l is contains('b') and t | contains('CRASH')", &ctx).unwrap());
        assert!(truthy("m | contains('k')", &ctx).unwrap());
        assert!(!truthy("missing | contains('x')", &ctx).unwrap());
        assert!(truthy("t is regex_match('^Fix: [A-Z]+$')", &ctx).unwrap());
        assert!(!truthy("t | regex_match('^fix')", &ctx).unwrap());
        assert!(truthy("t | regex_match('(')", &ctx).is_err());
        assert_eq!(eval("'14d' | duration", &ctx).unwrap(), json!(1_209_600));
        assert_eq!(eval("'1h30m' | duration", &ctx).unwrap(), json!(5400));
        assert_eq!(eval("90 | duration", &ctx).unwrap(), json!(90));
        assert!(eval("'soon' | duration", &ctx).is_err());
        assert_eq!(sole_expr(" {{ a.b }} "), Some("a.b"));
        assert_eq!(sole_expr("x {{ a }}"), None);
        assert_eq!(sole_expr("{{ a }}{{ b }}"), None);
    }

    #[test]
    fn error_lines() {
        let e = render("a\nb\n{{ x | nofilter }}", &json!({})).unwrap_err();
        assert_eq!(e.line, Some(3));
        assert_eq!(find_line("a\n{{ foo.bar }}", "foo.bar"), Some(2));
    }
}
