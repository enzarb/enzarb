//! Graph workflows: enzarb's own start gate (a wait on a `/plan` comment, the `plan` label or
//! the UI button), built from generic pieces (comment `command`, `labeled`, wait `actions`).

use super::*;
use crate::machine::graph::{BlockingDependency, GraphPhase, GraphSource, GraphTask};

const ENZARB: &str = include_str!("../../../../../../.enzarb/workflows/issue.yaml");

fn gate_ctx(perm: Permission) -> Ctx {
    Ctx {
        graph: Some(GraphSource {
            name: "issue".into(),
            path: ".enzarb/workflows/issue.yaml".into(),
            sha: "c0ffee".into(),
            source: ENZARB.into(),
            prompts: Default::default(),
        }),
        ..ctx(perm)
    }
}

#[track_caller]
fn graph_in(t: &Transition, phase: GraphPhase) -> GraphTask {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::Graph(g) => {
            assert_eq!(g.phase, phase, "note: {}", t.note);
            g.clone()
        }
        other => panic!("not a graph state: {other:?}"),
    }
}

fn label_by(id: &str, who: Actor, label: &str) -> EngineEvent {
    let mut e = conn(id, EventType::WorkItemLabeled, issue_item(), who, None);
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.label = Some(label.into());
    }
    e
}

/// The issue opened: it waits at `gate` for a human, with the button declared there.
fn at_gate() -> WorkflowState {
    let c = gate_ctx(Permission::Read);
    let t = transition(&c, &WorkflowState::New(WorkflowKind::Issue), &opened("e1"));
    let g = graph_in(&t, GraphPhase::Waiting);
    assert_eq!(g.data.graph.node, "gate");
    assert!(kinds(&t).iter().all(|k| *k != JobKind::AgentSegment));
    let sd = t.next.state_data();
    assert_eq!(
        sd["wait"],
        json!({
            "waiting_on": "human",
            "reason": "awaiting_start",
            "text": "Waiting for a maintainer to start planning (/plan)",
            "actions": [{"label": "Start planning", "command": "plan"}],
        })
    );
    t.next
}

#[track_caller]
fn planning_started(t: &Transition) {
    let g = graph_in(t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "route");
    let (_, _, input) = job_of(t, JobKind::AgentSegment);
    assert_eq!(input["graph"]["start"], "route");
}

#[test]
fn a_writer_comment_plan_starts_planning() {
    let s = at_gate();
    for body in [
        "/plan",
        "Go ahead.\n/PLAN",
        "@enzarb /plan",
        "@enzarb[bot]: /plan now",
    ] {
        let t = transition(
            &gate_ctx(Permission::Write),
            &s,
            &comment_by("c", "bob", 1, body),
        );
        planning_started(&t);
    }
}

#[test]
fn a_reader_plan_is_ignored() {
    let s = at_gate();
    let c = gate_ctx(Permission::Read);
    rejected(&transition(&c, &s, &comment_by("c", "eve", 1, "/plan")), &s);
    rejected(
        &transition(&c, &s, &label_by("l", actor("eve"), "plan")),
        &s,
    );
    rejected(&transition(&c, &s, &ui_comment("u", "eve", "/plan")), &s);
}

#[test]
fn other_comments_and_labels_loop_back_to_the_gate() {
    let s = at_gate();
    let c = gate_ctx(Permission::Write);
    for ev in [
        comment_by("c1", "bob", 1, "what about X?"),
        comment_by("c2", "bob", 2, "/approve"),
        comment_by("c3", "bob", 3, "please /plan"),
        label_by("l1", actor("bob"), "bug"),
    ] {
        let t = transition(&c, &s, &ev);
        let g = graph_in(&t, GraphPhase::Waiting);
        assert_eq!(g.data.graph.node, "gate", "{}", ev.id);
        assert!(kinds(&t).iter().all(|k| *k != JobKind::AgentSegment));
        assert_eq!(t.next.state_data()["wait"]["reason"], "awaiting_start");
    }
}

#[test]
fn the_plan_label_from_a_writer_starts_planning() {
    let s = at_gate();
    let t = transition(
        &gate_ctx(Permission::Write),
        &s,
        &label_by("l", actor("bob"), "plan"),
    );
    planning_started(&t);
    // enzarb's own label echo is ignored.
    let bot = Actor {
        login: "enzarb[bot]".into(),
        id: 1,
        is_bot: true,
    };
    rejected(
        &transition(
            &gate_ctx(Permission::Write),
            &s,
            &label_by("l2", bot, "plan"),
        ),
        &s,
    );
}

#[test]
fn the_ui_button_posts_plan_and_starts_planning() {
    let s = at_gate();
    let t = transition(
        &gate_ctx(Permission::Write),
        &s,
        &ui_comment("u", "bob", "/plan"),
    );
    planning_started(&t);
    let bodies = comment_bodies(&t);
    assert!(
        bodies
            .iter()
            .any(|b| b.starts_with("/plan") && b.contains("@bob")),
        "{bodies:?}"
    );
}

#[test]
fn a_plan_before_the_gate_is_not_kept() {
    // Blocked by a dependency: `/plan` now is rejected, and after unblocking the gate waits.
    let dep = BlockingDependency {
        key: "github:enzarb/enzarb-ui#3".into(),
        label: "#3".into(),
        state: "Implementing".into(),
        ..Default::default()
    };
    let blocked = Ctx {
        blocking_dependencies: vec![dep],
        ..gate_ctx(Permission::Write)
    };
    let t = transition(
        &blocked,
        &WorkflowState::New(WorkflowKind::Issue),
        &opened("e1"),
    );
    graph_in(&t, GraphPhase::Blocked);
    let s = t.next;
    rejected(
        &transition(&blocked, &s, &comment_by("c", "bob", 1, "/plan")),
        &s,
    );
    let deps = EngineEvent {
        id: "d".into(),
        payload: EventPayload::DependenciesChanged,
    };
    let t = transition(&gate_ctx(Permission::Read), &s, &deps);
    let g = graph_in(&t, GraphPhase::Waiting);
    assert_eq!(g.data.graph.node, "gate");
    // And while planning runs, a `/plan` is not waited for.
    let t = transition(
        &gate_ctx(Permission::Write),
        &t.next,
        &comment_by("c2", "bob", 2, "/plan"),
    );
    planning_started(&t);
    let s = t.next;
    rejected(
        &transition(
            &gate_ctx(Permission::Write),
            &s,
            &comment_by("c3", "bob", 3, "/plan"),
        ),
        &s,
    );
}
