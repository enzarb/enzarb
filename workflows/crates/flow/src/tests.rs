//! Cross-module tests: resolution, layering, params, exits, templates and
//! interpreter paths, against an in-memory loader.

use crate::document::{GitFacts, Subject, TaskDocument};
use crate::events::{Actor, Comment, Event, Label, Payload, Release};
use crate::format::*;
use crate::interp::*;
use crate::resolve::*;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

mod blocks;
mod commands;

const LIB: &str = "lib/std";
const APP: &str = "acme/app";

#[derive(Default, Clone)]
struct Mem(BTreeMap<(String, String), String>);

impl Mem {
    fn put(mut self, repo: &str, path: &str, text: &str) -> Self {
        self.0.insert((repo.into(), path.into()), text.into());
        self
    }
}

impl Loader for Mem {
    fn load(&self, repo: &str, rref: &str, path: &str) -> Result<Loaded, String> {
        self.0
            .get(&(repo.to_string(), path.to_string()))
            .map(|t| Loaded {
                sha: format!("{repo}@{rref}"),
                bytes: t.clone().into_bytes(),
            })
            .ok_or_else(|| "not found".into())
    }
}

const GATE: &str = r#"version: 1
params:
  command: {type: string, default: start}
exits: [rejected]
start: wait
nodes:
  wait:
    wait:
      events:
        - {event: comment, from: writer, if: "event.command == params.command"}
        - label
      reason: awaiting_start
      text: "Waiting for /{{ params.command }}"
      column: waiting
      actions: [{label: Start, command: start}]
    next:
      - {if: "event.type == 'label' and event.label == 'reject'", to: rejected}
      - {if: "event.type == 'comment' or event.label == 'go'", to: done}
  done: {end: {status: done}}
  rejected: {end: {status: rejected}}
"#;

const WORK: &str = r#"version: 1
params:
  prompt: {type: prompt, default: {file: prompts/work.md}}
  max_rounds: {type: integer, default: 3}
exits: [failed]
start: implement
nodes:
  implement:
    step: {use: steps/agent, with: {prompt: {param: prompt}}}
    max_visits: 3
    on_limit: give_up
    next:
      - {if: "last.exit == 'success' and facts.git.pushed", to: check}
      - {if: "last.exit == 'interrupted'", to: stopped}
      - {else: implement}
  check:
    step:
      inline:
        command: {run: "mise run check --rounds {{ params.rounds }}"}
        params: {rounds: {type: integer}}
      with: {rounds: {param: max_rounds}}
    next: [{if: "exit == 'success'", to: done}, {else: implement}]
  give_up: {end: {status: failed}}
  stopped: {end: {status: failed}}
  done: {end: {status: done}}
"#;

const AGENT: &str = r#"version: 1
params:
  prompt: {type: prompt}
agent: {runtime: runtimes/claude, model: opus, prompt: {param: prompt}}
inputs:
  plan: {from: outputs.plan, type: string, required: false}
outputs:
  summary: {file: .enzarb/out/summary.md, artifact: Summary, optional: true}
result: {schema: {type: object}, as: work}
github: {contents: write}
secrets: [API_KEY]
steer: [comment]
"#;

const RUNTIME: &str = r#"version: 1
install: {mise: ["npm:@agentclientprotocol/claude-agent-acp@1"]}
launch: {protocol: acp, command: [claude-agent-acp]}
capabilities: {sessions: true, resume: true, steering: true, structured_output: true}
secrets: [API_KEY]
"#;

const BOARD: &str = r#"version: 1
columns:
  - {id: waiting, title: Waiting}
  - {id: doing, title: Doing}
  - {id: done, title: Done}
  - {id: stuck, title: Stuck}
defaults: {running: doing, waiting_human: waiting, waiting_external: waiting, paused: stuck, ended: done}
"#;

const RELEASE: &str = r#"version: 1
start: wait_release
nodes:
  wait_release:
    wait: {events: [release, timer], reason: awaiting_release, timer: {seconds: 60}, refresh: [ci]}
    next:
      - {if: "event.type == 'timer'", to: announce}
      - {if: "event.action == 'published'", to: announce}
  announce:
    action:
      name: comment.create
      args: {body: "Released {{ event.tag }} for #{{ subjects.issue.number }}"}
      relate: {kind: announces, targets: "[1, 2]"}
    next: done
  done: {end: {status: done}}
"#;

const ISSUE: &str = r#"version: 1
imports: {std: lib/std@v1}
board: std/board
phases:
  - {use: std/phases/gate}
  - {id: work, use: std/phases/work, with: {max_rounds: 2}, exits: {failed: gate}, max_entries: 2}
  - {use: .enzarb/phases/release}
"#;

fn base() -> Mem {
    Mem::default()
        .put(LIB, "phases/gate.yaml", GATE)
        .put(LIB, "phases/work.yaml", WORK)
        .put(LIB, "steps/agent.yaml", AGENT)
        .put(
            LIB,
            "prompts/work.md",
            "Fix {{ subjects.issue.title }}\n{{ plan }}\n",
        )
        .put(LIB, "runtimes/claude.yaml", RUNTIME)
        .put(LIB, "board.yaml", BOARD)
        .put(APP, ".enzarb/phases/release.yaml", RELEASE)
        .put(APP, ".enzarb/workflows/issue.yaml", ISSUE)
}

fn opts() -> Options {
    Options {
        allow: vec![LIB.into()],
    }
}

fn entry(path: &str) -> Entry {
    Entry {
        repo: APP.into(),
        rref: "main".into(),
        path: path.into(),
    }
}

fn resolved(m: &Mem) -> Resolved {
    match resolve(&entry(".enzarb/workflows/issue.yaml"), m, &opts()) {
        Ok((r, _)) => r,
        Err(d) => panic!("{d}"),
    }
}

fn errors(m: &Mem, path: &str) -> Vec<crate::Diagnostic> {
    match resolve(&entry(path), m, &opts()) {
        Ok((_, w)) => w.into_iter().filter(|d| d.is_error()).collect(),
        Err(d) => d.0,
    }
}

// ---------------------------------------------------------------- resolution

#[test]
fn resolves_imports_params_exits() {
    let r = resolved(&base());
    let ids: Vec<_> = r.phases.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["gate", "work", "release"]);
    let gate = &r.phases[0];
    assert_eq!(gate.params["command"], json!("start"));
    assert_eq!(gate.origin.repo, LIB);
    assert_eq!(gate.origin.sha, "lib/std@v1");
    assert_eq!(gate.routes["done"], Route::Phase("work".into()));
    assert_eq!(gate.routes["rejected"], Route::End("rejected".into()));
    assert_eq!(gate.routes[EXIT_LIMIT], Route::End(EXIT_LIMIT.into()));
    let work = &r.phases[1];
    assert_eq!(work.params["max_rounds"], json!(2));
    assert_eq!(
        work.params["prompt"],
        json!("Fix {{ subjects.issue.title }}\n{{ plan }}\n")
    );
    assert_eq!(work.routes["failed"], Route::Phase("gate".into()));
    assert_eq!(work.max_entries, 2);
    assert_eq!(r.phases[2].routes["done"], Route::End("done".into()));
    let RKind::Step(st) = &work.nodes["check"].kind else {
        panic!()
    };
    assert_eq!(st.params["rounds"], json!(2));
    let RKind::Step(ag) = &work.nodes["implement"].kind else {
        panic!()
    };
    let Exec::Agent {
        runtime,
        model,
        prompt,
    } = &ag.exec
    else {
        panic!()
    };
    assert_eq!(
        (runtime.file.launch.protocol, model.as_deref()),
        (Protocol::Acp, Some("opus"))
    );
    assert!(prompt.starts_with("Fix "));
    assert_eq!(ag.stop, default_stop());
    assert_eq!(r.board.columns.len(), 4);
    assert!(r.origins.iter().any(|o| o.path == "prompts/work.md"));
}

/// `(file to replace or add, contents, expected message fragment, file, line)`.
type Case = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    usize,
);

#[test]
fn resolution_errors_have_locations() {
    let wf = ".enzarb/workflows/issue.yaml";
    let cases: &[Case] = &[
        (
            wf,
            "version: 1\nimports: {std: lib/std@v1}\nphases:\n  - {use: std/phases/gate, with: {nope: 1}}\n",
            "unknown param `nope`",
            wf,
            4,
        ),
        (
            wf,
            "version: 1\nimports: {std: lib/std@v1}\nphases:\n  - {use: std/phases/gate, with: {command: 3}}\n",
            "param `command` must be a string",
            wf,
            4,
        ),
        (
            wf,
            "version: 1\nimports: {std: lib/std@v1}\nphases:\n  - use: std/phases/gate\n    exits: {nope: next}\n",
            "has no exit `nope`",
            wf,
            5,
        ),
        (
            wf,
            "version: 1\nimports: {std: lib/std@v1}\nphases:\n  - use: std/phases/gate\n    exits: {rejected: nowhere}\n",
            "not a phase id",
            wf,
            5,
        ),
        (
            wf,
            "version: 1\nimports: {std: lib/std@v1}\nphases:\n  - use: std/phases/gate\n  - use: std/phases/gate\n",
            "duplicate phase id `gate`",
            wf,
            5,
        ),
        (
            wf,
            "version: 1\nimports: {std: lib/std@v1}\nphases:\n  - use: std/phases/gate\n    on_limit: nope\n",
            "on_limit `nope`",
            wf,
            5,
        ),
        (
            wf,
            "version: 1\nimports: {evil: evil/repo@v1}\nphases:\n  - use: evil/phases/x\n",
            "not an allowed definition repository",
            wf,
            2,
        ),
        (
            wf,
            "version: 1\nimports: {std: lib/std}\nphases:\n  - use: x\n",
            "must be `owner/repo@ref`",
            wf,
            2,
        ),
        (
            wf,
            "version: 2\nphases:\n  - use: .enzarb/phases/release\n",
            "unsupported version 2",
            wf,
            1,
        ),
        (
            wf,
            "version: 1\nphases:\n  - use: .enzarb/phases/release\n    surprise: 1\n",
            "unknown field `surprise`",
            wf,
            4,
        ),
        (
            wf,
            "version: 1\nphases:\n  - use: .enzarb/phases/missing\n",
            "cannot load `.enzarb/phases/missing.yaml`",
            wf,
            3,
        ),
        (wf, "version: 1\nphases: []\n", "at least one phase", wf, 2),
        (
            wf,
            "version: 1\nevents: {comment: {description: x}}\nphases:\n  - use: .enzarb/phases/release\n",
            "shadows a built-in",
            wf,
            2,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {wait: {events: [release]}, next: b}\n",
            "unknown node `b`",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {end: {status: weird}}\n",
            "`weird` is not an exit",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: x\nnodes:\n  a: {end: {status: done}}\n",
            "start node `x` does not exist",
            ".enzarb/phases/release.yaml",
            2,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {wait: {events: [release]}, end: {status: done}}\n",
            "exactly one of",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a:\n    action: {name: x}\n    next:\n      - {if: \"true\", to: b}\n  b: {end: {status: done}}\n",
            "must end with `else`",
            ".enzarb/phases/release.yaml",
            6,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a:\n    action: {name: x}\n    next:\n      - {else: b}\n      - {if: \"true\", to: b}\n  b: {end: {status: done}}\n",
            "`else` must be the last",
            ".enzarb/phases/release.yaml",
            7,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a:\n    action: {name: x}\n    next:\n      - {if: \"outputs.x ==\", to: b}\n      - {else: b}\n  b: {end: {status: done}}\n",
            "invalid expression",
            ".enzarb/phases/release.yaml",
            7,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a:\n    action: {name: x}\n    next:\n      - {if: \"params.nope\", to: b}\n      - {else: b}\n  b: {end: {status: done}}\n",
            "undefined param `nope`",
            ".enzarb/phases/release.yaml",
            7,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {wait: {events: [deployed]}, next: b}\n  b: {end: {status: done}}\n",
            "unknown event type `deployed`",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {wait: {events: [release], refresh: [vibes]}, next: b}\n  b: {end: {status: done}}\n",
            "unknown fact `vibes`",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a:\n    wait: {events: [release], text: \"{{ nonsense }}\"}\n    next: b\n  b: {end: {status: done}}\n",
            "undefined variable `nonsense`",
            ".enzarb/phases/release.yaml",
            5,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {phase: {use: .enzarb/phases/release}, next: a}\n",
            "includes itself",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a: {step: {inline: {command: {run: x}, agent: {runtime: r, prompt: p}}}, next: b}\n  b: {end: {status: done}}\n",
            "exactly one of `agent` and `command`",
            ".enzarb/phases/release.yaml",
            4,
        ),
        (
            ".enzarb/phases/release.yaml",
            "version: 1\nstart: a\nnodes:\n  a:\n    step: {inline: {command: {run: \"echo {{ plna }}\"}}}\n    next: b\n  b: {end: {status: done}}\n",
            "undefined variable `plna`",
            ".enzarb/phases/release.yaml",
            5,
        ),
        (
            "prompts/work.md",
            "line one\n{{ subjects.issue.title }}\n{{ outputs.x }} {{ mystery }}\n",
            "undefined variable `mystery`",
            "lib/std@v1:prompts/work.md",
            3,
        ),
        (
            "prompts/work.md",
            "a\n{% if %}\n",
            "invalid template",
            "lib/std@v1:prompts/work.md",
            2,
        ),
    ];
    for (file, text, want, at_file, line) in cases {
        let repo = if file.starts_with("prompts/") {
            LIB
        } else {
            APP
        };
        let m = base().put(repo, file, text);
        let errs = errors(&m, wf);
        let hit = errs.iter().find(|d| d.message.contains(want));
        let Some(d) = hit else {
            panic!(
                "{want:?} not reported; got:\n{}",
                crate::Diagnostics(errs.clone())
            )
        };
        assert_eq!((d.file.as_str(), d.line), (*at_file, *line), "{want}: {d}");
    }
}

#[test]
fn board_column_checked() {
    let m = base().put(
        LIB,
        "phases/gate.yaml",
        &GATE.replace("column: waiting", "column: nowhere"),
    );
    let e = errors(&m, ".enzarb/workflows/issue.yaml");
    assert!(
        e.iter()
            .any(|d| d.message.contains("unknown column `nowhere`")
                && d.file == "lib/std@v1:phases/gate.yaml"),
        "{e:?}"
    );
}

#[test]
fn param_kinds() {
    let phase = r#"version: 1
params:
  mode: {type: enum, values: [auto, manual]}
  tags: {type: list, items: integer, default: []}
  on: {type: boolean, default: false}
  run: {type: command, default: "true"}
start: a
nodes:
  a: {end: {status: done}}
"#;
    let cases: &[(&str, Option<&str>)] = &[
        ("{mode: auto}", None),
        ("{mode: auto, tags: [1, 2], on: true}", None),
        ("{mode: other}", Some("must be one of auto, manual")),
        ("{}", Some("missing required param `mode`")),
        (
            "{mode: auto, tags: [a]}",
            Some("param `tags[0]` must be an integer"),
        ),
        ("{mode: auto, tags: 3}", Some("must be a list")),
        ("{mode: auto, on: yes}", Some("must be a boolean")),
        (
            "{mode: {param: x}}",
            Some("the enclosing file has no param `x`"),
        ),
    ];
    for (with, want) in cases {
        let wf = format!("version: 1\nphases:\n  - use: .enzarb/phases/p\n    with: {with}\n");
        let m = base().put(APP, ".enzarb/phases/p.yaml", phase).put(
            APP,
            ".enzarb/workflows/w.yaml",
            &wf,
        );
        let e = errors(&m, ".enzarb/workflows/w.yaml");
        match want {
            None => assert!(e.is_empty(), "{with}: {e:?}"),
            Some(w) => assert!(
                e.iter().any(|d| d.message.contains(w)),
                "{with}: want {w:?}, got {e:?}"
            ),
        }
    }
}

#[test]
fn standalone_phase_and_step_files() {
    let m = base();
    let e = Entry {
        repo: LIB.into(),
        rref: "v1".into(),
        path: "phases/work.yaml".into(),
    };
    let (p, _) = resolve_phase_file(&e, &m, &Options::default()).unwrap();
    assert_eq!(p.params["max_rounds"], json!(3));
    let e = Entry {
        repo: LIB.into(),
        rref: "v1".into(),
        path: "steps/agent.yaml".into(),
    };
    let (s, _) = resolve_step_file(&e, &m, &Options::default()).unwrap();
    assert_eq!(s.secrets, [SecretDecl::from("API_KEY").normalized()]);
    // A phase alone may wait on events only a workflow declares: a warning.
    let m = m.put(LIB, "phases/c.yaml", "version: 1\nstart: a\nnodes:\n  a: {wait: {events: [deployed]}, next: b}\n  b: {end: {status: done}}\n  z: {end: {status: done}}\n");
    let e = Entry {
        repo: LIB.into(),
        rref: "v1".into(),
        path: "phases/c.yaml".into(),
    };
    let (_, warnings) = resolve_phase_file(&e, &m, &Options::default()).unwrap();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings
            .iter()
            .any(|w| w.message.contains("unreachable") && w.line == 6)
    );
}

#[test]
fn custom_events_declared() {
    let wf = "version: 1\nevents:\n  deployed: {description: x, schema: {type: object}}\nphases:\n  - use: .enzarb/phases/c\n";
    let m = base().put(APP, ".enzarb/workflows/w.yaml", wf).put(APP, ".enzarb/phases/c.yaml", "version: 1\nstart: a\nnodes:\n  a: {wait: {events: [deployed]}, next: b}\n  b: {end: {status: done}}\n");
    assert!(errors(&m, ".enzarb/workflows/w.yaml").is_empty());
}

// ---------------------------------------------------------------- layering

fn layer(name: &str, repo: &str, yaml: &str) -> Layer {
    let f: BindingsFile = crate::yaml::SourceFile::parse(name, yaml)
        .unwrap()
        .typed()
        .unwrap();
    Layer {
        name: name.into(),
        repo: repo.into(),
        rref: "main".into(),
        file: f,
    }
}

#[test]
fn bindings_layering() {
    let lib = layer(
        "library",
        LIB,
        "bindings:\n  - {id: issues, on: issue, if: \"event.action == 'opened'\", use: workflows/issue}\n  - {id: prs, on: pull_request, use: workflows/pr}\n  - {id: nightly, on: {schedule: '0 3 * * *'}, use: workflows/retro}\n",
    );
    let project = layer(
        "project",
        APP,
        "imports: {std: lib/std@v1}\nbindings:\n  - {id: prs, disabled: true}\n",
    );
    let repo = layer(
        "repo",
        APP,
        "imports: {std: lib/std@v1}\nbindings:\n  - {id: issues, on: issue, if: \"event.action == 'opened'\", use: .enzarb/workflows/issue}\n  - {on: comment, use: std/workflows/mention}\n",
    );
    let b = merge_bindings(&[lib, project, repo]).unwrap();
    let got: Vec<_> = b
        .iter()
        .map(|b| {
            (
                b.layer.as_str(),
                b.workflow.repo.as_str(),
                b.workflow.path.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            ("repo", APP, ".enzarb/workflows/issue.yaml"),
            ("repo", LIB, "workflows/mention.yaml"),
            ("library", LIB, "workflows/retro.yaml")
        ]
    );
    let ev = |p: Payload| Event::new(p);
    let opened = ev(Payload::Issue(crate::events::Issue {
        action: "opened".into(),
        number: 1,
        ..Default::default()
    }));
    let closed = ev(Payload::Issue(crate::events::Issue {
        action: "closed".into(),
        number: 1,
        ..Default::default()
    }));
    let cron = ev(Payload::Schedule(crate::events::Schedule {
        cron: "0 3 * * *".into(),
        binding: None,
    }));
    let pr = ev(Payload::PullRequest(Default::default()));
    let pick = |e: &Event| {
        select_binding(&b, e)
            .unwrap()
            .map(|b| b.workflow.path.clone())
    };
    assert_eq!(
        pick(&opened).as_deref(),
        Some(".enzarb/workflows/issue.yaml")
    );
    assert_eq!(pick(&closed), None);
    assert_eq!(pick(&cron).as_deref(), Some("workflows/retro.yaml"));
    assert_eq!(pick(&pr), None, "disabled in the project layer");
}

#[test]
fn board_layering() {
    let lib: BoardFile = crate::yaml::SourceFile::parse("b", BOARD)
        .unwrap()
        .typed()
        .unwrap();
    let over: BoardFile = crate::yaml::SourceFile::parse("o", "columns:\n  - {id: doing, title: In progress, limit: 3}\n  - {id: review, title: Review}\ndefaults: {paused: waiting}\n").unwrap().typed().unwrap();
    let b = lib.layer(&over);
    let cols: Vec<_> = b
        .columns
        .iter()
        .map(|c| (c.id.as_str(), c.title.as_str()))
        .collect();
    assert_eq!(
        cols,
        [
            ("waiting", "Waiting"),
            ("doing", "In progress"),
            ("done", "Done"),
            ("stuck", "Stuck"),
            ("review", "Review")
        ]
    );
    assert_eq!(b.defaults.paused.as_deref(), Some("waiting"));
    assert_eq!(b.defaults.running.as_deref(), Some("doing"));
}

// ---------------------------------------------------------------- interpreter

fn doc() -> TaskDocument {
    TaskDocument {
        subjects: crate::document::Subjects {
            issue: Some(Subject {
                repo: APP.into(),
                number: 7,
                title: "Crash on start".into(),
                ..Default::default()
            }),
            pull_requests: vec![],
        },
        ..Default::default()
    }
}

fn comment(body: &str, writer: bool) -> TaskEvent {
    let mut e = Event::new(Payload::Comment(Comment::parsed(body, Some("enzarb"))));
    e.actor = Some(Actor {
        login: "u".into(),
        can_write: writer,
        bot: false,
    });
    TaskEvent::Event { event: e }
}

fn label(l: &str) -> TaskEvent {
    TaskEvent::Event {
        event: Event::new(Payload::Label(Label {
            action: "added".into(),
            label: l.into(),
        })),
    }
}

fn start() -> TaskEvent {
    TaskEvent::Start {
        event: Event::new(Payload::Issue(crate::events::Issue {
            action: "opened".into(),
            number: 7,
            ..Default::default()
        })),
    }
}

struct Run {
    res: Resolved,
    s: TaskState,
    fx: Vec<Effect>,
}

impl Run {
    fn new(res: Resolved) -> Self {
        Self {
            res,
            s: TaskState::new(doc()),
            fx: vec![],
        }
    }
    fn on(&mut self, e: TaskEvent) -> &mut Self {
        let (s, fx) = step(&self.res, &self.s, &e);
        self.s = s;
        self.fx = fx;
        self
    }
    fn at(&self) -> String {
        self.s.position().unwrap_or_default()
    }
    fn kinds(&self) -> Vec<String> {
        self.fx
            .iter()
            .map(|e| {
                serde_json::to_value(e).unwrap()["effect"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .filter(|k| k != "update_projection")
            .collect()
    }
    fn projection(&self) -> Projection {
        self.fx
            .iter()
            .rev()
            .find_map(|e| {
                if let Effect::UpdateProjection(p) = e {
                    Some((**p).clone())
                } else {
                    None
                }
            })
            .expect("projection")
    }
    fn run_step(&self) -> RunStep {
        self.fx
            .iter()
            .find_map(|e| {
                if let Effect::RunStep(r) = e {
                    Some((**r).clone())
                } else {
                    None
                }
            })
            .expect("run_step")
    }
    fn run_id(&self) -> String {
        match &self.s.pending {
            Some(Pending::Step { run_id, .. }) => run_id.clone(),
            p => panic!("no running step: {p:?}"),
        }
    }
    fn finish(&mut self, exit: &str, pushed: bool) -> &mut Self {
        let run_id = self.run_id();
        let git = Some(GitFacts {
            pushed,
            head: "abc".into(),
            ..Default::default()
        });
        self.on(TaskEvent::StepFinished(StepFinished {
            run_id,
            exit: exit.into(),
            git,
            ..Default::default()
        }))
    }
}

#[test]
fn happy_path() {
    let mut r = Run::new(resolved(&base()));
    r.on(start());
    assert_eq!(
        (r.s.status.clone(), r.at()),
        (Status::Waiting, "gate/wait".into())
    );
    let p = r.projection();
    assert_eq!(
        (
            p.column.as_deref(),
            p.reason.as_deref(),
            p.text.as_deref(),
            p.needs_you
        ),
        (
            Some("waiting"),
            Some("awaiting_start"),
            Some("Waiting for /start"),
            true
        )
    );
    assert_eq!(p.actions[0].command, "start");

    // A non-writer's command and an unrelated comment are ignored.
    r.on(comment("/start", false));
    assert_eq!(
        (r.at(), r.kinds()),
        ("gate/wait".into(), vec!["reply".to_string()])
    );
    r.on(comment("/plan", true));
    assert_eq!(r.at(), "gate/wait");

    r.on(comment("@enzarb /start", true));
    assert_eq!(r.at(), "work/implement");
    let rs = r.run_step();
    let RunExec::Agent { prompt, model, .. } = &rs.exec else {
        panic!()
    };
    assert_eq!(prompt, "Fix Crash on start\n\n");
    assert_eq!(model.as_deref(), Some("opus"));
    assert_eq!(
        (rs.secrets.clone(), rs.github["contents"]),
        (
            vec![SecretDecl::from("API_KEY").normalized()],
            Access::Write
        )
    );
    assert_eq!(r.projection().column.as_deref(), Some("doing"));
    assert_eq!(r.s.doc.event.as_ref().unwrap().type_name(), "comment");

    let run_id = r.run_id();
    r.on(TaskEvent::StepFinished(StepFinished {
        run_id,
        exit: "success".into(),
        outputs: Map::from_iter([("summary".to_string(), json!("did it"))]),
        result: Some(json!({"files": 2})),
        git: Some(GitFacts {
            pushed: true,
            ..Default::default()
        }),
        environment: None,
    }));
    assert_eq!(r.at(), "work/check");
    let RunExec::Command { run, .. } = &r.run_step().exec else {
        panic!()
    };
    assert_eq!(run, "mise run check --rounds 2");
    assert_eq!(r.s.doc.outputs["work"], json!({"files": 2}));
    assert_eq!(r.s.doc.artifacts[0].title, "Summary");
    assert_eq!(r.s.doc.artifacts[0].format, "markdown");

    r.finish("success", true);
    assert_eq!(r.at(), "release/wait_release");
    assert_eq!(r.kinds(), ["refresh_fact", "set_timer"]);
    assert_eq!(r.projection().waiting_on, Some(WaitingOn::External));

    r.on(TaskEvent::Event {
        event: Event::new(Payload::Release(Release {
            action: "published".into(),
            tag: "v1.2.0".into(),
            ..Default::default()
        })),
    });
    assert_eq!(r.kinds(), ["create_relations", "call_action"]);
    let Some(Effect::CallAction {
        call_id,
        args,
        name,
    }) =
        r.fx.iter()
            .find(|e| matches!(e, Effect::CallAction { .. }))
            .cloned()
    else {
        panic!()
    };
    assert_eq!(
        (name.as_str(), args["body"].as_str()),
        ("comment.create", Some("Released v1.2.0 for #7"))
    );
    assert_eq!(r.s.doc.relations.len(), 2);
    r.on(TaskEvent::ActionFinished {
        call_id,
        ok: true,
        result: Value::Null,
    });
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
    assert_eq!(r.projection().column.as_deref(), Some("done"));
    assert_eq!(
        r.s.path,
        [
            "gate/wait",
            "gate/done",
            "work/implement",
            "work/check",
            "work/done",
            "release/wait_release",
            "release/announce",
            "release/done"
        ]
    );

    // Ended tasks ignore everything.
    let before = r.s.clone();
    r.on(comment("/start", true));
    assert_eq!((r.s.clone(), r.fx.len()), (before, 0));
}

#[test]
fn unmapped_exit_ends_task() {
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(label("reject"));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "rejected".into()
        }
    );
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(label("other"));
    assert_eq!(
        r.s.status,
        Status::Waiting,
        "no branch matched: keep waiting"
    );
    r.on(label("go"));
    assert_eq!(r.at(), "work/implement");
}

#[test]
fn visit_caps_backward_exits_and_entry_caps() {
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(comment("/start", true));
    // Not pushed: implement loops; the 4th visit goes to on_limit → exit failed → gate.
    r.finish("success", false).finish("failure", false);
    assert_eq!(r.at(), "work/implement");
    r.finish("success", false);
    assert_eq!(r.at(), "gate/wait", "{:?}", r.s.notes);
    assert_eq!(r.s.entries["gate"], 2);
    // Fresh phase on re-entry: visits reset.
    r.on(comment("/start", true));
    assert_eq!(
        (r.at(), r.s.frames[0].visits["implement"]),
        ("work/implement".into(), 1)
    );
    r.finish("failure", false)
        .finish("failure", false)
        .finish("failure", false);
    assert_eq!(r.at(), "gate/wait");
    // Third entry into `work` exceeds max_entries 2 → on_limit (limit_reached) → unmapped → ends.
    r.on(comment("/start", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: EXIT_LIMIT.into()
        }
    );
}

#[test]
fn steering_and_stop() {
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(comment("/start", true));
    let run = r.run_id();
    r.on(comment("also check the logs", false));
    assert_eq!(r.kinds(), ["steer"]);
    let Effect::Steer { run_id, .. } = &r.fx[0] else {
        panic!()
    };
    assert_eq!(run_id, &run);
    r.on(comment("/stop", false));
    assert_eq!(
        r.kinds(),
        ["steer"],
        "a non-writer's /stop is just steering"
    );
    r.on(comment("/stop", true));
    assert_eq!(r.kinds(), ["stop_step"]);
    assert_eq!(r.at(), "work/implement");
    r.finish("interrupted", false);
    assert_eq!(r.at(), "gate/wait", "interrupted → stopped → failed → gate");
    // Labels are not steering for this step.
    r.on(comment("/start", true)).on(label("x"));
    assert!(r.kinds().is_empty());
}

#[test]
fn stall_pause_resume() {
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(comment("/start", true));
    r.on(TaskEvent::StallDetected { reason: None });
    assert!(matches!(r.s.status, Status::Paused { ref reason, .. } if reason == "stalled"));
    let p = r.projection();
    assert_eq!(
        (p.status.as_str(), p.column.as_deref(), p.needs_you),
        ("paused", Some("stuck"), true)
    );
    r.on(TaskEvent::Resume);
    assert_eq!(r.s.status, Status::Running);
    assert!(r.kinds().is_empty(), "resume does not restart the step");
    r.on(TaskEvent::StallDetected { reason: None });
    // A result arriving while stalled un-pauses and continues.
    r.finish("success", true);
    assert_eq!(r.at(), "work/check");
    assert_eq!(r.s.status, Status::Running);
}

#[test]
fn stale_and_foreign_results_ignored() {
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(comment("/start", true));
    let before = r.s.clone();
    r.on(TaskEvent::StepFinished(StepFinished {
        run_id: "run-999".into(),
        exit: "success".into(),
        ..Default::default()
    }));
    assert_eq!(r.s, before);
    r.on(TaskEvent::Timer {
        timer_id: "timer-1".into(),
    });
    assert_eq!(r.s, before);
}

#[test]
fn timer_and_fact_events_at_waits() {
    let mut r = Run::new(resolved(&base()));
    r.on(start())
        .on(comment("/start", true))
        .finish("success", true)
        .finish("success", true);
    assert_eq!(r.at(), "release/wait_release");
    let timer = r.s.timer.clone().unwrap();
    r.on(TaskEvent::FactUpdated {
        fact: "ci".into(),
        value: json!({"sha": "abc", "state": "success"}),
    });
    assert_eq!(r.s.doc.facts.ci.as_ref().unwrap().state, "success");
    assert_eq!(
        r.at(),
        "release/wait_release",
        "this wait does not accept facts"
    );
    r.on(TaskEvent::Timer { timer_id: timer });
    assert_eq!(r.at(), "release/announce");
}

#[test]
fn missing_required_input_pauses_and_resume_reenters() {
    let m = base().put(
        LIB,
        "steps/agent.yaml",
        &AGENT.replace("required: false", "required: true"),
    );
    let mut r = Run::new(resolved(&m));
    r.on(start()).on(comment("/start", true));
    let Status::Paused {
        reason, resume_at, ..
    } = r.s.status.clone()
    else {
        panic!("{:?}", r.s.status)
    };
    assert!(reason.contains("missing required input `plan`"), "{reason}");
    assert_eq!(resume_at.as_deref(), Some("implement"));
    r.s.doc.outputs.insert("plan".into(), json!("the plan"));
    r.on(TaskEvent::Resume);
    let RunExec::Agent { prompt, .. } = &r.run_step().exec else {
        panic!()
    };
    assert!(prompt.contains("the plan"));
}

#[test]
fn retry_reruns_step() {
    let m = base().put(
        LIB,
        "steps/agent.yaml",
        &format!("{AGENT}retry: {{max: 1}}\n"),
    );
    let mut r = Run::new(resolved(&m));
    r.on(start()).on(comment("/start", true));
    r.finish("failure", false);
    assert_eq!(r.kinds(), ["run_step"]);
    assert_eq!(r.run_step().attempt, 2);
    assert_eq!(
        r.s.frames[0].visits["implement"], 1,
        "a retry is not a visit"
    );
    r.finish("failure", false);
    assert_eq!(
        r.run_step().attempt,
        1,
        "retries exhausted: the edge loops back to a fresh visit"
    );
}

#[test]
fn nested_phases() {
    let outer = r#"version: 1
exits: [bad]
start: inner
nodes:
  inner:
    phase: {use: .enzarb/phases/gate2, with: {command: go}}
    next:
      - {if: "exit == 'rejected'", to: bad}
      - {else: fin}
  bad: {end: {status: bad}}
  fin: {end: {status: done}}
"#;
    let m = base()
        .put(APP, ".enzarb/phases/outer.yaml", outer)
        .put(
            APP,
            ".enzarb/phases/gate2.yaml",
            &GATE.replace(
                "column: waiting",
                "column: waiting\n      waiting_on: agent",
            ),
        )
        .put(
            APP,
            ".enzarb/workflows/n.yaml",
            "version: 1\nphases:\n  - use: .enzarb/phases/outer\n    exits: {bad: {end: nope}}\n",
        );
    let res = match resolve(&entry(".enzarb/workflows/n.yaml"), &m, &opts()) {
        Ok((r, _)) => r,
        Err(d) => panic!("{d}"),
    };
    let mut r = Run::new(res.clone());
    r.on(start());
    assert_eq!(r.at(), "outer/inner/wait");
    assert_eq!(r.projection().phase, "outer/inner");
    assert_eq!(r.projection().waiting_on, Some(WaitingOn::Agent));
    r.on(comment("/go", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
    let mut r = Run::new(res);
    r.on(start()).on(label("reject"));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "nope".into()
        }
    );
}

#[test]
fn engine_loop_without_wait_pauses() {
    let m = base()
        .put(APP, ".enzarb/phases/spin.yaml", "version: 1\nstart: a\nnodes:\n  a: {action: {name: noop}, next: a, max_visits: 1000000}\n")
        .put(APP, ".enzarb/workflows/s.yaml", "version: 1\nphases:\n  - use: .enzarb/phases/spin\n");
    let (res, _) = resolve(&entry(".enzarb/workflows/s.yaml"), &m, &opts()).unwrap();
    let mut r = Run::new(res);
    r.on(start());
    // Actions are pending until finished, so this does not spin; the cap applies to visits.
    let Some(Pending::Action { call_id }) = r.s.pending.clone() else {
        panic!()
    };
    r.on(TaskEvent::ActionFinished {
        call_id,
        ok: false,
        result: Value::Null,
    });
    assert_eq!(r.s.frames[0].visits["a"], 2);
    assert_eq!(r.s.last["exit"], "failure");
}

#[test]
fn step_not_started_pauses_and_allows_reresolution() {
    let res = resolved(&base());
    let mut r = Run::new(res.clone());
    r.on(start()).on(comment("/start", true));
    let run_id = r.run_id();
    let node = r.at();
    // While it is pending the step defers re-resolution.
    assert_eq!(reresolve(&res, &r.s).1, Reresolution::Deferred);
    // A foreign run id is ignored.
    r.on(TaskEvent::StepNotStarted {
        run_id: "other".into(),
        reason: "x".into(),
    });
    assert_eq!(r.run_id(), run_id);
    r.on(TaskEvent::StepNotStarted {
        run_id: run_id.clone(),
        reason: "the GitHub App lacks actions: read requested by step work/do".into(),
    });
    assert_eq!(r.s.pending, None, "a step that never started is forgotten");
    assert!(
        matches!(&r.s.status, Status::Paused { reason, .. } if reason.contains("actions: read"))
    );
    assert_eq!(r.projection().status, "paused");
    // Nothing runs, so the task can take a new definition while paused.
    assert_eq!(reresolve(&res, &r.s).1, Reresolution::Upgraded);
    // Resume re-enters the step node: a new run starts.
    r.on(TaskEvent::Resume);
    assert_eq!(r.at(), node);
    assert_eq!(r.kinds(), ["run_step"]);
    assert_ne!(r.run_id(), run_id);
}

#[test]
fn cancel_stops_the_running_step_and_ends() {
    let mut r = Run::new(resolved(&base()));
    r.on(start()).on(comment("/start", true));
    let run_id = r.run_id();
    r.on(TaskEvent::Cancel);
    assert!(r.fx.contains(&Effect::StopStep { run_id }));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "cancelled".into()
        }
    );
    assert_eq!(r.projection().end_status.as_deref(), Some("cancelled"));
    // Ended tasks ignore it.
    r.on(TaskEvent::Cancel);
    assert!(r.fx.is_empty());
}

#[test]
fn run_ids_carry_the_task() {
    let mut r = Run::new(resolved(&base()));
    r.s.doc.task.id = "42".into();
    r.on(start()).on(comment("/start", true));
    assert!(r.run_id().starts_with("task-42-run-"), "{}", r.run_id());
}

#[test]
fn upgrade_keeps_position_or_pins() {
    let res = resolved(&base());
    let mut r = Run::new(res.clone());
    r.on(start());
    // Same definition: upgraded.
    assert_eq!(reresolve(&res, &r.s).1, Reresolution::Upgraded);
    // The wait node was renamed: pinned, with a note.
    let renamed = resolved(
        &base().put(
            LIB,
            "phases/gate.yaml",
            &GATE
                .replace("start: wait", "start: hold")
                .replace("  wait:\n    wait:", "  hold:\n    wait:"),
        ),
    );
    let (s, out) = reresolve(&renamed, &r.s);
    assert!(matches!(out, Reresolution::Pinned { .. }), "{out:?}");
    assert!(s.pinned.is_some() && s.notes.iter().any(|n| n.starts_with("pinned")));
    // Back on a definition that has the node: unpinned.
    let (s2, out) = reresolve(&res, &s);
    assert_eq!((out, s2.pinned), (Reresolution::Upgraded, None));
    // While a step runs, the step finishes on its definition.
    r.on(comment("/start", true));
    assert_eq!(reresolve(&renamed, &r.s).1, Reresolution::Deferred);
}

#[test]
fn deterministic() {
    let res = resolved(&base());
    let evs = [start(), comment("/start", true)];
    let run = || {
        let mut s = TaskState::new(doc());
        let mut all = vec![];
        for e in &evs {
            let (n, fx) = step(&res, &s, e);
            s = n;
            all.push(serde_json::to_string(&fx).unwrap());
        }
        (serde_json::to_string(&s).unwrap(), all)
    };
    assert_eq!(run(), run());
    // State round-trips through JSON (the engine persists it).
    let mut r = Run::new(res);
    r.on(start());
    let back: TaskState = serde_json::from_value(serde_json::to_value(&r.s).unwrap()).unwrap();
    assert_eq!(back, r.s);
}

#[test]
fn wait_steer_goes_to_last_agent_session() {
    let phase = r#"version: 1
start: s
nodes:
  s: {step: {inline: {agent: {runtime: rt, prompt: hi}}}, next: w}
  w:
    wait: {events: [{event: comment, if: "event.command == 'ok'"}], steer: [comment]}
    next: d
  d: {end: {status: done}}
"#;
    let m = base()
        .put(APP, ".enzarb/phases/p.yaml", phase)
        .put(APP, "rt.yaml", RUNTIME)
        .put(
            APP,
            ".enzarb/workflows/w.yaml",
            "version: 1\nphases:\n  - use: .enzarb/phases/p\n",
        );
    let (res, _) = resolve(&entry(".enzarb/workflows/w.yaml"), &m, &opts()).unwrap();
    let mut r = Run::new(res);
    r.on(start());
    let run = r.run_id();
    r.finish("success", false);
    r.on(comment("what about X?", false));
    assert!(matches!(&r.fx[..], [Effect::Steer { run_id, .. }] if *run_id == run));
    r.on(comment("/ok", false));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
}

#[test]
fn resolved_serializes() {
    let res = resolved(&base());
    let back: Resolved = serde_json::from_value(serde_json::to_value(&res).unwrap()).unwrap();
    assert_eq!(back, res);
}

#[test]
fn lint_workspace_on_disk() {
    let dir = std::env::temp_dir().join(format!("enzarb-flow-lint-{}", std::process::id()));
    let lib = dir.join("lib");
    let app = dir.join("app");
    let w = |root: &std::path::Path, p: &str, t: &str| {
        let f = root.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    };
    for (p, t) in [
        ("phases/gate.yaml", GATE),
        ("phases/work.yaml", WORK),
        ("steps/agent.yaml", AGENT),
        ("prompts/work.md", "Fix {{ subjects.issue.title }}\n"),
        ("runtimes/claude.yaml", RUNTIME),
        ("board.yaml", BOARD),
    ] {
        w(&lib, p, t);
    }
    w(&app, ".enzarb/phases/release.yaml", RELEASE);
    w(&app, ".enzarb/workflows/issue.yaml", ISSUE);
    w(
        &app,
        ".enzarb/workflows.yaml",
        "imports: {std: lib/std@v1}\nbindings:\n  - {id: issues, on: issue, if: \"event.action == 'opened'\", use: .enzarb/workflows/issue}\n",
    );
    w(
        &app,
        ".enzarb/tests/happy.yaml",
        r#"workflow: .enzarb/workflows/issue.yaml
start: {type: issue, action: opened, number: 7}
stubs:
  work/implement: [{exit: success, git: {pushed: true}}]
  check: [{exit: success}]
events:
  - event: {type: comment, body: /start, command: start, actor: {login: u, can_write: true}}
    expect:
      at: release/wait_release
      projection: {reason: awaiting_release, column: waiting, needs_you: false}
  - event: {type: release, action: published, tag: v1}
expect:
  status: ended:done
  path: [gate/wait, gate/done, work/implement, work/check, work/done, release/wait_release, release/announce, release/done]
"#,
    );
    w(
        &app,
        ".enzarb/tests/bad.yaml",
        "workflow: .enzarb/workflows/issue.yaml\nstart: {type: issue, action: opened, number: 7}\nexpect:\n  status: ended:done\n",
    );
    let cfg = crate::lint::Config {
        dir: app.clone(),
        repo: APP.into(),
        libraries: vec![("std".into(), lib.clone())],
    };
    let ws = crate::lint::Workspace::new(cfg.clone()).unwrap();
    let errs: Vec<_> = ws.check().into_iter().filter(|d| d.is_error()).collect();
    assert!(errs.is_empty(), "{errs:?}");
    let (n, fails) = ws.test();
    assert_eq!(n, 2);
    assert_eq!(fails.len(), 1, "{fails:?}");
    assert!(
        fails[0].file.ends_with(".enzarb/tests/bad.yaml")
            && fails[0].line == 4
            && fails[0].message.contains("`waiting`"),
        "{}",
        fails[0]
    );

    // Errors in the library are reported at the library's local path.
    w(
        &lib,
        "phases/gate.yaml",
        &GATE.replace("to: rejected", "to: rejectd"),
    );
    let ws = crate::lint::Workspace::new(cfg).unwrap();
    let errs: Vec<_> = ws.check().into_iter().filter(|d| d.is_error()).collect();
    assert!(!errs.is_empty());
    assert!(
        errs.iter()
            .all(|d| d.file == lib.join("phases/gate.yaml").display().to_string()),
        "{errs:?}"
    );
    assert!(errs[0].github().starts_with("::error file="));
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------- checks

const CI_CHECK: &str = r#"
version: 1
id: ci-never-started
kind: dead_wait
severity: warning
text: "{task} waits on CI that has not started"
when:
  all:
    - waiting_on: external
    - fact_count: {name: ci.check_runs, eq: 0}
    - any:
        - fact: {name: mergeable, is: unknown}
        - fact: {name: mergeable, is: false}
    - idle_for: {no_event: 30m}
"#;

fn check(text: &str) -> crate::format::CheckFile {
    crate::yaml::SourceFile::parse("c.yaml", text)
        .unwrap()
        .typed()
        .unwrap()
}

#[test]
fn check_file_parses_and_validates() {
    let c = check(CI_CHECK);
    assert_eq!(c.id, "ci-never-started");
    assert_eq!(c.scope, crate::format::CheckScope::Task);
    assert!(c.problems().is_empty(), "{:?}", c.problems());
}

#[test]
fn check_file_problems() {
    let bad = CI_CHECK.replace("30m", "soon").replace(
        "eq: 0",
        "eq: 0}\n    - fact_unknown_for: {name: x, refreshes: 0",
    );
    let p = check(&bad).problems();
    assert!(
        p.iter().any(|m| m.contains("`soon` is not a duration")),
        "{p:?}"
    );
    assert!(p.iter().any(|m| m.contains("at least 1")), "{p:?}");
    let platform = check(
        "version: 1\nid: x\nkind: platform_failure\nseverity: info\nscope: platform\ntext: t\nwhen: {waiting_on: external}\n",
    );
    assert!(platform.problems()[0].contains("scope: task"));
    let empty =
        check("version: 1\nid: x\nkind: dead_wait\nseverity: info\ntext: t\nwhen: {all: []}\n");
    assert!(empty.problems()[0].contains("at least one"));
}

#[test]
fn check_file_rejects_unknown_leaves() {
    let r = crate::yaml::SourceFile::parse(
        "c.yaml",
        "version: 1\nid: x\nkind: dead_wait\nseverity: info\ntext: t\nwhen: {run_script: 'x'}\n",
    )
    .unwrap()
    .typed::<crate::format::CheckFile>();
    assert!(r.is_err());
}

#[test]
fn lint_checks_directory() {
    let dir = std::env::temp_dir().join(format!("enzarb-flow-checks-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("checks")).unwrap();
    std::fs::write(dir.join("checks/ci.yaml"), CI_CHECK).unwrap();
    std::fs::write(dir.join("checks/bad.yaml"), CI_CHECK.replace("30m", "soon")).unwrap();
    let ws = crate::lint::Workspace::new(crate::lint::Config {
        dir: dir.clone(),
        repo: APP.into(),
        libraries: vec![],
    })
    .unwrap();
    let errs: Vec<_> = ws.check().into_iter().filter(|d| d.is_error()).collect();
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].file.ends_with("checks/bad.yaml"));
    std::fs::remove_dir_all(&dir).ok();
}
