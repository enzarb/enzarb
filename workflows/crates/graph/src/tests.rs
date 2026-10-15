use super::*;
use serde_json::json;

const EXAMPLE: &str = include_str!("../../../../docs/workflows/examples/issue.yaml");

fn def(src: &str) -> Definition {
    Definition::from_yaml(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

/// Wraps `nodes` in a minimal header.
fn with_nodes(start: &str, nodes: &str) -> String {
    format!("version: 1\nname: t\non: [issue_opened]\nstart: {start}\nnodes:\n{nodes}")
}

#[test]
fn example_validates() {
    let d = load(EXAMPLE).unwrap();
    assert_eq!(d.name, "issue");
    assert_eq!(d.on, ["issue_opened"]);
    assert_eq!(d.start, "prepare");
    let implement = d.node("implement").unwrap();
    assert_eq!(implement.max_visits, DEFAULT_MAX_VISITS);
    assert_eq!(
        implement.next,
        Some(Edges::Cond(vec![
            ("pushed".into(), "open_pr".into()),
            ("else".into(), "failed".into())
        ]))
    );
    assert!(d.node("prepare").unwrap().optional);
    assert_eq!(d.node("open_pr").unwrap().kind.runner(), Runner::Kernel);
}

#[test]
fn condition_order_is_file_order() {
    let d = def(&with_nodes(
        "a",
        "  a: {type: agent, prompt: x, next: {z: e, b: e, m: e, else: e}}\n  e: {type: end, status: done}\n",
    ));
    let Some(Edges::Cond(c)) = &d.node("a").unwrap().next else {
        panic!()
    };
    let keys: Vec<&str> = c.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["z", "b", "m", "else"]);
}

#[test]
fn from_value_accepts_string_and_object() {
    let d = Definition::from_value(&Value::String(EXAMPLE.into())).unwrap();
    assert_eq!(d, def(EXAMPLE));
    let v = json!({"version": 1, "name": "t", "on": ["issue_opened"], "start": "e",
        "nodes": {"e": {"type": "end", "status": "done"}}});
    validate(&Definition::from_value(&v).unwrap()).unwrap();
}

#[test]
fn parse_errors() {
    for src in [
        with_nodes("a", "  a: {type: teleport, next: a}\n"),
        with_nodes("a", "  a: {type: end, status: maybe}\n"),
        with_nodes("a", "  a: {type: command, next: e}\n"),
        with_nodes("a", "  a: {type: agent, prompt: x, next: [e]}\n"),
        "version: one".into(),
        "[]".into(),
    ] {
        assert!(
            matches!(Definition::from_yaml(&src), Err(Error::Parse(_))),
            "{src}"
        );
    }
}

/// `(name, nodes, start, expected problem substring)`.
#[test]
fn validation_rules() {
    let end = "  e: {type: end, status: done}\n";
    let cases: &[(&str, String, &str, &str)] = &[
        (
            "missing start",
            end.into(),
            "nope",
            "start node `nope` does not exist",
        ),
        (
            "unknown edge target",
            format!("  a: {{type: command, run: x, next: ghost}}\n{end}"),
            "a",
            "edge to unknown node `ghost`",
        ),
        (
            "unknown cond target",
            format!("  a: {{type: agent, prompt: x, next: {{pushed: ghost, else: e}}}}\n{end}"),
            "a",
            "edge to unknown node `ghost`",
        ),
        (
            "unknown on_limit target",
            format!("  a: {{type: command, run: x, next: e, on_limit: ghost}}\n{end}"),
            "a",
            "edge to unknown node `ghost`",
        ),
        (
            "unknown action",
            format!("  a: {{type: action, action: deploy, next: e}}\n{end}"),
            "a",
            "unknown action `deploy`",
        ),
        (
            "unknown event",
            format!("  a: {{type: wait, events: [comment, pager], next: e}}\n{end}"),
            "a",
            "unknown event `pager`",
        ),
        (
            "empty events",
            format!("  a: {{type: wait, events: [], next: e}}\n{end}"),
            "a",
            "`events` is empty",
        ),
        (
            "missing next",
            format!("  a: {{type: command, run: x}}\n{end}"),
            "a",
            "`next` is missing",
        ),
        (
            "end with next",
            "  e: {type: end, status: done, next: e}\n".into(),
            "e",
            "an end node has no `next`",
        ),
        (
            "no else",
            format!("  a: {{type: agent, prompt: x, next: {{pushed: e}}}}\n{end}"),
            "a",
            "needs an `else`",
        ),
        (
            "else not last",
            format!("  a: {{type: agent, prompt: x, next: {{else: e, pushed: e}}}}\n{end}"),
            "a",
            "`else` must be the last",
        ),
        (
            "bad condition",
            format!("  a: {{type: agent, prompt: x, next: {{\"pushed ==\": e, else: e}}}}\n{end}"),
            "a",
            "condition `pushed ==`",
        ),
        (
            "cannot reach end",
            format!(
                "  a: {{type: command, run: x, next: b}}\n  b: {{type: command, run: y, next: a}}\n{end}"
            ),
            "a",
            "node `a` cannot reach an end node",
        ),
        (
            "zero visits",
            format!("  a: {{type: command, run: x, next: e, max_visits: 0}}\n{end}"),
            "a",
            "`max_visits` must be at least 1",
        ),
        (
            "self on_limit",
            format!("  a: {{type: command, run: x, next: e, on_limit: a}}\n{end}"),
            "a",
            "`on_limit` points at itself",
        ),
        (
            "empty run",
            format!("  a: {{type: command, run: \" \", next: e}}\n{end}"),
            "a",
            "`run` is empty",
        ),
        (
            "empty prompt",
            format!("  a: {{type: agent, prompt: \"\", next: e}}\n{end}"),
            "a",
            "`prompt` is empty",
        ),
        (
            "optional wait",
            format!("  a: {{type: wait, events: [comment], optional: true, next: e}}\n{end}"),
            "a",
            "only command and agent nodes can be optional",
        ),
        (
            "bad id",
            format!("  Bad: {{type: command, run: x, next: e}}\n{end}"),
            "e",
            "node id `Bad`",
        ),
        (
            "merge without wait",
            format!(
                "  a: {{type: agent, prompt: x, next: m}}\n  m: {{type: action, action: merge_change_request, next: e}}\n{end}"
            ),
            "a",
            "reachable only through a wait on `merge_requested`",
        ),
        (
            "merge after wait on comment",
            format!(
                "  w: {{type: wait, events: [comment], next: m}}\n  m: {{type: action, action: merge_change_request, next: e}}\n{end}"
            ),
            "w",
            "reachable only through a wait on `merge_requested`",
        ),
        (
            "merge after wait on merge_requested or comment",
            format!(
                "  w: {{type: wait, events: [merge_requested, comment], next: {{event == \"merge_requested\": m, else: e}}}}\n  m: {{type: action, action: merge_change_request, next: e}}\n{end}"
            ),
            "w",
            "reachable only through a wait on `merge_requested`",
        ),
        (
            "merge via on_limit",
            format!(
                "  w: {{type: wait, events: [merge_requested], next: e, on_limit: m}}\n  m: {{type: action, action: merge_change_request, next: e}}\n{end}"
            ),
            "w",
            "reachable only through a wait on `merge_requested`",
        ),
        (
            "merge as start",
            format!("  m: {{type: action, action: merge_change_request, next: e}}\n{end}"),
            "m",
            "a merge cannot be the start node",
        ),
    ];
    for (name, nodes, start, want) in cases {
        let d = def(&with_nodes(start, nodes));
        match validate(&d) {
            Err(Error::Invalid(p)) => assert!(
                p.iter().any(|x| x.contains(want)),
                "{name}: wanted `{want}` in {p:?}"
            ),
            other => panic!("{name}: expected invalid, got {other:?}"),
        }
    }
}

#[test]
fn header_rules() {
    let src = "version: 2\nname: \"\"\non: []\nstart: e\nnodes:\n  e: {type: end, status: done}\n";
    let Err(Error::Invalid(p)) = validate(&def(src)) else {
        panic!()
    };
    let all = p.join("\n");
    for want in [
        "unsupported version 2",
        "`name` is empty",
        "`on` lists no triggers",
    ] {
        assert!(all.contains(want), "{want} in {all}");
    }
}

#[test]
fn valid_graphs() {
    let end = "  e: {type: end, status: done}\n  f: {type: end, status: failed}\n";
    for nodes in [
        // Gated merge.
        format!(
            "  w: {{type: wait, events: [merge_requested], next: m}}\n  m: {{type: action, action: merge_change_request, next: e}}\n{end}"
        ),
        // A loop with a limit edge.
        format!(
            "  a: {{type: agent, prompt: fix, next: {{\"exit_code == 0\": e, else: a}}, max_visits: 3, on_limit: f}}\n{end}"
        ),
        // Unreachable but well-formed nodes are allowed.
        format!(
            "  a: {{type: command, run: x, next: e}}\n  spare: {{type: command, run: y, next: f}}\n{end}"
        ),
        // Every wait event and the two other actions.
        format!(
            "  a: {{type: wait, events: [comment, approval, ci, review, merge_requested, dependencies_done, timer], next: p}}\n  p: {{type: action, action: post_comment, body: hi, next: o}}\n  o: {{type: action, action: open_change_request, next: e}}\n{end}"
        ),
    ] {
        let start = if nodes.starts_with("  w:") { "w" } else { "a" };
        let d = def(&with_nodes(start, &nodes));
        validate(&d).unwrap_or_else(|e| panic!("{e}\n{nodes}"));
    }
}

fn walk_def() -> Definition {
    load(&with_nodes(
        "prep",
        "  prep: {type: command, run: x, optional: true, next: impl}
  impl: {type: agent, prompt: implement, optional: true, next: {\"pushed && decision == 'done'\": pr, \"exit_code != 0\": fix, else: failed}}
  fix: {type: agent, prompt: fix, next: impl, max_visits: 2, on_limit: failed}
  check: {type: command, run: test, next: pr}
  pr: {type: action, action: open_change_request, next: done}
  failed: {type: end, status: failed}
  done: {type: end, status: done}
",
    ))
    .unwrap()
}

#[test]
fn begin_enters_start() {
    let d = walk_def();
    let (s, n) = GraphState::begin(&d);
    assert_eq!(n, Next::Node("prep".into()));
    assert_eq!(s.node, "prep");
    assert_eq!(s.visits["prep"], 1);
}

/// `(output of the current node, expected next)`, walked in order.
#[test]
fn advance_table() {
    let d = walk_def();
    let (mut s, _) = GraphState::begin(&d);
    let steps: &[(Value, Next)] = &[
        // An optional command that failed still follows its plain edge.
        (json!({"exit_code": 1}), Next::Node("impl".into())),
        (json!({"exit_code": 2}), Next::Node("fix".into())),
        (json!({"ok": true}), Next::Node("impl".into())),
        (json!({"exit_code": 3}), Next::Node("fix".into())),
        (json!({"ok": true}), Next::Node("impl".into())),
        (json!({"exit_code": 3}), Next::Node("failed".into())), // fix's 3rd visit > 2: on_limit
    ];
    for (i, (out, want)) in steps.iter().enumerate() {
        assert_eq!(&advance(&d, &mut s, out.clone()), want, "step {i}");
    }
    assert_eq!(s.visits["fix"], 2);
    assert_eq!(s.visits["impl"], 3);
    assert_eq!(s.outputs["impl"], json!({"exit_code": 3}));
}

#[test]
fn advance_cases() {
    let d = walk_def();
    // (current node, output, expected)
    let cases: &[(&str, Value, Next)] = &[
        (
            "impl",
            json!({"pushed": true, "decision": "done"}),
            Next::Node("pr".into()),
        ),
        (
            "impl",
            json!({"pushed": true, "decision": "nope", "exit_code": 0}),
            Next::Node("failed".into()),
        ),
        // A missing field is null, and `null != 0`.
        ("impl", json!({}), Next::Node("fix".into())),
        ("pr", json!({"number": 7}), Next::Node("done".into())),
        // A non-optional sandbox step that failed on a plain edge fails the graph.
        (
            "check",
            json!({"exit_code": 1}),
            Next::Failed("node `check` failed".into()),
        ),
        ("check", json!({"exit_code": 0}), Next::Node("pr".into())),
        (
            "check",
            json!({"ok": false, "exit_code": 0}),
            Next::Failed("node `check` failed".into()),
        ),
        // Kernel nodes ignore `ok` on a plain edge.
        ("pr", json!({"ok": false}), Next::Node("done".into())),
        (
            "done",
            json!({}),
            Next::Failed("node `done` has no `next`".into()),
        ),
    ];
    for (cur, out, want) in cases {
        let mut s = GraphState {
            node: cur.to_string(),
            ..Default::default()
        };
        assert_eq!(&advance(&d, &mut s, out.clone()), want, "{cur} {out}");
    }
}

#[test]
fn a_failed_non_optional_step_fails_whatever_its_edges() {
    let d = load(&with_nodes(
        "a",
        "  a: {type: command, run: x, next: {\"exit_code == 0\": e, else: f}}\n  e: {type: end, status: done}\n  f: {type: end, status: failed}\n",
    ))
    .unwrap();
    let (mut s, _) = GraphState::begin(&d);
    assert_eq!(
        advance(&d, &mut s, json!({"exit_code": 1})),
        Next::Failed("node `a` failed".into())
    );
    assert_eq!(s.outputs["a"]["exit_code"], 1);
}

#[test]
fn limit_without_on_limit_fails() {
    let d = load(&with_nodes(
        "a",
        "  a: {type: command, run: x, optional: true, next: {\"exit_code == 0\": e, else: a}, max_visits: 2}\n  e: {type: end, status: done}\n",
    ))
    .unwrap();
    let (mut s, _) = GraphState::begin(&d);
    assert_eq!(
        advance(&d, &mut s, json!({"exit_code": 1})),
        Next::Node("a".into())
    );
    assert_eq!(
        advance(&d, &mut s, json!({"exit_code": 1})),
        Next::Failed("node `a` exceeded max_visits (2)".into())
    );
}

#[test]
fn chained_on_limit() {
    let d = load(&with_nodes(
        "a",
        "  a: {type: command, run: x, next: b, max_visits: 1}
  b: {type: command, run: y, next: a, max_visits: 1, on_limit: c}
  c: {type: command, run: z, next: e, max_visits: 1, on_limit: a}
  e: {type: end, status: done}
",
    ))
    .unwrap();
    let mut s = GraphState::default();
    s.visits.insert("b".into(), 1);
    s.visits.insert("c".into(), 1);
    s.visits.insert("a".into(), 1);
    // b -> limit -> c -> limit -> a -> limit, no on_limit: fails, no infinite loop.
    assert!(matches!(enter(&d, &mut s, "b"), Next::Failed(_)));
}

#[test]
fn segment_stops_are_kernel_nodes_behind_sandbox_nodes() {
    let d = load(&with_nodes(
        "a",
        "  a: {type: command, run: x, optional: true, next: {ok: w, else: b}, on_limit: f}
  b: {type: agent, prompt: p, next: p2}
  p2: {type: action, action: post_comment, next: c}
  w: {type: wait, events: [comment], next: c}
  c: {type: command, run: y, next: e}
  e: {type: end, status: done}
  f: {type: end, status: failed}
",
    ))
    .unwrap();
    let names = |s: BTreeSet<String>| s.into_iter().collect::<Vec<_>>();
    assert_eq!(names(segment_stops(&d, "a")), ["f", "p2", "w"]);
    assert_eq!(names(segment_stops(&d, "c")), ["e"]);
    assert_eq!(names(segment_stops(&d, "w")), ["w"]);
}

#[test]
fn wait_from_parses() {
    let d = def(&with_nodes(
        "w",
        "  w: {type: wait, events: [comment], from: anyone, next: e}\n  e: {type: end, status: done}\n",
    ));
    assert_eq!(
        d.node("w").unwrap().kind,
        NodeKind::Wait {
            events: vec!["comment".into()],
            from: WaitFrom::Anyone
        }
    );
    let d = def(&with_nodes(
        "w",
        "  w: {type: wait, events: [comment], next: e}\n  e: {type: end, status: done}\n",
    ));
    assert!(matches!(
        d.node("w").unwrap().kind,
        NodeKind::Wait {
            from: WaitFrom::Writers,
            ..
        }
    ));
    let bad = with_nodes(
        "w",
        "  w: {type: wait, events: [comment], from: everyone, next: e}\n  e: {type: end, status: done}\n",
    );
    assert!(matches!(Definition::from_yaml(&bad), Err(Error::Parse(_))));
}

#[test]
fn output_ok_rules() {
    for (v, want) in [
        (json!({}), true),
        (json!({"exit_code": 0}), true),
        (json!({"exit_code": 1}), false),
        (json!({"ok": false}), false),
        (json!({"ok": true, "exit_code": 1}), true),
        (json!(null), true),
    ] {
        assert_eq!(output_ok(&v), want, "{v}");
    }
}

const ENZARB_ISSUE: &str = include_str!("../../../../.enzarb/workflows/issue.yaml");

/// enzarb's own definition: feedback re-plans, only an approval implements.
#[test]
fn enzarb_issue_workflow_gates_implementation_on_approval() {
    let d = load(ENZARB_ISSUE).unwrap();
    let (mut s, n) = GraphState::begin(&d);
    assert_eq!(n, Next::Node("plan".into()));
    assert_eq!(
        advance(&d, &mut s, json!({"ok": true})),
        Next::Node("post_plan".into())
    );
    assert_eq!(
        advance(&d, &mut s, json!({})),
        Next::Node("approval".into())
    );
    let feedback = json!({"event": "comment", "author": "a", "body": "x", "can_write": true});
    assert_eq!(advance(&d, &mut s, feedback), Next::Node("plan".into()));
    advance(&d, &mut s, json!({"ok": true}));
    advance(&d, &mut s, json!({}));
    let approved = json!({"event": "approval", "author": "a"});
    assert_eq!(advance(&d, &mut s, approved), Next::Node("prepare".into()));
    // A failing optional prepare still implements.
    assert_eq!(
        advance(&d, &mut s, json!({"exit_code": 1})),
        Next::Node("implement".into())
    );
    assert_eq!(
        advance(&d, &mut s, json!({"ok": true, "pushed": true})),
        Next::Node("open_pr".into())
    );
}
const EPIC_EXAMPLE: &str = include_str!("../../../../docs/workflows/examples/epic-issue.yaml");

#[test]
fn epic_example_validates_and_walks() {
    let d = load(EPIC_EXAMPLE).unwrap();
    let walk = |item: Value, steps: &[Value]| {
        let (mut s, _) = GraphState::begin(&d);
        s.outputs.insert(ITEM.into(), item);
        let mut path = vec![s.node.clone()];
        for out in steps {
            match advance(&d, &mut s, out.clone()) {
                Next::Node(n) => path.push(n),
                Next::Failed(r) => path.push(format!("failed: {r}")),
            }
        }
        path
    };
    let yes = json!({"epic": false, "epic_allowed": true});
    // Triage says epic; the item becomes an epic (the kernel sets the fact).
    let epic = json!({"epic": true, "epic_allowed": true});
    assert_eq!(
        walk(yes.clone(), &[json!({"decision": "epic"})]),
        ["triage", "plan_epic"]
    );
    let approved = json!({"event": "approval"});
    assert_eq!(
        walk(
            epic.clone(),
            &[
                json!({"decision": "ready"}),
                json!({"decision": "plan"}),
                json!({"id": 1}),
                approved.clone(),
                json!({"created": []}),
                json!({"event": "children_done", "failed": []}),
            ]
        ),
        [
            "triage",
            "plan_epic",
            "post_plan",
            "approval",
            "create",
            "children",
            "done"
        ]
    );
    // A closed sub-issue: tell people, wait for a human or for it to be replaced.
    assert_eq!(
        walk(
            epic.clone(),
            &[
                json!({}),
                json!({}),
                json!({}),
                approved.clone(),
                json!({}),
                json!({"event": "children_done", "failed": ["#3"]}),
                json!({}),
                json!({"event": "children_done", "failed": ["#3"]}),
                approved.clone(),
            ]
        ),
        [
            "triage",
            "plan_epic",
            "post_plan",
            "approval",
            "create",
            "children",
            "tell_stuck",
            "stuck",
            "stuck",
            "done"
        ]
    );
    // Not an epic, or not allowed at this depth: the ordinary path.
    let no = json!({"epic": false, "epic_allowed": false});
    assert_eq!(
        walk(
            no,
            &[json!({"decision": "epic"}), json!({}), json!({}), approved]
        ),
        ["triage", "plan", "post_plan", "approval", "implement"]
    );
}

#[test]
fn create_sub_issues_needs_an_approval_wait() {
    let end = "  e: {type: end, status: done}\n";
    let bad = [
        format!(
            "  a: {{type: agent, prompt: x, next: c}}\n  c: {{type: action, action: create_sub_issues, next: e}}\n{end}"
        ),
        format!(
            "  a: {{type: wait, events: [comment], next: c}}\n  c: {{type: action, action: create_sub_issues, next: e}}\n{end}"
        ),
        format!(
            "  a: {{type: wait, events: [approval], next: e, on_limit: c}}\n  c: {{type: action, action: create_sub_issues, next: e}}\n{end}"
        ),
    ];
    for nodes in bad {
        let Err(Error::Invalid(p)) = validate(&def(&with_nodes("a", &nodes))) else {
            panic!("{nodes}")
        };
        assert!(
            p.iter()
                .any(|x| x.contains("reachable only through a wait on `approval`")),
            "{p:?}"
        );
    }
    let ok = format!(
        "  a: {{type: wait, events: [approval, comment], next: c}}\n  c: {{type: action, action: create_sub_issues, next: e}}\n{end}"
    );
    validate(&def(&with_nodes("a", &ok))).unwrap();
    let Err(Error::Invalid(p)) =
        validate(&def(&with_nodes("_x", "  _x: {type: end, status: done}\n")))
    else {
        panic!()
    };
    assert!(p.iter().any(|x| x.contains("not start with `_`")));
}

#[test]
fn item_facts_fill_missing_identifiers_only() {
    let d = load(&with_nodes(
        "a",
        "  a: {type: command, run: x, next: {epic: e, else: f}}\n  e: {type: end, status: done}\n  f: {type: end, status: failed}\n",
    ))
    .unwrap();
    let run = |item: Value, out: Value| {
        let (mut s, _) = GraphState::begin(&d);
        s.outputs.insert(ITEM.into(), item);
        advance(&d, &mut s, out)
    };
    assert_eq!(
        run(json!({"epic": true}), json!({})),
        Next::Node("e".into())
    );
    assert_eq!(
        run(json!({"epic": true}), json!({"epic": false})),
        Next::Node("f".into())
    );
    assert_eq!(run(json!({}), json!({})), Next::Node("f".into()));
}
