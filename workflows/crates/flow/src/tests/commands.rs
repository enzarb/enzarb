//! The command model: workflow defaults, wait overrides, routing, the reply to a command the
//! wait does not accept, `/cancel`, the projection list and the lint rules.

use super::*;
use crate::events::SubjectRef;

const GATE_CMDS: &str = r#"version: 1
exits: [again]
start: wait
nodes:
  wait:
    wait:
      events:
        - {event: comment, from: writer, if: "event.command == 'go'"}
      reason: awaiting_go
      column: waiting
      actions: [{label: Go, command: go}]
      commands:
        - {name: again, label: Again, description: Leave through the exit, to: "exit:again"}
    next: done
  after:
    wait:
      events: [{event: label}]
    next: done
  done: {end: {status: done}}
"#;

fn wf(defaults: &str, phase: &str) -> Mem {
    let exits = if phase.contains("exits: [again]") {
        ", exits: {again: gate}"
    } else {
        ""
    };
    Mem::default()
        .put(LIB, "phases/gate.yaml", phase)
        .put(LIB, "board.yaml", BOARD)
        .put(
            APP,
            ".enzarb/workflows/issue.yaml",
            &format!(
                "version: 1\nimports: {{std: lib/std@v1}}\nboard: std/board\n{defaults}phases:\n  - {{use: std/phases/gate{exits}}}\n"
            ),
        )
}

const DEFAULTS: &str = r#"commands:
  - {name: retry, label: Retry, description: Run the phase again, to: start}
"#;

fn run(m: &Mem) -> Run {
    let mut r = Run::new(resolved(m));
    r.on(start());
    r
}

fn reply(r: &Run) -> Option<(Option<u64>, String)> {
    r.fx.iter().find_map(|e| match e {
        Effect::Reply { number, body } => Some((*number, body.clone())),
        _ => None,
    })
}

fn bot_comment(body: &str) -> TaskEvent {
    let mut e = Event::new(Payload::Comment(Comment::parsed(body, Some("enzarb"))));
    e.actor = Some(Actor {
        login: "enzarb[bot]".into(),
        can_write: true,
        bot: true,
    });
    TaskEvent::Event { event: e }
}

#[test]
fn a_default_command_routes_where_the_workflow_says() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    assert_eq!(r.at(), "gate/wait");
    let visits = r.s.frames[0].visits["wait"];
    r.on(comment("/retry", true));
    assert_eq!(r.at(), "gate/wait");
    assert_eq!(
        r.s.frames[0].visits["wait"],
        visits + 1,
        "entered the start node again"
    );
    assert!(reply(&r).is_none());
}

#[test]
fn a_wait_declares_its_own_route_and_listing() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    let names: Vec<_> = r
        .projection()
        .commands
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(names, ["again", "retry", "go", "cancel"]);
    let again = &r.projection().commands[0];
    assert_eq!(
        (
            again.label.as_str(),
            again.description.as_deref(),
            again.from
        ),
        ("Again", Some("Leave through the exit"), Sender::Writer)
    );
    // The exit leaves the phase; the workflow routes `again` back to `gate`.
    r.on(comment("/again", true));
    assert_eq!(r.at(), "gate/wait");
    assert_eq!(r.s.entries["gate"], 2);
}

#[test]
fn a_command_the_wait_does_not_accept_gets_one_reply_listing_the_accepted_ones() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    r.on(comment("/frobnicate", true));
    assert_eq!(r.at(), "gate/wait");
    let (_, body) = reply(&r).expect("a reply");
    assert!(
        body.starts_with("`/frobnicate` does not apply right now."),
        "{body}"
    );
    for c in ["`/again`", "`/retry`", "`/go`", "`/cancel`"] {
        assert!(body.contains(c), "{c} in {body}");
    }
    assert_eq!(
        r.fx.iter()
            .filter(|e| matches!(e, Effect::Reply { .. }))
            .count(),
        1
    );
}

#[test]
fn the_reply_names_the_issue_or_pull_request_the_comment_is_on() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    let TaskEvent::Event { mut event } = comment("/nope", true) else {
        unreachable!()
    };
    event.subject = Some(SubjectRef {
        kind: crate::events::SubjectKind::PullRequest,
        number: 12,
    });
    r.on(TaskEvent::Event { event });
    assert_eq!(reply(&r).unwrap().0, Some(12));
}

#[test]
fn a_writer_only_command_from_a_reader_says_so_and_does_not_run() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    let before = r.s.frames[0].visits["wait"];
    r.on(comment("/retry", false));
    assert_eq!(r.s.frames[0].visits["wait"], before);
    let (_, body) = reply(&r).expect("a reply");
    assert!(body.contains("needs write access"), "{body}");
    // The reader is offered no writer commands.
    assert!(body.ends_with("none."), "{body}");
}

#[test]
fn bots_and_plain_comments_get_no_reply() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    r.on(bot_comment("/frobnicate"));
    assert!(reply(&r).is_none());
    r.on(comment("just talking", true));
    assert!(reply(&r).is_none());
    // Not waiting: nothing to list.
    r.on(comment("/go", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
    r.on(comment("/frobnicate", true));
    assert!(reply(&r).is_none());
}

#[test]
fn cancel_is_native_for_writers_only() {
    let mut r = run(&wf(DEFAULTS, GATE_CMDS));
    r.on(comment("/cancel", false));
    assert_eq!(r.s.status, Status::Waiting);
    assert!(reply(&r).expect("a reply").1.contains("needs write access"));
    r.on(comment("/cancel", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "cancelled".into()
        }
    );
}

#[test]
fn the_waits_own_rules_win_over_a_default() {
    let phase = r#"version: 1
exits: [mine]
start: wait
nodes:
  wait:
    wait:
      events: [{event: comment, if: "not event.actor.bot"}]
    next:
      - {if: "event.command == 'retry' and event.actor.can_write", to: mine}
      - {if: "not event.command", to: done}
  mine: {end: {status: mine}}
  done: {end: {status: done}}
"#;
    let mut r = run(&wf(DEFAULTS, phase));
    r.on(comment("/retry", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "mine".into()
        }
    );
}

#[test]
fn old_libraries_without_commands_list_what_their_rules_match() {
    let m = wf("", GATE_CMDS.replace("      commands:\n        - {name: again, label: Again, description: Leave through the exit, to: \"exit:again\"}\n", "").as_str());
    let (_, diags) = resolve(&entry(".enzarb/workflows/issue.yaml"), &m, &opts()).expect("loads");
    // Lint is a warning for a library that declares no commands at all.
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("does not accept `/retry`")),
        "{diags:?}"
    );
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:?}");
    let mut r = run(&m);
    let names: Vec<_> = r
        .projection()
        .commands
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(names, ["go", "cancel"]);
    r.on(comment("/retry", true));
    assert!(reply(&r).expect("a reply").1.contains("`/go`"));
    r.on(comment("/go", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
}

fn error_text(m: &Mem) -> String {
    errors(m, ".enzarb/workflows/issue.yaml")
        .iter()
        .map(|d| d.message.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lint_requires_retry_on_every_wait_once_commands_are_declared() {
    // `after` is a node of the phase with a default that cannot route there.
    let defaults = "commands:\n  - {name: retry, to: only_here}\n";
    let e = error_text(&wf(defaults, GATE_CMDS));
    assert!(e.contains("does not accept `/retry`"), "{e}");
}

#[test]
fn lint_rejects_bad_routes_and_declarations() {
    let bad_to = GATE_CMDS.replace("exit:again", "nowhere");
    assert!(error_text(&wf(DEFAULTS, &bad_to)).contains("`nowhere` is not a node"));
    let bad_exit = GATE_CMDS.replace("exit:again", "exit:missing");
    assert!(error_text(&wf(DEFAULTS, &bad_exit)).contains("no such exit"));
    let no_route = GATE_CMDS.replace(", to: \"exit:again\"", "");
    assert!(error_text(&wf(DEFAULTS, &no_route)).contains("has no route"));
    let bad_name = GATE_CMDS.replace("name: again", "name: \"two words\"");
    assert!(error_text(&wf(DEFAULTS, &bad_name)).contains("single word"));
    let bad_from = GATE_CMDS.replace("name: again,", "name: again, from: nobody,");
    assert!(!error_text(&wf(DEFAULTS, &bad_from)).is_empty());
    let default_without_route = "commands:\n  - {name: retry}\n";
    assert!(error_text(&wf(default_without_route, GATE_CMDS)).contains("needs a route"));
    let bad_goto = "commands:\n  - {name: retry, to: \"goto:nope\"}\n";
    assert!(error_text(&wf(bad_goto, GATE_CMDS)).contains("not a phase id"));
}

#[test]
fn a_command_name_can_be_a_param() {
    let phase = r#"version: 1
params:
  approve: {type: string, default: approve}
start: wait
nodes:
  wait:
    wait:
      events: [{event: label}]
      commands: [{name: {param: approve}, to: done}]
    next: done
  done: {end: {status: done}}
"#;
    let mut r = run(&wf(DEFAULTS, phase));
    assert!(r.projection().commands.iter().any(|c| c.name == "approve"));
    r.on(comment("/approve", true));
    assert_eq!(
        r.s.status,
        Status::Ended {
            status: "done".into()
        }
    );
}

#[test]
fn feedback_is_new_people_comments_without_commands() {
    let c = |author: &str, body: &str, at: &str, command: Option<&str>, bot: bool| json!({"author": author, "body": body, "at": at, "command": command, "bot": bot});
    let mut doc = json!({
        "vars": {"feedback_since": "2026-10-17T10:00:00Z"},
        "subjects": {"issue": null, "pull_requests": [{
            "linked_at": "2026-10-17T09:00:00+00:00",
            "comments": [
                c("a", "old", "2026-10-17T09:30:00Z", None, false),
                c("a", "/approve", "2026-10-17T11:00:00Z", Some("approve"), false),
                c("enzarb[bot]", "review", "2026-10-17T11:00:00Z", None, true),
                c("a", "new", "2026-10-17T11:00:01+00:00", None, false),
            ]}]}
    });
    crate::document::add_feedback(&mut doc);
    let bodies: Vec<_> = doc["subjects"]["pull_requests"][0]["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(bodies, ["new"]);
    // Without a watermark the subject's link time is the floor.
    doc["vars"] = json!({});
    crate::document::add_feedback(&mut doc);
    let bodies: Vec<_> = doc["subjects"]["pull_requests"][0]["feedback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["body"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(bodies, ["old", "new"]);
}
