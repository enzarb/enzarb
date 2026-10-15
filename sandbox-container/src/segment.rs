//! Graph segment runs (ADR-0020, `RunStart.graph`): walk a workflow graph
//! from the segment's start node, running consecutive `command` and `agent`
//! nodes in the run's worktree, and stop at the first `wait`, `action` or
//! `end` node. The graph logic itself is `enzarb-graph`, shared with the
//! workflows kernel; this module only drives it.

use std::collections::BTreeMap;

use enzarb_graph::{Definition, GraphState, Next, NodeKind, advance};
use enzarb_proto::Usage;
use serde_json::{Map, Value, json};

/// Bytes of a command's stdout kept in its output.
pub const STDOUT_TAIL: usize = 4 * 1024;
/// Bytes of an agent's message kept in its output (`summary`).
pub const SUMMARY_CAP: usize = 64 * 1024;

/// Where a segment ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// At a kernel node (`wait`, `action` or `end`).
    At(String),
    /// The graph failed after `node` ran (see [`enzarb_graph::Next::Failed`]).
    Failed { node: String, reason: String },
}

/// Runs one sandbox node. Implemented by the run manager (and by fakes in
/// tests).
pub trait Steps {
    type Err;
    /// Runs `run` with `sh -c`; returns the node output.
    fn command(
        &mut self,
        node: &str,
        run: &str,
    ) -> impl std::future::Future<Output = Result<Value, Self::Err>> + Send;
    /// Runs one agent turn with `prompt`; returns the node output.
    fn agent(
        &mut self,
        node: &str,
        prompt: &str,
    ) -> impl std::future::Future<Output = Result<Value, Self::Err>> + Send;
}

/// Walks from `state.node` (whose visit the sender already counted) until a
/// kernel node or a failure. `prompts` overrides an agent node's `prompt`
/// with the text the kernel rendered.
pub async fn walk<S: Steps>(
    def: &Definition,
    state: &mut GraphState,
    prompts: &BTreeMap<String, String>,
    steps: &mut S,
) -> Result<Stopped, S::Err> {
    loop {
        let id = state.node.clone();
        let Some(node) = def.node(&id) else {
            return Ok(Stopped::Failed {
                reason: format!("node `{id}` does not exist"),
                node: id,
            });
        };
        let out = match &node.kind {
            NodeKind::Command { run } => steps.command(&id, run).await?,
            NodeKind::Agent { prompt } => {
                let p = prompts.get(&id).unwrap_or(prompt);
                steps.agent(&id, p).await?
            }
            NodeKind::Wait { .. } | NodeKind::Action { .. } | NodeKind::End { .. } => {
                return Ok(Stopped::At(id));
            }
        };
        if let Next::Failed(reason) = advance(def, state, out) {
            return Ok(Stopped::Failed { node: id, reason });
        }
    }
}

/// The last `max` bytes of `s`, on a char boundary.
pub fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = s.len() - max;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

/// Output of a `command` node.
pub fn command_output(exit_code: Option<i32>, stdout: &str) -> Value {
    json!({
        "ok": exit_code == Some(0),
        "exit_code": exit_code,
        "stdout": tail(stdout, STDOUT_TAIL),
    })
}

/// The object in the trailing fenced `json` block of an agent message, if
/// any (contracts: "Agent output").
pub fn final_json(text: &str) -> Option<Map<String, Value>> {
    let start = text.rfind("```json")?;
    let body = &text[start + "```json".len()..];
    let end = body.find("```")?;
    match serde_json::from_str::<Value>(body[..end].trim()) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// Output of an `agent` node: the fields of its trailing `json` block, then
/// `ok`, `summary` (capped), `pushed`, `head_sha` and `branch`, which win.
pub fn agent_output(ok: bool, text: &str, pushed: bool, head_sha: &str, branch: &str) -> Value {
    let mut m = final_json(text).unwrap_or_default();
    m.insert("ok".into(), ok.into());
    m.insert("summary".into(), tail(text, SUMMARY_CAP).into());
    m.insert("pushed".into(), pushed.into());
    m.insert("head_sha".into(), head_sha.into());
    m.insert("branch".into(), branch.into());
    Value::Object(m)
}

fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    }
}

/// Sums the usage of two agent turns.
pub fn add_usage(a: Option<Usage>, b: Option<Usage>) -> Option<Usage> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => {
            let mut models = a.models;
            for m in b.models {
                if !models.contains(&m) {
                    models.push(m);
                }
            }
            Some(Usage {
                input_tokens: add(a.input_tokens, b.input_tokens),
                output_tokens: add(a.output_tokens, b.output_tokens),
                cache_read_input_tokens: add(a.cache_read_input_tokens, b.cache_read_input_tokens),
                cache_creation_input_tokens: add(
                    a.cache_creation_input_tokens,
                    b.cache_creation_input_tokens,
                ),
                models,
                cost_usd: match (a.cost_usd, b.cost_usd) {
                    (None, None) => None,
                    (x, y) => Some(x.unwrap_or(0.0) + y.unwrap_or(0.0)),
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEF: &str = r#"
version: 1
name: issue
on: [issue_opened]
start: prepare
nodes:
  prepare: {type: command, run: "prep", optional: true, next: implement}
  implement: {type: agent, prompt: implement, next: {pushed: check, else: failed}}
  check: {type: command, run: "test", next: {"exit_code == 0": open_pr, else: fix}}
  fix: {type: agent, prompt: "fix it", next: check, max_visits: 2}
  open_pr: {type: action, action: open_change_request, next: done}
  failed: {type: end, status: failed}
  done: {type: end, status: done}
"#;

    /// Scripted outputs, recording what ran.
    #[derive(Default)]
    struct Fake {
        commands: BTreeMap<String, Vec<i32>>,
        pushed: bool,
        log: Vec<String>,
    }

    impl Steps for Fake {
        type Err = String;
        async fn command(&mut self, node: &str, run: &str) -> Result<Value, String> {
            self.log.push(format!("{node}:{run}"));
            let codes = self.commands.get_mut(node).ok_or("unscripted")?;
            let c = if codes.len() > 1 {
                codes.remove(0)
            } else {
                codes[0]
            };
            Ok(command_output(Some(c), "out"))
        }
        async fn agent(&mut self, node: &str, prompt: &str) -> Result<Value, String> {
            self.log.push(format!("{node}:{prompt}"));
            Ok(agent_output(true, "x", self.pushed, "abc", "b"))
        }
    }

    fn begin(def: &Definition) -> GraphState {
        GraphState::begin(def).0
    }

    #[tokio::test]
    async fn walks_to_the_first_kernel_node() {
        let def = enzarb_graph::load(DEF).unwrap();
        // (exit codes by command node, agent pushes, expected stop, nodes run)
        type Case<'a> = (&'a [(&'a str, &'a [i32])], bool, Stopped, &'a [&'a str]);
        let cases: &[Case] = &[
            (
                &[("prepare", &[1]), ("check", &[0])],
                true,
                Stopped::At("open_pr".into()),
                &["prepare:prep", "implement:Implement #1", "check:test"],
            ),
            (
                &[("prepare", &[0])],
                false,
                Stopped::At("failed".into()),
                &["prepare:prep", "implement:Implement #1"],
            ),
            (
                &[("prepare", &[0]), ("check", &[1, 1, 0])],
                true,
                Stopped::At("open_pr".into()),
                &[
                    "prepare:prep",
                    "implement:Implement #1",
                    "check:test",
                    "fix:fix it",
                    "check:test",
                    "fix:fix it",
                    "check:test",
                ],
            ),
            (
                &[("prepare", &[0]), ("check", &[1])],
                true,
                Stopped::Failed {
                    node: "check".into(),
                    reason: "node `fix` exceeded max_visits (2)".into(),
                },
                &[
                    "prepare:prep",
                    "implement:Implement #1",
                    "check:test",
                    "fix:fix it",
                    "check:test",
                    "fix:fix it",
                    "check:test",
                ],
            ),
        ];
        let prompts: BTreeMap<String, String> =
            [("implement".to_string(), "Implement #1".to_string())].into();
        for (scripts, pushed, want, log) in cases {
            let mut f = Fake {
                commands: scripts
                    .iter()
                    .map(|(n, c)| (n.to_string(), c.to_vec()))
                    .collect(),
                pushed: *pushed,
                ..Fake::default()
            };
            let mut st = begin(&def);
            let got = walk(&def, &mut st, &prompts, &mut f).await.unwrap();
            assert_eq!(&got, want);
            assert_eq!(f.log, *log);
            assert_eq!(st.outputs["prepare"]["stdout"], "out");
        }
    }

    #[tokio::test]
    async fn starts_at_a_kernel_node_without_running_anything() {
        let def = enzarb_graph::load(DEF).unwrap();
        let mut st = GraphState {
            node: "done".into(),
            ..Default::default()
        };
        let mut f = Fake::default();
        let got = walk(&def, &mut st, &BTreeMap::new(), &mut f).await.unwrap();
        assert_eq!(got, Stopped::At("done".into()));
        assert!(f.log.is_empty());
    }

    #[test]
    fn helpers() {
        assert_eq!(tail("héllo", 4), "llo");
        assert_eq!(tail("abc", 10), "abc");
        let m =
            final_json("x\n```json\n{\"a\":1}\n```\nmore ```json\n{\"decision\":\"done\"}\n```")
                .unwrap();
        assert_eq!(m["decision"], "done");
        assert!(final_json("no block").is_none());
        assert!(final_json("```json\n[1]\n```").is_none());
        let o = agent_output(
            true,
            "```json\n{\"pushed\": false, \"body\": \"b\"}\n```",
            true,
            "s",
            "br",
        );
        assert_eq!(o["pushed"], true);
        assert_eq!(o["body"], "b");
        let o = command_output(None, "");
        assert_eq!(o["ok"], false);
        let u = add_usage(
            Some(Usage {
                input_tokens: Some(1),
                models: vec!["a".into()],
                ..Usage::default()
            }),
            Some(Usage {
                input_tokens: Some(2),
                cost_usd: Some(0.5),
                models: vec!["a".into(), "b".into()],
                ..Usage::default()
            }),
        )
        .unwrap();
        assert_eq!(u.input_tokens, Some(3));
        assert_eq!(u.output_tokens, None);
        assert_eq!(u.cost_usd, Some(0.5));
        assert_eq!(u.models, ["a", "b"]);
    }
}
