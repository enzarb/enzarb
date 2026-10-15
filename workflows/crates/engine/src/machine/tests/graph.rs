//! Graph workflows (ADR-0020): the kernel's pure transitions.

use super::*;
use crate::machine::graph::{GraphPhase, GraphSource, GraphTask};
use enzarb_proto::GraphResult;

const EXAMPLE: &str = include_str!("../../../../../../docs/workflows/examples/issue.yaml");

/// A graph with a wait on approval and a post_comment action.
const GATED: &str = r#"
version: 1
name: issue
on: [issue_opened]
start: plan
nodes:
  plan: {type: agent, prompt: plan, next: {"decision == 'plan'": post, else: failed}}
  post: {type: action, action: post_comment, next: approve}
  approve: {type: wait, events: [approval, comment], next: {"event == 'approval'": implement, else: plan}}
  implement: {type: agent, prompt: "Implement {title} now", next: {pushed: open_pr, else: failed}}
  open_pr: {type: action, action: open_change_request, next: done}
  failed: {type: end, status: failed}
  done: {type: end, status: done}
"#;

fn source(yaml: &str) -> GraphSource {
    GraphSource {
        name: "issue".into(),
        path: ".enzarb/workflows/issue.yaml".into(),
        sha: "c0ffee".into(),
        source: yaml.into(),
    }
}

fn gctx(perm: Permission, yaml: Option<&str>) -> Ctx {
    Ctx {
        graph: yaml.map(source),
        ..ctx(perm)
    }
}

#[track_caller]
fn assert_graph(t: &Transition, phase: GraphPhase) -> GraphTask {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::Graph(g) => {
            assert_eq!(g.phase, phase, "note: {}", t.note);
            g.clone()
        }
        other => panic!("not a graph state: {other:?}"),
    }
}

fn start(c: &Ctx) -> Transition {
    transition(c, &WorkflowState::New(WorkflowKind::Issue), &opened("e1"))
}

/// A `RunFinished` with a graph result stopped at `at`.
fn segment_done(id: &str, at: &str, summary: &str, outputs: Value) -> EngineEvent {
    let fin = RunFinished_ {
        summary: summary.into(),
        graph: GraphResult {
            stopped_at: at.into(),
            outputs: serde_json::from_value(outputs).unwrap(),
            // The sandbox returns the visits so far; every test segment ran its start once.
            visits: [("plan".to_string(), 1), ("prepare".to_string(), 1)].into(),
        },
    };
    ok(id, JobKind::AgentSegment, fin.json())
}

struct RunFinished_ {
    summary: String,
    graph: GraphResult,
}

impl RunFinished_ {
    fn json(self) -> Value {
        serde_json::to_value(enzarb_proto::RunFinished {
            run_id: "job-1".into(),
            outcome: enzarb_proto::RunOutcome::Succeeded,
            exit_code: Some(0),
            summary: self.summary,
            branch: "enzarb/issue-42".into(),
            head_sha: "sha2".into(),
            pushed: true,
            usage: None,
            graph: Some(self.graph),
        })
        .unwrap()
    }
}

const DONE_SUMMARY: &str = "Did it.\n\n```json\n{\"decision\":\"done\",\"body\":\"Adds dark mode.\",\"title\":\"feat(ui): dark mode\"}\n```";

#[test]
fn without_a_definition_the_compiled_machine_runs() {
    let t = start(&gctx(Permission::Read, None));
    assert_issue(&t, IssuePhase::Triaging);
    assert_eq!(t.next.workflow_name(), "issue");
}

#[test]
fn a_definition_starts_a_graph_task_with_a_segment() {
    let t = start(&gctx(Permission::Read, Some(EXAMPLE)));
    let g = assert_graph(&t, GraphPhase::Running);
    assert_eq!(t.next.workflow_name(), "graph:issue");
    assert_eq!(t.next.state_name(), "Running");
    assert_eq!(g.data.graph.sha, "c0ffee");
    assert_eq!(g.data.graph.node, "prepare");
    assert_eq!(g.data.graph.visits["prepare"], 1);
    assert_eq!(g.data.waiting_on, Some(JobKind::AgentSegment));
    assert_eq!(
        kinds(&t),
        [
            JobKind::SetProgress,
            JobKind::SetProgress,
            JobKind::AgentSegment
        ]
    );
    let (_, key, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(key, "github:enzarb/enzarb-ui#42:e1:agent_segment");
    assert_eq!(input["branch"], "enzarb/issue-42");
    assert_eq!(input["push"], true);
    assert_eq!(input["graph"]["start"], "prepare");
    assert_eq!(input["graph"]["definition"], EXAMPLE);
    let implement = input["graph"]["prompts"]["implement"].as_str().unwrap();
    assert!(implement.contains("Add dark mode"), "{implement}");
    // state_data pins the definition.
    let sd = t.next.state_data();
    assert_eq!(sd["graph"]["source"], EXAMPLE);
    let back = WorkflowState::from_row("graph:issue", 1, &t.next.state_name(), &sd).unwrap();
    assert_eq!(back, t.next);
}

#[test]
fn inline_prompts_are_rendered() {
    let t = start(&gctx(Permission::Read, Some(GATED)));
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(
        input["graph"]["prompts"]["implement"],
        "Implement Add dark mode now"
    );
}

#[test]
fn rejected_definitions_fall_back_with_a_comment() {
    let cases: &[(&str, &str)] = &[
        (
            "version: 1\nname: issue\non: [issue_opened]\nstart: x\nnodes: {}",
            "start node `x` does not exist",
        ),
        ("{{{", "does not parse"),
        (
            "version: 1\nname: issue\non: [issue_opened]\nstart: w\nnodes:\n  w: {type: wait, events: [merge_requested], next: m}\n  m: {type: action, action: merge_change_request, next: e}\n  e: {type: end, status: done}\n",
            "action `merge_change_request` is not supported yet",
        ),
        (
            "version: 1\nname: issue\non: [issue_opened]\nstart: w\nnodes:\n  w: {type: wait, events: [ci], next: e}\n  e: {type: end, status: done}\n",
            "waiting on `ci` is not supported yet",
        ),
    ];
    for (yaml, want) in cases {
        let t = start(&gctx(Permission::Read, Some(yaml)));
        assert_issue(&t, IssuePhase::Triaging);
        let bodies = comment_bodies(&t);
        assert_eq!(bodies.len(), 1, "{want}");
        assert!(bodies[0].contains(want), "{want}: {}", bodies[0]);
        assert!(bodies[0].contains(".enzarb/workflows/issue.yaml"));
        assert!(jobs(&t).iter().any(|j| j.2.ends_with(":graph_invalid")));
    }
}

#[test]
fn a_definition_for_other_triggers_falls_back_silently() {
    let yaml = EXAMPLE.replace("on: [issue_opened]", "on: [comment_added]");
    let t = start(&gctx(Permission::Read, Some(&yaml)));
    assert_issue(&t, IssuePhase::Triaging);
    assert!(comment_bodies(&t).is_empty());
}

#[test]
fn bot_opened_issues_do_not_start_a_graph() {
    let ev = conn(
        "e1",
        EventType::WorkItemOpened,
        issue_item(),
        Actor {
            login: "enzarb[bot]".into(),
            id: 1,
            is_bot: true,
        },
        None,
    );
    let s = WorkflowState::New(WorkflowKind::Issue);
    let t = transition(&gctx(Permission::Read, Some(EXAMPLE)), &s, &ev);
    rejected(&t, &s);
}

fn running(yaml: &str) -> WorkflowState {
    start(&gctx(Permission::Read, Some(yaml))).next
}

#[test]
fn segment_result_moves_to_the_action_and_opens_the_change_request() {
    let c = gctx(Permission::Read, Some(EXAMPLE));
    let s = running(EXAMPLE);
    let outputs = json!({"prepare": {"exit_code": 0}, "implement": {"pushed": true}});
    let t = transition(
        &c,
        &s,
        &segment_done("e2", "open_pr", DONE_SUMMARY, outputs),
    );
    let g = assert_graph(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "open_pr");
    assert_eq!(g.data.waiting_on, Some(JobKind::OpenChangeRequest));
    assert_eq!(g.data.branch, "enzarb/issue-42");
    let (_, _, input) = job_of(&t, JobKind::OpenChangeRequest);
    assert_eq!(input["spec"]["title"], "feat(ui): dark mode");
    assert_eq!(input["spec"]["body"], "Adds dark mode.\n\nCloses #42");
    assert_eq!(input["spec"]["head_ref"], "enzarb/issue-42");
    assert_eq!(input["spec"]["base_ref"], "main");

    // The change request opened: the graph ends done and hands the PR over.
    let t2 = transition(
        &c,
        &t.next,
        &ok("e3", JobKind::OpenChangeRequest, pr_opened_output()),
    );
    let g = assert_graph(&t2, GraphPhase::Done);
    assert_eq!(g.data.change_request, Some(43));
    assert_eq!(g.data.graph.outputs["open_pr"]["number"], 43);
    assert_eq!(progress_values(&t2), ["Done"]);
    assert!(has(&t2, |e| matches!(
        e,
        Effect::StartWorkflow {
            workflow: WorkflowKind::ChangeRequest,
            ..
        }
    )));
    assert!(t2.next.is_terminal());
    // Finished: everything else is rejected.
    rejected(
        &transition(&c, &t2.next, &comment_by("e4", "bob", 5, "hi")),
        &t2.next,
    );
}

#[test]
fn segment_stopping_at_a_failed_end_fails_the_task() {
    let c = gctx(Permission::Read, Some(EXAMPLE));
    let s = running(EXAMPLE);
    let t = transition(&c, &s, &segment_done("e2", "failed", "no", json!({})));
    let g = assert_graph(&t, GraphPhase::Failed);
    assert!(g.data.error.contains("`failed`"), "{}", g.data.error);
    assert_eq!(progress_values(&t), ["Failed"]);
    assert_eq!(comment_bodies(&t).len(), 1);
}

#[test]
fn broken_segment_results_fail_the_task() {
    let c = gctx(Permission::Read, Some(EXAMPLE));
    let s = running(EXAMPLE);
    let cases: &[(EngineEvent, &str)] = &[
        (
            ok(
                "e2",
                JobKind::AgentSegment,
                json!({"summary": "x", "branch": "b", "head_sha": "", "pushed": false, "run_id": "r", "outcome": "succeeded"}),
            ),
            "no graph result",
        ),
        (
            segment_done("e2", "implement", "", json!({})),
            "sandbox node `implement`",
        ),
        (
            segment_done("e2", "ghost", "", json!({})),
            "unknown node `ghost`",
        ),
        (failed("e2", JobKind::AgentSegment), "boom"),
    ];
    for (ev, want) in cases {
        let t = transition(&c, &s, ev);
        let g = assert_graph(&t, GraphPhase::Failed);
        assert!(g.data.error.contains(want), "{want}: {}", g.data.error);
    }
}

#[test]
fn other_jobs_are_acknowledged_without_moving() {
    let c = gctx(Permission::Read, Some(EXAMPLE));
    let s = running(EXAMPLE);
    let t = transition(&c, &s, &ok("e2", JobKind::SetProgress, json!({})));
    assert!(t.accepted);
    assert_eq!(t.next, s);
    assert!(t.effects.is_empty());
    let t = transition(
        &c,
        &s,
        &ok("e2", JobKind::OpenChangeRequest, pr_opened_output()),
    );
    assert_eq!(t.next, s);
}

#[test]
fn waits_for_an_approval_with_write_access() {
    let read = gctx(Permission::Read, Some(GATED));
    let write = gctx(Permission::Write, Some(GATED));
    let plan = "Plan.\n\n```json\n{\"decision\":\"plan\",\"body\":\"1. do it\"}\n```";
    let t = transition(
        &read,
        &running(GATED),
        &segment_done("e2", "post", plan, json!({"plan": {"decision": "plan"}})),
    );
    assert_graph(&t, GraphPhase::Running);
    assert_eq!(comment_bodies(&t), ["1. do it"]);
    let t = transition(
        &read,
        &t.next,
        &ok("e3", JobKind::PostComment, json!({"id": 1})),
    );
    let waiting = assert_graph(&t, GraphPhase::Waiting);
    assert_eq!(waiting.data.graph.node, "approve");
    assert!(t.effects.is_empty());
    let s = t.next;

    // Approval needs write access; bots are ignored.
    rejected(&transition(&read, &s, &approval_comment("e4", "bob")), &s);
    let bot = conn(
        "e4",
        EventType::ApprovalGranted,
        issue_item(),
        Actor {
            login: "enzarb[bot]".into(),
            id: 1,
            is_bot: true,
        },
        None,
    );
    rejected(&transition(&write, &s, &bot), &s);
    // Unrelated events are rejected.
    rejected(&transition(&write, &s, &timer("e4", None)), &s);
    rejected(
        &transition(&write, &s, &ui("e4", "alice", UiActionKind::Retry)),
        &s,
    );

    let t = transition(&write, &s, &approval_label("e5", "alice"));
    let g = assert_graph(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "implement");
    assert_eq!(g.data.graph.outputs["approve"]["event"], "approval");
    assert_eq!(g.data.segments, 2);
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["graph"]["start"], "implement");
    assert_eq!(input["graph"]["outputs"]["approve"]["author"], "alice");

    // A comment instead loops back to planning.
    let t = transition(&read, &s, &comment_by("e6", "bob", 7, "please also X"));
    let g = assert_graph(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "plan");
    assert_eq!(g.data.graph.visits["plan"], 2);
    assert_eq!(g.data.graph.outputs["approve"]["body"], "please also X");
    assert_eq!(g.data.graph.outputs["approve"]["can_write"], false);
}

#[test]
fn wait_ignores_events_it_does_not_list() {
    let yaml = GATED.replace("events: [approval, comment]", "events: [approval]");
    let read = gctx(Permission::Read, Some(&yaml));
    let plan = "```json\n{\"decision\":\"plan\",\"body\":\"p\"}\n```";
    let t = transition(
        &read,
        &running(&yaml),
        &segment_done("e2", "post", plan, json!({"plan": {"decision": "plan"}})),
    );
    let t = transition(&read, &t.next, &ok("e3", JobKind::PostComment, json!({})));
    let s = t.next;
    rejected(&transition(&read, &s, &comment_by("e4", "bob", 7, "x")), &s);
}

#[test]
fn cancel_and_close() {
    let s = running(EXAMPLE);
    let read = gctx(Permission::Read, Some(EXAMPLE));
    let write = gctx(Permission::Write, Some(EXAMPLE));
    rejected(
        &transition(&read, &s, &ui("c", "bob", UiActionKind::Cancel)),
        &s,
    );
    let t = transition(&write, &s, &ui("c", "alice", UiActionKind::Cancel));
    assert_graph(&t, GraphPhase::Cancelled);
    assert!(cancels(&t));
    assert_eq!(comment_bodies(&t), ["enzarb: cancelled by @alice."]);
    let t = transition(&read, &s, &closed("c", issue_item(), false));
    assert_graph(&t, GraphPhase::Cancelled);
    assert!(cancels(&t));
    assert!(comment_bodies(&t).is_empty());
}
