//! Tests of the generic building blocks: decide, vars, templated literals,
//! task-wide handlers, action results, binding params, secrets, workspaces.

use super::*;

const LOOP: &str = r#"version: 1
params:
  rounds: {type: integer, default: 2}
  col: {type: string, default: waiting}
  label: {type: string, default: Go}
  wait_for: {type: string, default: 14d}
start: init
nodes:
  init:
    set: {rounds: 0, name: "subjects.issue.title | lower", greeting: {param: label}}
    next: check
  check:
    decide: {}
    max_visits: "{{ params.rounds + 1 }}"
    next:
      - {if: "vars.rounds >= params.rounds", to: done}
      - {else: hold}
  hold:
    wait:
      events: [comment, fact]
      reason: "round_{{ vars.rounds }}"
      text: "Round {{ vars.rounds }} CI {{ facts.ci.state }}"
      column: {param: col}
      actions: [{label: {param: label}, command: "go{{ vars.rounds }}"}]
      timer: {seconds: {param: wait_for}}
    set_on_exit: {rounds: "vars.rounds + 1", last_cmd: "event.command"}
    next:
      - {if: "event.type == 'comment'", to: check}
  done: {end: {status: done}}
"#;

fn loop_res() -> Resolved {
    let m = base().put(APP, ".enzarb/phases/loop.yaml", LOOP).put(
        APP,
        ".enzarb/workflows/l.yaml",
        "version: 1\nparams: {r: {type: integer, default: 2}}\nphases:\n  - {use: .enzarb/phases/loop, with: {rounds: {param: r}}, max_entries: \"{{ params.r }}\"}\n",
    );
    match resolve(&entry(".enzarb/workflows/l.yaml"), &m, &opts()) {
        Ok((r, _)) => r,
        Err(d) => panic!("{d}"),
    }
}

#[test]
fn decide_vars_templated_literals_and_rerender() {
    let res = loop_res();
    assert_eq!(res.phases[0].max_entries, 2);
    assert_eq!(res.phases[0].nodes["check"].max_visits, 3);
    let mut r = Run::new(res);
    r.on(start());
    assert_eq!(r.at(), "loop/hold");
    assert_eq!(r.s.doc.vars["rounds"], json!(0));
    assert_eq!(r.s.doc.vars["name"], json!("crash on start"));
    assert_eq!(
        r.s.doc.vars["greeting"],
        json!("Go"),
        "string param is a literal"
    );
    assert!(r.fx.iter().any(|e| matches!(
        e,
        Effect::SetTimer {
            seconds: 1_209_600,
            ..
        }
    )));
    let p = r.projection();
    assert_eq!(p.column.as_deref(), Some("waiting"));
    assert_eq!(p.reason.as_deref(), Some("round_0"));
    assert_eq!(p.text.as_deref(), Some("Round 0 CI "));
    assert_eq!(
        p.actions,
        [UiAction {
            label: "Go".into(),
            command: "go0".into()
        }]
    );
    // A fact the text reads changes: re-render without re-entering.
    let path = r.s.path.len();
    r.on(TaskEvent::FactUpdated {
        fact: "ci".into(),
        value: json!({"sha": "a", "state": "success"}),
    });
    assert_eq!((r.at(), r.s.path.len()), ("loop/hold".into(), path));
    assert_eq!(r.projection().text.as_deref(), Some("Round 0 CI success"));
    // A fact that changes nothing visible emits nothing.
    r.on(TaskEvent::FactUpdated {
        fact: "mergeable".into(),
        value: json!(true),
    });
    assert!(r.fx.is_empty(), "{:?}", r.fx);
    r.on(comment("/x", false));
    assert_eq!(r.s.doc.vars["rounds"], json!(1));
    assert_eq!(r.s.doc.vars["last_cmd"], json!("x"));
    assert_eq!(r.projection().text.as_deref(), Some("Round 1 CI success"));
    r.on(comment("/y", false));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
    assert_eq!(
        r.s.path,
        [
            "loop/init",
            "loop/check",
            "loop/hold",
            "loop/check",
            "loop/hold",
            "loop/check",
            "loop/done"
        ]
    );
}

const HANDLED: &str = r#"version: 1
imports: {std: lib/std@v1}
on:
  - {events: [{event: comment, from: writer, if: "event.command == 'cancel'"}], do: cancel}
  - {events: [comment], if: "event.command == 'again'", do: "goto:gate"}
  - {events: [{event: label, if: "event.label == 'halt'"}], do: stop_step}
  - {events: [comment], if: "event.mentioned", set: {mentions: "(vars.mentions or 0) + 1"}}
phases:
  - {use: std/phases/gate}
  - {id: work, use: std/phases/work}
"#;

#[test]
fn task_wide_handlers() {
    let m = base().put(APP, ".enzarb/workflows/h.yaml", HANDLED);
    let (res, _) = resolve(&entry(".enzarb/workflows/h.yaml"), &m, &opts()).unwrap();
    assert_eq!(res.handlers.len(), 4);
    let mut r = Run::new(res);
    r.on(start());
    r.on(comment("hey @enzarb look", false));
    assert_eq!(r.s.doc.vars["mentions"], json!(1));
    assert_eq!(r.at(), "gate/wait");
    // A set-only handler does not consume the event.
    r.on(comment("@enzarb /start", true));
    assert_eq!(
        (r.at(), r.s.doc.vars["mentions"].clone()),
        ("work/implement".into(), json!(2))
    );
    let run = r.run_id();
    r.on(label("halt"));
    assert_eq!(r.kinds(), ["stop_step"]);
    assert_eq!(r.at(), "work/implement");
    // goto: stops the step, then enters the phase.
    r.on(comment("/again", false));
    assert_eq!(r.kinds(), ["stop_step"]);
    assert_eq!((r.at(), r.s.entries["gate"]), ("gate/wait".into(), 2));
    // The stopped step's late result is stale.
    let before = r.s.clone();
    r.on(TaskEvent::StepFinished(StepFinished {
        run_id: run,
        exit: "interrupted".into(),
        ..Default::default()
    }));
    assert_eq!(r.s, before);
    // stop_step without a running step does nothing.
    r.on(label("halt"));
    assert!(r.kinds().is_empty());
    r.on(comment("/start", true));
    r.on(comment("/cancel", false));
    assert_eq!(r.at(), "work/implement", "cancel needs a writer");
    r.on(comment("/cancel", true));
    assert_eq!(r.kinds(), ["stop_step"]);
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: crate::format::CANCELLED.into()
        }
    );
}

const OPEN_PR: &str = r#"version: 1
params: {draft: {type: boolean, default: true}}
start: open
nodes:
  open:
    action:
      name: pull_request.create
      args:
        title: "Fix #{{ subjects.issue.number }}"
        number: "{{ subjects.issue.number }}"
        labels: [a, "{{ vars.x }}"]
        draft: {param: draft}
      as: pr
      subject: true
      relate: {kind: implements, from: "{'repo': result.repo, 'number': result.number}"}
    next:
      - {if: "outputs.pr.number == 12", to: link}
      - {else: done}
  link:
    action:
      name: noop
      subject: {kind: issue, number: "result.n"}
    next: done
  done: {end: {status: done}}
"#;

#[test]
fn action_results_become_outputs_subjects_relations() {
    let m = base().put(APP, ".enzarb/phases/pr.yaml", OPEN_PR).put(
        APP,
        ".enzarb/workflows/p.yaml",
        "version: 1\nphases:\n  - use: .enzarb/phases/pr\n",
    );
    let (res, _) = resolve(&entry(".enzarb/workflows/p.yaml"), &m, &opts()).unwrap();
    let mut r = Run::new(res);
    r.s.doc.task.repo = APP.into();
    r.s.doc.vars.insert("x".into(), json!("b"));
    r.s.doc.subjects.issue = None;
    r.on(start());
    let Some(Effect::CallAction { call_id, args, .. }) =
        r.fx.iter()
            .find(|e| matches!(e, Effect::CallAction { .. }))
            .cloned()
    else {
        panic!()
    };
    assert_eq!(
        Value::Object(args),
        json!({"title": "Fix #", "number": null, "labels": ["a", "b"], "draft": true})
    );
    r.on(TaskEvent::ActionFinished {
        call_id,
        ok: true,
        result: json!({"kind": "pull_request", "number": 12, "repo": "acme/app", "url": "u"}),
    });
    assert_eq!(
        r.kinds(),
        ["link_subject", "create_relations", "call_action"]
    );
    assert!(r.fx.contains(&Effect::LinkSubject {
        kind: crate::events::SubjectKind::PullRequest,
        repo: APP.into(),
        number: 12
    }));
    assert_eq!(r.s.doc.outputs["pr"]["number"], json!(12));
    assert_eq!(r.s.doc.subjects.pull_requests[0].number, 12);
    assert_eq!(
        r.s.doc.relations[0].target,
        json!({"repo": APP, "number": 12})
    );
    let Some(Pending::Action { call_id }) = r.s.pending.clone() else {
        panic!()
    };
    r.on(TaskEvent::ActionFinished {
        call_id,
        ok: true,
        result: json!({"n": "9"}),
    });
    assert!(r.fx.contains(&Effect::LinkSubject {
        kind: crate::events::SubjectKind::Issue,
        repo: APP.into(),
        number: 9
    }));
    assert_eq!(r.s.doc.subjects.issue.as_ref().unwrap().number, 9);
    assert!(matches!(r.s.status, Status::Ended { .. }));
}

#[test]
fn typed_action_args_from_whole_expressions() {
    let ctx = json!({"subjects": {"issue": {"number": 7}}});
    let v = crate::template::render_value(
        &json!({"n": "{{ subjects.issue.number }}", "s": "#{{ subjects.issue.number }}", "l": ["{{ [1, 2] }}"]}),
        &ctx,
    )
    .unwrap();
    assert_eq!(v, json!({"n": 7, "s": "#7", "l": [[1, 2]]}));
}

#[test]
fn binding_with_and_routing_relations() {
    let l = layer(
        "repo",
        APP,
        "bindings:\n  - id: prs\n    on: pull_request\n    if: \"relations | length > 0\"\n    use: .enzarb/workflows/pr\n    with: {parent: \"{{ relations[0].task }}\", n: \"{{ event.number }}\", fixed: 3}\n  - {id: other, on: pull_request, use: .enzarb/workflows/solo}\n",
    );
    let b = merge_bindings(&[l]).unwrap();
    let ev = Event::new(Payload::PullRequest(crate::events::PullRequest {
        number: 5,
        ..Default::default()
    }));
    let mut ctx = crate::document::RoutingContext {
        event: ev.clone(),
        relations: vec![],
    };
    let s = select_binding_in(&b, &ctx).unwrap().unwrap();
    assert_eq!(s.binding.workflow.path, ".enzarb/workflows/solo.yaml");
    ctx.relations.push(crate::document::RoutingRelation {
        kind: "references".into(),
        task: "t-1".into(),
        ..Default::default()
    });
    let s = select_binding_in(&b, &ctx).unwrap().unwrap();
    assert_eq!(s.binding.workflow.path, ".enzarb/workflows/pr.yaml");
    assert_eq!(
        Value::Object(s.params),
        json!({"parent": "t-1", "n": 5, "fixed": 3})
    );
    assert_eq!(
        select_binding(&b, &ev).unwrap().unwrap().workflow.path,
        ".enzarb/workflows/solo.yaml"
    );
}

#[test]
fn workflow_params_bind_and_check() {
    let m = base()
        .put(
            APP,
            ".enzarb/workflows/wp.yaml",
            "version: 1\nparams: {cmd: {type: string}}\nphases:\n  - {use: .enzarb/phases/g, with: {command: {param: cmd}}}\n",
        )
        .put(APP, ".enzarb/phases/g.yaml", GATE);
    let e = entry(".enzarb/workflows/wp.yaml");
    let errs = resolve(&e, &m, &opts()).unwrap_err();
    assert!(
        errs.0
            .iter()
            .any(|d| d.message.contains("missing required param `cmd`"))
    );
    let given = Map::from_iter([("cmd".to_string(), json!("go"))]);
    let (res, _) = resolve_with(&e, &m, &opts(), Some(&given)).unwrap();
    assert_eq!(res.phases[0].params["command"], json!("go"));
    assert_eq!(res.params["cmd"], json!("go"));
    assert!(
        resolve_with(&e, &m, &opts(), None).is_ok(),
        "lint uses placeholders"
    );
}

#[test]
fn secrets_and_workspaces() {
    let step = r#"version: 1
command: {run: "true"}
secrets:
  - TOKEN
  - {name: npm, scope: user, file: .npmrc}
  - {name: KEY, scope: instance}
workspaces:
  - {repo: "{{ task.repo }}-docs", mode: write, branch: "enzarb/{{ task.id }}", base: "{{ task.default_branch }}"}
"#;
    let m = base().put(APP, "steps/s.yaml", step);
    let e = Entry {
        repo: APP.into(),
        rref: "main".into(),
        path: "steps/s.yaml".into(),
    };
    let (s, _) = resolve_step_file(&e, &m, &Options::default()).unwrap();
    assert_eq!(
        s.secrets,
        [
            Secret {
                name: "TOKEN".into(),
                scope: SecretScope::Project,
                env: Some("TOKEN".into()),
                file: None
            },
            Secret {
                name: "npm".into(),
                scope: SecretScope::User,
                env: None,
                file: Some(".npmrc".into())
            },
            Secret {
                name: "KEY".into(),
                scope: SecretScope::Instance,
                env: Some("KEY".into()),
                file: None
            },
        ]
    );
    let phase = "version: 1\nstart: s\nnodes:\n  s: {step: {use: steps/s}, next: d}\n  d: {end: {status: done}}\n";
    let m = m.put(APP, ".enzarb/phases/s.yaml", phase).put(
        APP,
        ".enzarb/workflows/s.yaml",
        "version: 1\nphases:\n  - use: .enzarb/phases/s\n",
    );
    let (res, _) = resolve(&entry(".enzarb/workflows/s.yaml"), &m, &opts()).unwrap();
    let mut r = Run::new(res);
    r.s.doc.task.repo = APP.into();
    r.s.doc.task.id = "t9".into();
    r.s.doc.task.default_branch = "main".into();
    r.on(start());
    let rs = r.run_step();
    assert_eq!(
        rs.workspaces,
        [RunWorkspace {
            repo: "acme/app-docs".into(),
            mode: Access::Write,
            branch: Some("enzarb/t9".into()),
            base: Some("main".into())
        }]
    );
    assert_eq!(rs.secrets.len(), 3);
}

#[test]
fn building_block_lint_errors() {
    let p = ".enzarb/phases/release.yaml";
    let wf = ".enzarb/workflows/issue.yaml";
    let tail = "    next: b\n  b: {end: {status: done}}\n";
    let node = |body: &str| format!("version: 1\nstart: a\nnodes:\n  a:\n{body}{tail}");
    let cases: Vec<(&str, String, &str, &str, usize)> = vec![
        (p, node("    decide: {}\n    max_visits: \"{{ vars.x }}\"\n"), "may only read `params`", p, 6),
        (p, node("    decide: {}\n    max_visits: lots\n"), "must be a non-negative integer", p, 6),
        (p, node("    wait: {events: [release], timer: {seconds: soon}}\n"), "or a duration", p, 5),
        (p, "version: 1\nstart: a\nnodes:\n  a:\n    decide: {}\n    next:\n      - {if: \"vars.x\", to: b}\n  b: {end: {status: done}}\n".into(), "must end with `else`", p, 6),
        (p, node("    set: {bad-name: 1}\n"), "not a variable name", p, 5),
        (p, node("    set: {x: \"nope.y +\"}\n"), "invalid expression", p, 5),
        (p, node("    action: {name: x, relate: {kind: k, targets: \"[]\", from: \"result\"}}\n"), "exactly one of `targets` and `from`", p, 5),
        (p, node("    action: {name: x, args: {n: {param: zz}}}\n"), "no param `zz`", p, 5),
        (p, node("    wait: {events: [release], column: {param: nope}}\n"), "no param `nope`", p, 5),
        (p, node("    step: {inline: {command: {run: x}, secrets: [{name: K, env: \"1BAD\"}]}}\n"), "not an environment variable name", p, 5),
        (p, node("    step: {inline: {command: {run: x}, workspaces: [{repo: a/b, mode: read, branch: x}]}}\n"), "only for write workspaces", p, 5),
        (wf, "version: 1\non:\n  - {events: [comment], do: \"goto:nowhere\"}\nphases:\n  - use: .enzarb/phases/release\n".into(), "not a phase id", wf, 3),
        (wf, "version: 1\non:\n  - {events: [comment]}\nphases:\n  - use: .enzarb/phases/release\n".into(), "needs `do` or `set`", wf, 3),
        (wf, "version: 1\non:\n  - {events: [comment], do: explode}\nphases:\n  - use: .enzarb/phases/release\n".into(), "must be `cancel`", wf, 3),
        (wf, "version: 1\non:\n  - {events: [nonsense], do: cancel}\nphases:\n  - use: .enzarb/phases/release\n".into(), "unknown event type `nonsense`", wf, 3),
        (wf, "version: 1\nphases:\n  - {use: .enzarb/phases/release, max_entries: {param: q}}\n".into(), "no param `q`", wf, 3),
    ];
    for (file, text, want, at_file, line) in &cases {
        let m = base().put(APP, file, text);
        let errs = errors(&m, wf);
        let Some(d) = errs.iter().find(|d| d.message.contains(want)) else {
            panic!(
                "{want:?} not reported; got:\n{}",
                crate::Diagnostics(errs.clone())
            )
        };
        assert_eq!((d.file.as_str(), d.line), (*at_file, *line), "{want}: {d}");
    }
}

#[test]
fn scenario_params_and_vars() {
    let res = loop_res();
    let sc: crate::scenario::Scenario = crate::yaml::SourceFile::parse(
        "s",
        r#"workflow: .enzarb/workflows/l.yaml
params: {r: 2}
document: {vars: {seed: 1}, subjects: {issue: {repo: acme/app, number: 7, title: T}}}
start: {type: issue, action: opened, number: 7}
events:
  - {fact: {fact: ci, value: {sha: a, state: failure}}, expect: {effects: [], projection: {text: "Round 0 CI failure"}}}
  - event: {type: comment, body: /a, command: a}
    expect: {vars: {rounds: 1, last_cmd: a, seed: 1}}
expect: {status: waiting, vars: {rounds: 1}}
"#,
    )
    .unwrap()
    .typed()
    .unwrap();
    let f = crate::scenario::run(&res, &sc);
    assert!(f.is_empty(), "{f:?}");
}

#[test]
fn mentions_parsed() {
    use crate::events::mentions;
    assert!(mentions("hi @Enzarb!", "enzarb"));
    assert!(!mentions("hi @enzarbx", "enzarb"));
    assert!(!mentions("mail a@enzarb", "enzarb"));
    assert!(Comment::parsed("@enzarb /plan", Some("enzarb")).mentioned);
    assert!(!Comment::parsed("/plan", Some("enzarb")).mentioned);
}
