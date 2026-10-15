//! Edge conditions: a tiny expression language over the outputs of the node
//! that just ran. Never arbitrary code.
//!
//! ```text
//! expr    = or
//! or      = and ("||" and)*
//! and     = unary ("&&" unary)*
//! unary   = "!" unary | cmp
//! cmp     = primary (("==" | "!=") primary)?
//! primary = ident | number | string | true | false | null | "(" expr ")"
//! ident   = [A-Za-z_][A-Za-z0-9_]* ("." [A-Za-z_][A-Za-z0-9_]*)*
//! ```
//!
//! An identifier names a field of the output object (dots walk nested
//! objects); a missing field is `null`. Truthiness: `null`, `false`, `0`, `""`
//! and empty arrays/objects are false, everything else true. `==` compares
//! numbers numerically and everything else by JSON equality.

use serde_json::Value;

/// A parsed condition.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Value),
    Ident(Vec<String>),
    Not(Box<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Ne(Box<Expr>, Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

/// Why a condition does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("condition `{expr}`: {reason}")]
pub struct ExprError {
    pub expr: String,
    pub reason: String,
}

/// Longest condition accepted, in bytes.
pub const MAX_EXPR_LEN: usize = 512;
/// Deepest nesting accepted (parentheses and `!`).
const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(Vec<String>),
    Lit(Value),
    Eq,
    Ne,
    And,
    Or,
    Not,
    LParen,
    RParen,
}

fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        match c {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b'=' if b.get(i + 1) == Some(&b'=') => {
                out.push(Tok::Eq);
                i += 2;
            }
            b'!' if b.get(i + 1) == Some(&b'=') => {
                out.push(Tok::Ne);
                i += 2;
            }
            b'!' => {
                out.push(Tok::Not);
                i += 1;
            }
            b'&' if b.get(i + 1) == Some(&b'&') => {
                out.push(Tok::And);
                i += 2;
            }
            b'|' if b.get(i + 1) == Some(&b'|') => {
                out.push(Tok::Or);
                i += 2;
            }
            b'"' | b'\'' => {
                let q = c;
                let mut j = i + 1;
                let mut lit = String::new();
                loop {
                    match b.get(j) {
                        None => return Err("unterminated string".into()),
                        Some(&x) if x == q => break,
                        Some(b'\\') => {
                            match b.get(j + 1) {
                                Some(&e @ (b'\\' | b'"' | b'\'')) => lit.push(e as char),
                                Some(b'n') => lit.push('\n'),
                                _ => return Err("bad escape in string".into()),
                            }
                            j += 2;
                        }
                        Some(_) => {
                            // Copy one UTF-8 character.
                            let ch = s[j..].chars().next().expect("in bounds");
                            lit.push(ch);
                            j += ch.len_utf8();
                        }
                    }
                }
                out.push(Tok::Lit(Value::String(lit)));
                i = j + 1;
            }
            b'0'..=b'9' | b'-' => {
                let start = i;
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                let text = &s[start..i];
                let v = if let Ok(n) = text.parse::<i64>() {
                    Value::from(n)
                } else {
                    match text.parse::<f64>() {
                        Ok(f) if f.is_finite() => Value::from(f),
                        _ => return Err(format!("bad number `{text}`")),
                    }
                };
                out.push(Tok::Lit(v));
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.')
                {
                    i += 1;
                }
                let word = &s[start..i];
                out.push(match word {
                    "true" => Tok::Lit(Value::Bool(true)),
                    "false" => Tok::Lit(Value::Bool(false)),
                    "null" => Tok::Lit(Value::Null),
                    _ => {
                        let parts: Vec<String> = word.split('.').map(str::to_string).collect();
                        if parts
                            .iter()
                            .any(|p| p.is_empty() || p.as_bytes()[0].is_ascii_digit())
                        {
                            return Err(format!("bad identifier `{word}`"));
                        }
                        Tok::Ident(parts)
                    }
                });
            }
            _ => {
                return Err(format!(
                    "unexpected character `{}`",
                    s[i..].chars().next().unwrap()
                ));
            }
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.peek() == Some(t) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn deeper(&mut self) -> Result<(), String> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err("nested too deeply".into());
        }
        Ok(())
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.eat(&Tok::Or) {
            l = Expr::Or(Box::new(l), Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.unary()?;
        while self.eat(&Tok::And) {
            l = Expr::And(Box::new(l), Box::new(self.unary()?));
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr, String> {
        if self.eat(&Tok::Not) {
            self.deeper()?;
            let e = self.unary()?;
            self.depth -= 1;
            return Ok(Expr::Not(Box::new(e)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let l = self.primary()?;
        if self.eat(&Tok::Eq) {
            return Ok(Expr::Eq(Box::new(l), Box::new(self.primary()?)));
        }
        if self.eat(&Tok::Ne) {
            return Ok(Expr::Ne(Box::new(l), Box::new(self.primary()?)));
        }
        Ok(l)
    }

    fn primary(&mut self) -> Result<Expr, String> {
        let t = self.peek().cloned();
        self.pos += 1;
        match t {
            Some(Tok::Ident(p)) => Ok(Expr::Ident(p)),
            Some(Tok::Lit(v)) => Ok(Expr::Lit(v)),
            Some(Tok::LParen) => {
                self.deeper()?;
                let e = self.or()?;
                self.depth -= 1;
                if !self.eat(&Tok::RParen) {
                    return Err("missing `)`".into());
                }
                Ok(e)
            }
            Some(t) => Err(format!("unexpected {t:?}")),
            None => Err("unexpected end".into()),
        }
    }
}

/// Parses a condition.
pub fn parse(src: &str) -> Result<Expr, ExprError> {
    let err = |reason: String| ExprError {
        expr: src.to_string(),
        reason,
    };
    if src.len() > MAX_EXPR_LEN {
        return Err(err(format!("longer than {MAX_EXPR_LEN} bytes")));
    }
    let toks = lex(src).map_err(err)?;
    if toks.is_empty() {
        return Err(err("empty".into()));
    }
    let mut p = Parser {
        toks,
        pos: 0,
        depth: 0,
    };
    let e = p.or().map_err(err)?;
    if p.pos != p.toks.len() {
        return Err(err(format!("unexpected {:?}", p.toks[p.pos])));
    }
    Ok(e)
}

/// JSON truthiness (see the module docs).
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

fn value(e: &Expr, ctx: &Value) -> Value {
    match e {
        Expr::Lit(v) => v.clone(),
        Expr::Ident(path) => {
            let mut cur = ctx;
            for p in path {
                match cur.get(p) {
                    Some(v) => cur = v,
                    None => return Value::Null,
                }
            }
            cur.clone()
        }
        _ => Value::Bool(eval(e, ctx)),
    }
}

/// Evaluates `e` against `ctx` (the output of the node that just ran).
pub fn eval(e: &Expr, ctx: &Value) -> bool {
    match e {
        Expr::Lit(_) | Expr::Ident(_) => truthy(&value(e, ctx)),
        Expr::Not(x) => !eval(x, ctx),
        Expr::Eq(a, b) => equal(&value(a, ctx), &value(b, ctx)),
        Expr::Ne(a, b) => !equal(&value(a, ctx), &value(b, ctx)),
        Expr::And(a, b) => eval(a, ctx) && eval(b, ctx),
        Expr::Or(a, b) => eval(a, ctx) || eval(b, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn evaluates() {
        let ctx = json!({
            "pushed": true, "exit_code": 0, "decision": "approve", "empty": "",
            "n": 2.0, "list": [], "nested": {"ok": true, "code": 3}, "nil": null
        });
        let cases: &[(&str, bool)] = &[
            ("pushed", true),
            ("!pushed", false),
            ("missing", false),
            ("!missing", true),
            ("empty", false),
            ("list", false),
            ("nil", false),
            ("exit_code", false),
            ("exit_code == 0", true),
            ("exit_code != 0", false),
            ("n == 2", true),
            ("n != 2", false),
            ("decision == \"approve\"", true),
            ("decision == 'approve'", true),
            ("decision != \"approve\"", false),
            ("decision == \"reject\"", false),
            ("pushed && exit_code == 0", true),
            ("pushed && missing", false),
            ("missing || pushed", true),
            ("missing || nil", false),
            ("!(pushed && missing)", true),
            ("!!pushed", true),
            ("nested.ok", true),
            ("nested.code == 3", true),
            ("nested.missing.deep", false),
            ("pushed == true", true),
            ("missing == null", true),
            ("true", true),
            ("false || false", false),
            ("a || b && c", false),
            ("pushed || missing && nil", true),
            ("-1 == -1", true),
            ("decision == \"a\\\"b\"", false),
        ];
        for (src, want) in cases {
            let e = parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert_eq!(eval(&e, &ctx), *want, "{src}");
        }
    }

    #[test]
    fn rejects() {
        for src in [
            "", "   ", "a ==", "== a", "(a", "a)", "a b", "a = b", "a & b", "a | b", "\"open",
            "a.", "a..b", "1.2.3", "f(x)", "a + b", "a; b", "$x",
        ] {
            assert!(parse(src).is_err(), "{src:?} should not parse");
        }
        assert!(parse(&"a || ".repeat(200)).is_err());
        assert!(parse(&format!("{}a{}", "(".repeat(40), ")".repeat(40))).is_err());
        assert!(parse(&"!".repeat(40)).is_err());
    }
}
