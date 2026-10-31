use super::*;
use crate::schema::parse;
use serde_json::json;

const WORKFLOWS: &str = include_str!("../../tests/fixtures/workflows.toml");

fn labeled() -> Event {
    Event::new(
        "issues",
        Some("labeled".into()),
        json!({
            "action": "labeled",
            "label": {"name": "ez"},
            "issue": {"number": 42, "title": "Crash on start", "body": "It crashes.",
                      "user": {"login": "alice"}, "html_url": "https://github.com/o/r/issues/42"}
        }),
    )
}

fn comment(by: &str) -> Event {
    Event::new(
        "issue_comment",
        Some("created".into()),
        json!({
            "action": "created",
            "comment": {"body": "more info", "user": {"login": by}},
            "issue": {"number": 42, "user": {"login": "alice"}}
        }),
    )
}

fn run(test: bool) -> RunInfo {
    RunInfo {
        id: "r1".into(),
        attempt: 1,
        test,
    }
}

fn setup(test: bool) -> (Engine, WorkflowsFile, RunState) {
    let file = parse(WORKFLOWS).unwrap();
    let state = RunState::start(&file, "fix-issue", &labeled(), run(test)).unwrap();
    (Engine::new(), file, state)
}

fn at(state: &RunState) -> &str {
    state.current().unwrap_or("<ended>")
}

#[test]
fn starts_at_the_start_node_with_the_trigger_context() {
    let (e, file, state) = setup(false);
    assert_eq!(at(&state), "triage");
    assert_eq!(state.visits["triage"], 1);
    assert_eq!(state.subject.as_ref().unwrap().author, "alice");
    let Rendered::Agent {
        mode,
        instruction,
        result_schema,
        push,
    } = state.render(&e, &file).unwrap().unwrap()
    else {
        panic!("not an agent node")
    };
    assert_eq!(mode, "triage");
    assert_eq!(instruction, "Issue #42: Crash on start\n\nIt crashes.");
    assert!(!push);
    let schema = result_schema.unwrap();
    assert_eq!(schema["required"], json!(["decision", "summary"]));
    assert_eq!(schema["additionalProperties"], json!(false));
    assert_eq!(
        schema["properties"]["decision"]["enum"],
        json!(["fix", "needs_info"])
    );

    assert_eq!(
        RunState::start(&file, "nope", &labeled(), run(false)).unwrap_err(),
        RunError::UnknownWorkflow("nope".into())
    );
}

#[test]
fn walks_the_happy_path_to_done() {
    let (e, file, mut s) = setup(false);
    let t = s
        .advance(
            &e,
            &file,
            json!({"result": {"decision": "fix", "summary": "null deref"}, "exit": "success"}),
        )
        .unwrap();
    assert_eq!(
        t,
        Transition::Moved {
            from: "triage".into(),
            to: "implement".into(),
            edge: 0
        }
    );
    let Rendered::Agent {
        instruction, push, ..
    } = s.render(&e, &file).unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(instruction, "Fix issue #42. Plan: null deref");
    assert!(push);

    s.advance(
        &e,
        &file,
        json!({"git": {"pushed": true, "branch": "ez/fix-issue/r1"}, "exit": "success"}),
    )
    .unwrap();
    assert_eq!(at(&s), "open_pr");
    let Rendered::Action {
        action,
        title,
        body,
        ..
    } = s.render(&e, &file).unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(action, ActionKind::OpenPullRequest);
    assert_eq!(title.as_deref(), Some("Fix #42: Crash on start"));
    assert_eq!(body.as_deref(), Some("Closes #42"));

    let t = s
        .advance(&e, &file, json!({"number": 7, "url": "u"}))
        .unwrap();
    assert!(matches!(t, Transition::Moved { ref to, .. } if to == "end_done"));
    assert_eq!(
        s.status,
        RunStatus::Ended {
            status: EndStatus::Done,
            node: Some("end_done".into()),
            reason: None
        }
    );
    assert_eq!(s.nodes["open_pr"]["number"], 7);
    assert_eq!(
        s.advance(&e, &file, json!({})).unwrap_err(),
        RunError::NotRunning
    );
}

#[test]
fn test_runs_never_push() {
    let (e, file, mut s) = setup(true);
    s.advance(
        &e,
        &file,
        json!({"result": {"decision": "fix", "summary": "x"}}),
    )
    .unwrap();
    let Rendered::Agent { push, .. } = s.render(&e, &file).unwrap().unwrap() else {
        panic!()
    };
    assert!(!push);
}

#[test]
fn waits_accept_matching_events_and_take_the_resuming_event() {
    let (e, file, mut s) = setup(false);
    s.advance(
        &e,
        &file,
        json!({"result": {"decision": "needs_info", "summary": "Which OS?"}}),
    )
    .unwrap();
    assert_eq!(at(&s), "ask");
    let Rendered::Action { body, .. } = s.render(&e, &file).unwrap().unwrap() else {
        panic!()
    };
    assert_eq!(body.as_deref(), Some("Which OS?"));
    s.advance(&e, &file, json!({"number": 1, "url": "u"}))
        .unwrap();
    assert_eq!(at(&s), "wait_reply");
    assert!(matches!(
        s.render(&e, &file).unwrap().unwrap(),
        Rendered::Wait {
            timeout_secs: Some(604800),
            ..
        }
    ));

    // Wrong event, wrong author, then the author.
    assert!(!s.wait_accepts(&e, &file, &labeled()).unwrap().unwrap());
    assert!(
        !s.wait_accepts(&e, &file, &comment("mallory"))
            .unwrap()
            .unwrap()
    );
    assert!(
        s.wait_accepts(&e, &file, &comment("alice"))
            .unwrap()
            .unwrap()
    );

    s.advance(&e, &file, wait_outputs(Some(&comment("alice"))))
        .unwrap();
    assert_eq!(at(&s), "triage");
    assert_eq!(s.visits["triage"], 2);
    assert_eq!(s.event["name"], "issue_comment");
    assert_eq!(s.event["comment"]["body"], "more info");
    // The trigger's subject stays.
    assert_eq!(s.subject.as_ref().unwrap().number, 42);
}

#[test]
fn wait_timeouts_take_the_timed_out_edge_and_keep_the_event() {
    let (e, file, mut s) = setup(false);
    s.advance(
        &e,
        &file,
        json!({"result": {"decision": "needs_info", "summary": "?"}}),
    )
    .unwrap();
    s.advance(&e, &file, json!({"number": 1, "url": "u"}))
        .unwrap();
    s.advance(&e, &file, wait_outputs(None)).unwrap();
    assert_eq!(
        s.status,
        RunStatus::Ended {
            status: EndStatus::Cancelled,
            node: Some("end_stale".into()),
            reason: Some("ended at `end_stale` (cancelled) after `wait_reply`".into())
        }
    );
    assert_eq!(s.event["name"], "issues");
}

#[test]
fn wait_accepts_rejects_other_node_types() {
    let (e, file, s) = setup(false);
    assert!(matches!(
        s.wait_accepts(&e, &file, &comment("alice")),
        Err(RunError::WrongNodeType { .. })
    ));
}

#[test]
fn loops_end_at_max_visits() {
    let (e, file, mut s) = setup(false);
    for _ in 0..5 {
        assert_eq!(at(&s), "triage");
        s.advance(
            &e,
            &file,
            json!({"result": {"decision": "needs_info", "summary": "?"}}),
        )
        .unwrap();
        s.advance(&e, &file, json!({"number": 1, "url": "u"}))
            .unwrap();
        s.advance(&e, &file, wait_outputs(Some(&comment("alice"))))
            .unwrap();
    }
    assert_eq!(
        s.status,
        RunStatus::Ended {
            status: EndStatus::Failed,
            node: None,
            reason: Some("node `triage` reached max_visits (5)".into())
        }
    );
}

#[test]
fn failing_edges_and_templates_end_the_run() {
    let (e, file, mut s) = setup(false);
    // `result.decision` on a missing result: the condition fails, the run ends failed.
    let t = s.advance(&e, &file, json!({"exit": "failure"})).unwrap();
    let Transition::Ended { status, reason } = t else {
        panic!("{t:?}")
    };
    assert_eq!(status, EndStatus::Failed);
    assert!(reason.unwrap().contains("next[0]"));

    // The plan template needs nodes.triage.result.summary.
    let (e, file, mut s) = setup(false);
    s.advance(&e, &file, json!({"result": {"decision": "fix"}}))
        .unwrap();
    let err = s.render(&e, &file).unwrap().unwrap_err();
    assert!(err.contains("instruction"), "{err}");
    s.fail(err);
    assert!(s.is_ended());
    s.cancel("ignored after the end");
    assert!(matches!(
        s.status,
        RunStatus::Ended {
            status: EndStatus::Failed,
            ..
        }
    ));
}

#[test]
fn state_round_trips_through_json() {
    let (e, file, mut s) = setup(false);
    s.advance(
        &e,
        &file,
        json!({"result": {"decision": "needs_info", "summary": "?"}}),
    )
    .unwrap();
    let text = serde_json::to_string(&s).unwrap();
    let back: RunState = serde_json::from_str(&text).unwrap();
    assert_eq!(back, s);
    assert!(text.contains(r#""state":"at""#), "{text}");
}

#[test]
fn shell_commands_are_quoted() {
    let file = parse(
        r#"
version = 1
[workflows.w]
on = ["issues"]
start = "c"
[workflows.w.nodes.c]
type = "command"
run = "echo {{ event.issue.title }} > out.txt"
next = "e"
[workflows.w.nodes.e]
type = "end"
status = "done"
"#,
    )
    .unwrap();
    let mut ev = labeled();
    ev.payload["issue"]["title"] = json!("x'; rm -rf / #");
    let s = RunState::start(&file, "w", &ev, run(false)).unwrap();
    let Rendered::Command { script, task, .. } = s.render(&Engine::new(), &file).unwrap().unwrap()
    else {
        panic!()
    };
    assert_eq!(task, None);
    assert_eq!(script.unwrap(), r#"echo 'x'\''; rm -rf / #' > out.txt"#);
}

#[test]
fn a_start_node_that_is_an_end_ends_at_once() {
    let file = parse(
        "version = 1\n[workflows.w]\non = [\"issues\"]\nstart = \"e\"\nnodes.e = { type = \"end\", status = \"cancelled\" }\n",
    )
    .unwrap();
    let s = RunState::start(&file, "w", &labeled(), run(false)).unwrap();
    assert!(matches!(
        s.status,
        RunStatus::Ended {
            status: EndStatus::Cancelled,
            ..
        }
    ));
}

const END_REASONS: &str = r#"
version = 1
[workflows.w]
on = ["issues.labeled"]
start = "a"
[workflows.w.nodes.a]
type = "command"
task = "t"
next = [{ if = "exit_code == 0", to = "ok" }, { if = "exit_code == 1", to = "custom" }, { if = "exit_code == 2", to = "bad" }, { to = "plain" }]
[workflows.w.nodes.ok]
type = "end"
status = "done"
[workflows.w.nodes.custom]
type = "end"
status = "failed"
reason = "nothing to fix: {{ nodes.a.output }}"
[workflows.w.nodes.bad]
type = "end"
status = "failed"
reason = "{{ nodes.missing.x }}"
[workflows.w.nodes.plain]
type = "end"
status = "failed"
"#;

fn end_reason_of(exit: i64) -> RunStatus {
    let file = parse(END_REASONS).unwrap();
    let e = Engine::new();
    let mut s = RunState::start(&file, "w", &labeled(), run(false)).unwrap();
    s.advance(&e, &file, json!({"exit_code": exit, "output": "no diff"}))
        .unwrap();
    s.status
}

#[test]
fn end_reasons_render_or_default() {
    let reason = |st: RunStatus| match st {
        RunStatus::Ended { status, reason, .. } => (status, reason),
        o => panic!("{o:?}"),
    };
    assert_eq!(reason(end_reason_of(0)), (EndStatus::Done, None));
    assert_eq!(
        reason(end_reason_of(1)),
        (EndStatus::Failed, Some("nothing to fix: no diff".into()))
    );
    assert_eq!(
        reason(end_reason_of(3)),
        (
            EndStatus::Failed,
            Some("ended at `plain` (failed) after `a`".into())
        )
    );
    let (st, r) = reason(end_reason_of(2));
    assert_eq!(st, EndStatus::Failed);
    assert!(r.unwrap().contains("reason"));
}

#[test]
fn end_reason_is_validated() {
    let bad = END_REASONS.replace("{{ nodes.a.output }}", "{{ bogus.x }}");
    let file = parse(&bad).unwrap();
    let rep = crate::validate::validate(&Engine::new(), &file, None);
    assert!(!rep.is_ok(), "{rep:?}");
}

#[test]
fn vars_render_and_drive_edges() {
    let text = "version = 1\n[workflows.w]\non = [\"issues\"]\nstart = \"a\"\n[workflows.w.vars]\ngate = true\nlabel = \"hi\"\n[workflows.w.nodes.a]\ntype = \"command\"\nrun = \"echo {{ vars.label }}\"\nnext = [{ if = \"vars.gate\", to = \"yes\" }, { to = \"no\" }]\n[workflows.w.nodes.yes]\ntype = \"end\"\nstatus = \"done\"\n[workflows.w.nodes.no]\ntype = \"end\"\nstatus = \"failed\"\n";
    let file = parse(text).unwrap();
    let e = Engine::new();
    let mut s = RunState::start(&file, "w", &labeled(), run(false)).unwrap();
    let Rendered::Command { script, .. } = s.render(&e, &file).unwrap().unwrap() else {
        panic!("not a command")
    };
    assert_eq!(script.unwrap(), "echo 'hi'");
    s.advance(&e, &file, json!({"exit_code": 0})).unwrap();
    assert_eq!(
        s.status,
        RunStatus::Ended {
            status: EndStatus::Done,
            node: Some("yes".into()),
            reason: None
        }
    );
    let back: RunState = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(back.vars["gate"], json!(true));
}
