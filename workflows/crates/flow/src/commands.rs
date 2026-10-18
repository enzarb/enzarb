//! Helpers of the command model: where a declared command goes, and the commands a wait
//! already handles in its `if:` conditions (so libraries that declare no `commands` still
//! list and accept what their waits do).

use serde_json::{Map, Value};

/// Where a declared command goes (`CommandDecl::to`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRoute {
    /// A node of the wait's phase.
    Node(String),
    /// Leave the phase through this exit.
    Exit(String),
    /// The phase's first node.
    Start,
    /// This wait again.
    Own,
    /// A top-level phase.
    Goto(String),
}

impl CommandRoute {
    /// Parses `to`; `nodes` decides whether a bare word is a node (else it is `start`, `self`
    /// or an error).
    pub fn parse(to: &str, is_node: impl Fn(&str) -> bool) -> Result<Self, String> {
        let to = to.trim();
        if let Some(e) = to.strip_prefix("exit:") {
            return Ok(Self::Exit(e.trim().to_string()));
        }
        if let Some(p) = to.strip_prefix("goto:") {
            return Ok(Self::Goto(p.trim().to_string()));
        }
        if is_node(to) {
            return Ok(Self::Node(to.to_string()));
        }
        match to {
            "start" => Ok(Self::Start),
            "self" => Ok(Self::Own),
            _ => Err(format!(
                "`{to}` is not a node of the phase, `start`, `self`, `exit:<name>` or `goto:<phase>`"
            )),
        }
    }
}

/// A valid command word: no slash, no whitespace.
pub fn is_command_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The command words an `if:` condition compares `event.command` with: `== 'x'`,
/// `== params.p` (looked up in `params`) and `in ['x', params.p]`. Anything it cannot read
/// statically is skipped. Each word comes with `true` when the condition also requires
/// `can_write`.
pub fn literal_commands(cond: &str, params: &Map<String, Value>) -> Vec<(String, bool)> {
    let writer = cond.contains("can_write");
    let mut out = Vec::new();
    let mut rest = cond;
    while let Some(i) = rest.find("event.command") {
        rest = &rest[i + "event.command".len()..];
        let t = rest.trim_start();
        let operands: Vec<&str> = if let Some(r) = t.strip_prefix("==") {
            vec![operand(r)]
        } else if let Some(r) = t.strip_prefix("in") {
            let r = r.trim_start();
            match r.strip_prefix('[').and_then(|r| r.split_once(']')) {
                Some((list, _)) => list.split(',').collect(),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        for op in operands {
            let op = op.trim();
            let word = if let Some(p) = op.strip_prefix("params.") {
                params
                    .get(p.trim())
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                op.strip_prefix('\'')
                    .and_then(|r| r.strip_suffix('\''))
                    .or_else(|| op.strip_prefix('"').and_then(|r| r.strip_suffix('"')))
                    .map(str::to_string)
            };
            if let Some(w) = word.filter(|w| is_command_word(w)) {
                out.push((w.to_ascii_lowercase(), writer));
            }
        }
    }
    out
}

/// The first operand of a comparison: up to the next ` and `, ` or `, `)` or the end.
fn operand(s: &str) -> &str {
    let s = s.trim_start();
    let end = [" and ", " or ", ")"]
        .iter()
        .filter_map(|d| s.find(d))
        .min()
        .unwrap_or(s.len());
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params() -> Map<String, Value> {
        json!({"retry_command": "retry", "command": "merge"})
            .as_object()
            .cloned()
            .unwrap()
    }

    #[test]
    fn reads_literals_params_and_lists() {
        let p = params();
        assert_eq!(
            literal_commands("event.command == 'epic' and event.actor.can_write", &p),
            vec![("epic".to_string(), true)]
        );
        assert_eq!(
            literal_commands(
                "event.command in [params.retry_command, params.command]",
                &p
            ),
            vec![("retry".to_string(), false), ("merge".to_string(), false)]
        );
        assert!(literal_commands("not event.command", &p).is_empty());
        assert!(literal_commands("event.type == 'comment'", &p).is_empty());
        assert!(literal_commands("event.command != 'x'", &p).is_empty());
    }

    #[test]
    fn routes() {
        let node = |n: &str| n == "retry";
        assert_eq!(
            CommandRoute::parse("retry", node),
            Ok(CommandRoute::Node("retry".into()))
        );
        assert_eq!(CommandRoute::parse("start", node), Ok(CommandRoute::Start));
        assert_eq!(
            CommandRoute::parse("exit:restart", node),
            Ok(CommandRoute::Exit("restart".into()))
        );
        assert!(CommandRoute::parse("nowhere", node).is_err());
    }
}
