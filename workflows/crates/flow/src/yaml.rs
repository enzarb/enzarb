//! YAML loading with positions.
//!
//! Files are parsed with `yaml-rust2`'s marked event stream into a
//! `serde_json::Value` plus a map from value path (`phases[2].use`) to the
//! 1-based line/column of that key or sequence item. Typed deserialization
//! goes through `serde_path_to_error`, so type errors point at the offending
//! key too.

use crate::diag::Diagnostic;
use serde::de::DeserializeOwned;
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// A parsed YAML file.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// Display name used in diagnostics.
    pub name: String,
    pub value: Value,
    spans: HashMap<String, (usize, usize)>,
}

impl SourceFile {
    /// Parses one YAML document. An empty file is `null`.
    pub fn parse(name: &str, text: &str) -> Result<Self, Diagnostic> {
        let mut b = Builder::default();
        let mut parser = Parser::new_from_str(text);
        if let Err(e) = parser.load(&mut b, false) {
            let m = e.marker();
            return Err(Diagnostic::error(
                name,
                (m.line(), m.col() + 1),
                e.info().to_string(),
            ));
        }
        if let Some((pos, msg)) = b.error {
            return Err(Diagnostic::error(name, pos, msg));
        }
        Ok(Self {
            name: name.to_string(),
            value: b.root.unwrap_or(Value::Null),
            spans: b.spans,
        })
    }

    /// Position of `path`, or of its closest ancestor that has one.
    pub fn pos(&self, path: &str) -> (usize, usize) {
        let mut p = path.to_string();
        loop {
            if let Some(s) = self.spans.get(&p) {
                return *s;
            }
            match p.rfind(['.', '[']) {
                Some(i) => p.truncate(i),
                None => return self.spans.get("").copied().unwrap_or((1, 1)),
            }
        }
    }

    pub fn error(&self, path: &str, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error(&self.name, self.pos(path), msg)
    }

    pub fn warning(&self, path: &str, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::warning(&self.name, self.pos(path), msg)
    }

    /// Deserializes the whole file into `T`, locating errors.
    pub fn typed<T: DeserializeOwned>(&self) -> Result<T, Diagnostic> {
        typed_at(self, "", self.value.clone())
    }
}

/// Deserializes `value` (found at `base` in `file`) into `T`.
pub fn typed_at<T: DeserializeOwned>(
    file: &SourceFile,
    base: &str,
    value: Value,
) -> Result<T, Diagnostic> {
    serde_path_to_error::deserialize::<_, T>(value).map_err(|e| {
        let mut path = base.to_string();
        for seg in e.path().iter() {
            use serde_path_to_error::Segment;
            match seg {
                Segment::Seq { index } => path.push_str(&format!("[{index}]")),
                Segment::Map { key } => join(&mut path, key),
                Segment::Enum { .. } | Segment::Unknown => {}
            }
        }
        let msg = e.inner().to_string();
        // Point unknown-field errors at the field itself.
        if let Some(field) = msg
            .strip_prefix("unknown field `")
            .and_then(|r| r.split('`').next())
        {
            let mut p = path.clone();
            join(&mut p, field);
            return file.error(&p, msg);
        }
        file.error(&path, msg)
    })
}

pub fn join(path: &mut String, key: &str) {
    if !path.is_empty() {
        path.push('.');
    }
    path.push_str(key);
}

enum Frame {
    Seq(Vec<Value>, String, usize),
    Map(Map<String, Value>, String, Option<String>, usize),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    root: Option<Value>,
    spans: HashMap<String, (usize, usize)>,
    anchors: HashMap<usize, Value>,
    error: Option<((usize, usize), String)>,
}

impl Builder {
    /// Path the next value will be stored at.
    fn next_path(&self) -> Option<String> {
        match self.stack.last() {
            None => Some(String::new()),
            Some(Frame::Seq(items, path, _)) => Some(format!("{path}[{}]", items.len())),
            Some(Frame::Map(_, path, Some(key), _)) => {
                let mut p = path.clone();
                join(&mut p, key);
                Some(p)
            }
            Some(Frame::Map(_, _, None, _)) => None, // a key comes next
        }
    }

    fn push_value(&mut self, v: Value, anchor: usize, mark: Marker) {
        if anchor > 0 {
            self.anchors.insert(anchor, v.clone());
        }
        let pos = (mark.line(), mark.col() + 1);
        match self.stack.last_mut() {
            None => {
                self.spans.entry(String::new()).or_insert(pos);
                self.root = Some(v);
            }
            Some(Frame::Seq(items, path, _)) => {
                self.spans
                    .entry(format!("{path}[{}]", items.len()))
                    .or_insert(pos);
                items.push(v);
            }
            Some(Frame::Map(map, path, key, _)) => match key.take() {
                None => {
                    let k = match v {
                        Value::String(s) => s,
                        Value::Null => "null".into(),
                        other => other.to_string(),
                    };
                    let mut p = path.clone();
                    join(&mut p, &k);
                    if map.contains_key(&k) && self.error.is_none() {
                        self.error = Some((pos, format!("duplicate key `{k}`")));
                    }
                    self.spans.insert(p, pos);
                    *key = Some(k);
                }
                Some(k) => {
                    map.insert(k, v);
                }
            },
        }
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, ev: Event, mark: Marker) {
        match ev {
            Event::Scalar(s, style, anchor, tag) => {
                let v = if style == TScalarStyle::Plain && tag.is_none() {
                    plain(&s)
                } else {
                    Value::String(s)
                };
                self.push_value(v, anchor, mark);
            }
            Event::SequenceStart(anchor, _) => {
                let path = self.next_path().unwrap_or_default();
                if let Some(Frame::Map(_, _, None, _)) = self.stack.last() {
                    self.error.get_or_insert((
                        (mark.line(), mark.col() + 1),
                        "complex mapping keys are not supported".into(),
                    ));
                }
                if matches!(self.stack.last(), None | Some(Frame::Seq(..))) {
                    self.spans
                        .entry(path.clone())
                        .or_insert((mark.line(), mark.col() + 1));
                }
                self.stack.push(Frame::Seq(Vec::new(), path, anchor));
            }
            Event::SequenceEnd => {
                if let Some(Frame::Seq(items, _, anchor)) = self.stack.pop() {
                    self.push_value(Value::Array(items), anchor, mark);
                }
            }
            Event::MappingStart(anchor, _) => {
                let path = self.next_path().unwrap_or_default();
                if let Some(Frame::Map(_, _, None, _)) = self.stack.last() {
                    self.error.get_or_insert((
                        (mark.line(), mark.col() + 1),
                        "complex mapping keys are not supported".into(),
                    ));
                }
                if matches!(self.stack.last(), None | Some(Frame::Seq(..))) {
                    self.spans
                        .entry(path.clone())
                        .or_insert((mark.line(), mark.col() + 1));
                }
                self.stack.push(Frame::Map(Map::new(), path, None, anchor));
            }
            Event::MappingEnd => {
                if let Some(Frame::Map(map, _, _, anchor)) = self.stack.pop() {
                    self.push_value(Value::Object(map), anchor, mark);
                }
            }
            Event::Alias(id) => {
                let v = self.anchors.get(&id).cloned().unwrap_or(Value::Null);
                self.push_value(v, 0, mark);
            }
            _ => {}
        }
    }
}

/// YAML 1.2 core schema resolution for plain scalars.
fn plain(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::Number(i.into());
    }
    if let Some(hex) = s.strip_prefix("0x")
        && let Ok(i) = i64::from_str_radix(hex, 16)
    {
        return Value::Number(i.into());
    }
    let looks_numeric = s
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.');
    if looks_numeric
        && let Ok(f) = s.parse::<f64>()
        && f.is_finite()
        && let Some(n) = Number::from_f64(f)
    {
        return Value::Number(n);
    }
    Value::String(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn values_and_spans() {
        let f = SourceFile::parse("a.yaml", "a: 1\nb:\n  - x\n  - {c: true, d: 'yes'}\ne: ~\n")
            .unwrap();
        assert_eq!(
            f.value,
            serde_json::json!({"a": 1, "b": ["x", {"c": true, "d": "yes"}], "e": null})
        );
        assert_eq!(f.pos("a"), (1, 1));
        assert_eq!(f.pos("b[0]"), (3, 5));
        assert_eq!(f.pos("b[1].d"), (4, 15));
        assert_eq!(f.pos("b[1].zzz"), (4, 5));
    }

    #[test]
    fn duplicate_key_and_syntax_errors() {
        let e = SourceFile::parse("a.yaml", "a: 1\na: 2\n").unwrap_err();
        assert_eq!((e.line, e.message.as_str()), (2, "duplicate key `a`"));
        let e = SourceFile::parse("a.yaml", "a: [1\n").unwrap_err();
        assert!(e.line >= 1, "{e}");
    }

    #[test]
    fn typed_errors_point_at_key() {
        #[derive(Deserialize, Debug)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct T {
            a: u32,
            b: Option<Vec<u32>>,
        }
        let f = SourceFile::parse("t.yaml", "a: 1\nb:\n  - 1\n  - nope\n").unwrap();
        let e = f.typed::<T>().unwrap_err();
        assert_eq!((e.line, e.col), (4, 5), "{e}");
        let f = SourceFile::parse("t.yaml", "a: 1\nzz: 2\n").unwrap();
        let e = f.typed::<T>().unwrap_err();
        assert_eq!((e.line, e.col), (2, 1), "{e}");
    }

    #[test]
    fn anchors() {
        let f = SourceFile::parse("a.yaml", "x: &a {k: 1}\ny: *a\n").unwrap();
        assert_eq!(f.value["y"], serde_json::json!({"k": 1}));
    }
}
