//! Graph workflows: epics as graph nodes, and the dependency guard (`Blocked`).

use super::*;
use crate::machine::graph::{
    BlockingDependency, DependencyProgress, GraphPhase, GraphSource, GraphTask, dependency_progress,
};
use enzarb_proto::GraphResult;

const EPIC: &str = include_str!("../../../../../../docs/workflows/examples/epic-issue.yaml");

/// enzarb's own issue graph (plan, approval wait, prepare, implement).
const ADOPT: &str = r#"
version: 1
name: issue
on: [issue_opened]
start: plan
nodes:
  plan:      {type: agent, prompt: plan, next: post_plan}
  post_plan: {type: action, action: post_comment, next: approval}
  approval:  {type: wait, events: [approval, comment], next: {'event == "approval"': prepare, else: plan}}
  prepare:   {type: command, run: "mise run prepare", optional: true, next: implement}
  implement: {type: agent, prompt: implement, next: {pushed: open_pr, else: failed}}
  open_pr:   {type: action, action: open_change_request, next: done}
  failed:    {type: end, status: failed}
  done:      {type: end, status: done}
"#;

fn breakdown() -> String {
    "## Sub-issues\n\n```enzarb-subissues\n[\
     {\"key\":\"api\",\"title\":\"Add the API\",\"body\":\"API part\",\"depends_on\":[]},\
     {\"key\":\"ui\",\"title\":\"Show it\",\"body\":\"UI part\",\"depends_on\":[\"api\"]}]\n```\n"
        .into()
}

fn plan_summary(body: &str) -> String {
    let block = json!({"decision": "plan", "body": body});
    format!("Planned.\n\n```json\n{block}\n```")
}

fn gsource(yaml: &str) -> GraphSource {
    GraphSource {
        name: "issue".into(),
        path: ".enzarb/workflows/issue.yaml".into(),
        sha: "c0ffee".into(),
        source: yaml.into(),
        prompts: Default::default(),
    }
}

fn gc(perm: Permission, yaml: &str) -> Ctx {
    Ctx {
        graph: Some(gsource(yaml)),
        ..ctx(perm)
    }
}

#[track_caller]
fn graph_of(t: &Transition, phase: GraphPhase) -> GraphTask {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::Graph(g) => {
            assert_eq!(g.phase, phase, "note: {}", t.note);
            g.clone()
        }
        other => panic!("not a graph state: {other:?}"),
    }
}

fn seg(id: &str, at: &str, summary: &str, outputs: Value) -> EngineEvent {
    let fin = enzarb_proto::RunFinished {
        run_id: "job-1".into(),
        outcome: enzarb_proto::RunOutcome::Succeeded,
        exit_code: Some(0),
        summary: summary.into(),
        branch: "enzarb/issue-42".into(),
        head_sha: "sha2".into(),
        pushed: false,
        usage: None,
        graph: Some(GraphResult {
            stopped_at: at.into(),
            outputs: serde_json::from_value(outputs).unwrap(),
            visits: Default::default(),
        }),
    };
    ok(
        id,
        JobKind::AgentSegment,
        serde_json::to_value(fin).unwrap(),
    )
}

fn deps_changed(id: &str) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::DependenciesChanged,
    }
}

fn open_dep(n: u64, problem: &str) -> OpenDependency {
    OpenDependency {
        key: format!("github:enzarb/enzarb-ui#{n}"),
        label: format!("#{n}"),
        url: String::new(),
        problem: problem.into(),
    }
}

fn blocker(n: u64, state: &str, pr: &str, failed: bool) -> BlockingDependency {
    BlockingDependency {
        key: format!("github:enzarb/enzarb-ui#{n}"),
        label: format!("#{n}"),
        title: format!("Dep {n}"),
        state: state.into(),
        url: format!("https://github.com/enzarb/enzarb-ui/issues/{n}"),
        pr_url: pr.into(),
        failed,
    }
}

fn start(c: &Ctx) -> Transition {
    transition(c, &WorkflowState::New(WorkflowKind::Issue), &opened("e1"))
}

/// An epic planned and posted, waiting at `approval`.
fn epic_at_approval(c: &Ctx) -> WorkflowState {
    let t = start(c);
    let outputs = json!({"triage": {"decision": "epic"}, "plan_epic": {"decision": "plan"}});
    let t = transition(
        c,
        &t.next,
        &seg("e2", "post_plan", &plan_summary(&breakdown()), outputs),
    );
    let g = graph_of(&t, GraphPhase::Running);
    assert!(g.data.epic, "a triage `epic` decision makes it an epic");
    assert!(comment_bodies(&t)[0].contains("enzarb-subissues"));
    let t = transition(
        c,
        &t.next,
        &ok("e3", JobKind::PostComment, json!({"id": 1})),
    );
    graph_of(&t, GraphPhase::Waiting);
    t.next
}

fn created_children() -> EngineEvent {
    let child = |n: u64, key: &str| json!({"key": key, "item": format!("github:enzarb/enzarb-ui#{n}"), "title": key});
    ok(
        "e5",
        JobKind::CreateSubIssues,
        json!({"children": [child(50, "api"), child(51, "ui")]}),
    )
}

#[test]
fn an_approved_epic_creates_its_sub_issues_and_waits_for_them() {
    let c = gc(Permission::Write, EPIC);
    let s = epic_at_approval(&c);
    // The item facts say it is an epic.
    let WorkflowState::Graph(g) = &s else {
        panic!()
    };
    assert_eq!(g.data.graph.outputs["_item"]["epic"], true);

    let t = transition(&c, &s, &approval_comment("e4", "alice"));
    let g = graph_of(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "create");
    assert!(g.data.accepts_children);
    assert_eq!(g.data.sub_issues.len(), 2);
    let (_, _, input) = job_of(&t, JobKind::CreateSubIssues);
    assert_eq!(input["depth"], 1);
    assert_eq!(input["sub_issues"][0]["key"], "api");

    let t = transition(&c, &t.next, &created_children());
    let g = graph_of(&t, GraphPhase::Waiting);
    assert_eq!(g.data.graph.node, "children");
    assert!(g.data.accepts_children);
    assert_eq!(g.data.graph.outputs["create"]["created"][1]["number"], 51);

    // One still open: stored, still waiting.
    let with = |deps: Vec<OpenDependency>| Ctx {
        open_dependencies: deps,
        ..c.clone()
    };
    let t2 = transition(&with(vec![open_dep(51, "")]), &t.next, &deps_changed("e6"));
    let g = graph_of(&t2, GraphPhase::Waiting);
    assert_eq!(g.data.open_dependencies.len(), 1);
    assert!(t2.effects.is_empty());
    // All done: the epic is done.
    let t3 = transition(&with(vec![]), &t2.next, &deps_changed("e7"));
    let g = graph_of(&t3, GraphPhase::Done);
    assert_eq!(
        g.data.graph.outputs["children"]["completed"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!g.data.accepts_children);
    // One closed without completing: tell people and wait for a human.
    let t4 = transition(
        &with(vec![open_dep(51, "closed")]),
        &t2.next,
        &deps_changed("e8"),
    );
    let g = graph_of(&t4, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "tell_stuck");
    assert_eq!(
        g.data.graph.outputs["children"]["failed"][0],
        "github:enzarb/enzarb-ui#51"
    );
    let t5 = transition(&c, &t4.next, &ok("e9", JobKind::PostComment, json!({})));
    graph_of(&t5, GraphPhase::Waiting);
    let t6 = transition(&c, &t5.next, &approval_comment("e10", "alice"));
    graph_of(&t6, GraphPhase::Done);
}

#[test]
fn an_unusable_breakdown_fails_the_node() {
    let c = gc(Permission::Write, EPIC);
    let t = start(&c);
    let outputs = json!({"triage": {"decision": "epic"}});
    let t = transition(
        &c,
        &t.next,
        &seg("e2", "post_plan", &plan_summary("no block"), outputs),
    );
    let t = transition(&c, &t.next, &ok("e3", JobKind::PostComment, json!({})));
    let t = transition(&c, &t.next, &approval_comment("e4", "alice"));
    let g = graph_of(&t, GraphPhase::Failed);
    assert!(
        g.data.error.contains("unusable sub-issue breakdown"),
        "{}",
        g.data.error
    );
    assert!(kinds(&t).iter().all(|k| *k != JobKind::CreateSubIssues));
}

#[test]
fn epic_requests_set_the_item_fact() {
    let c = gc(Permission::Write, EPIC);
    let plain = {
        let t = start(&c);
        let t = transition(
            &c,
            &t.next,
            &seg(
                "e2",
                "post_plan",
                &plan_summary("p"),
                json!({"triage": {"decision": "ready"}}),
            ),
        );
        transition(&c, &t.next, &ok("e3", JobKind::PostComment, json!({}))).next
    };
    // A writer's `/epic` at the approval wait re-plans as a breakdown.
    let t = transition(&c, &plain, &comment_by("e4", "alice", 9, "/epic"));
    let g = graph_of(&t, GraphPhase::Running);
    assert!(g.data.epic);
    assert_eq!(g.data.graph.node, "plan_epic");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert!(
        input["graph"]["prompts"]["plan_epic"]
            .as_str()
            .unwrap()
            .contains("enzarb-subissues")
    );
    // The label from a writer sets it without moving; from a reader it is rejected.
    let label = {
        let mut e = conn(
            "e5",
            EventType::WorkItemLabeled,
            issue_item(),
            actor("alice"),
            None,
        );
        if let EventPayload::Connector(x) = &mut e.payload {
            x.data.label = Some("epic".into());
        }
        e
    };
    let t = transition(&c, &plain, &label);
    let g = graph_of(&t, GraphPhase::Waiting);
    assert!(g.data.epic);
    rejected(
        &transition(&gc(Permission::Read, EPIC), &plain, &label),
        &plain,
    );
    // At the depth limit an issue cannot be an epic.
    let mut deep = plain.clone();
    if let WorkflowState::Graph(g) = &mut deep {
        g.data.epic_depth = 1;
    }
    rejected(&transition(&c, &deep, &label), &deep);
}

#[test]
fn sub_issue_intake_accepts_only_enzarbs_own_children() {
    let parent = WorkItemKey::new(repo(), 7);
    let marked = |body: &str| {
        let mut item = issue_item();
        item.body = format!(
            "{body}\n{}",
            enzarb_connector::epic::epic_marker(&parent, "api")
        );
        conn(
            "e1",
            EventType::WorkItemOpened,
            item,
            Actor {
                login: "enzarb[bot]".into(),
                id: 1,
                is_bot: true,
            },
            None,
        )
    };
    let new = WorkflowState::New(WorkflowKind::Issue);
    let with_parent = Ctx {
        epic_parent: Some(EpicParent {
            key: parent.clone(),
            depth: 0,
            sub_keys: vec!["api".into()],
        }),
        ..gc(Permission::None, EPIC)
    };
    let t = transition(&with_parent, &new, &marked("x"));
    let g = graph_of(&t, GraphPhase::Running);
    assert_eq!(g.data.epic_depth, 1);
    assert_eq!(
        g.data.epic_parent.as_deref(),
        Some("github:enzarb/enzarb-ui#7")
    );
    assert_eq!(g.data.graph.outputs["_item"]["epic_allowed"], false);
    // Unknown parent, or a key not in its plan.
    rejected(
        &transition(&gc(Permission::None, EPIC), &new, &marked("x")),
        &new,
    );
    let other_key = Ctx {
        epic_parent: Some(EpicParent {
            key: parent.clone(),
            depth: 0,
            sub_keys: vec!["ui".into()],
        }),
        ..gc(Permission::None, EPIC)
    };
    rejected(&transition(&other_key, &new, &marked("x")), &new);
}

#[test]
fn dependency_progress_table() {
    use DependencyProgress::*;
    type Case<'a> = (
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        DependencyProgress,
    );
    let cases: &[Case] = &[
        (None, None, None, Open),
        (Some("issue"), Some("Done"), Some("Merged"), Completed),
        (Some("graph:issue"), Some("Done"), Some("Open"), Open),
        (Some("issue"), Some("Implementing"), None, Open),
        (Some("issue"), Some("Done"), None, Completed),
        (Some("issue"), Some("Cancelled"), Some("Merged"), Completed),
        (Some("issue"), Some("Done"), Some("Closed"), Failed),
        (
            Some("change_request"),
            Some("Merged"),
            Some("Merged"),
            Completed,
        ),
        (
            Some("change_request"),
            Some("ReadyToMerge"),
            Some("ReadyToMerge"),
            Open,
        ),
        (Some("issue"), Some("Failed"), None, Failed),
        (Some("issue"), Some("Cancelled"), None, Failed),
        (Some("graph:issue"), Some("Blocked"), None, Open),
    ];
    for (w, s, cr, want) in cases {
        assert_eq!(
            dependency_progress(*w, *s, *cr),
            *want,
            "{w:?} {s:?} {cr:?}"
        );
    }
}

fn blocked_ctx(perm: Permission, yaml: &str, deps: Vec<BlockingDependency>) -> Ctx {
    Ctx {
        blocking_dependencies: deps,
        ..gc(perm, yaml)
    }
}

#[test]
fn a_task_with_open_dependencies_is_blocked_before_start() {
    let pr = "https://github.com/enzarb/enzarb-ui/pull/9";
    let c = blocked_ctx(
        Permission::Read,
        ADOPT,
        vec![
            blocker(3, "Done", pr, false),
            blocker(4, "Implementing", "", false),
        ],
    );
    let t = start(&c);
    let g = graph_of(&t, GraphPhase::Blocked);
    assert_eq!(t.next.state_name(), "Blocked");
    assert!(kinds(&t).iter().all(|k| *k != JobKind::AgentSegment));
    assert_eq!(g.data.blocked_resume, None);
    let wait = g.data.wait.clone().unwrap();
    assert_eq!(wait["waiting_on"], "dependency");
    assert_eq!(wait["reason"], "dependencies_open");
    assert_eq!(
        wait["text"],
        format!("Blocked by #3 (Dep 3, Done, PR {pr} not merged), #4 (Dep 4, Implementing)")
    );
    assert_eq!(t.next.state_data()["wait"]["reason"], "dependencies_open");

    // Other events wait; a dependency change that leaves it blocked keeps it there.
    let s = t.next;
    rejected(&transition(&c, &s, &comment_by("x", "bob", 1, "hi")), &s);
    let still = blocked_ctx(
        Permission::Read,
        ADOPT,
        vec![blocker(4, "Implementing", "", false)],
    );
    let t = transition(&still, &s, &deps_changed("d1"));
    let g = graph_of(&t, GraphPhase::Blocked);
    assert_eq!(g.data.blocked_by.len(), 1);
    // All completed: it enters `start` by itself.
    let t = transition(&gc(Permission::Read, ADOPT), &t.next, &deps_changed("d2"));
    let g = graph_of(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "plan");
    assert_eq!(g.data.wait.as_ref().unwrap()["waiting_on"], "enzarb");
    assert_eq!(t.note, "dependencies completed; resumed at plan");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["graph"]["start"], "plan");
}

#[test]
fn a_failed_dependency_needs_a_human_and_retry_does_not_override() {
    let failed = blocked_ctx(
        Permission::Write,
        ADOPT,
        vec![blocker(3, "Cancelled", "", true)],
    );
    let t = start(&failed);
    let g = graph_of(&t, GraphPhase::Blocked);
    let wait = g.data.wait.unwrap();
    assert_eq!(
        (wait["waiting_on"].as_str(), wait["reason"].as_str()),
        (Some("human"), Some("dependency_failed"))
    );
    assert!(
        wait["text"]
            .as_str()
            .unwrap()
            .contains("cannot complete by itself")
    );
    let s = t.next;
    // A write-access retry re-evaluates: still blocked.
    let t = transition(&failed, &s, &ui("r", "alice", UiActionKind::Retry));
    graph_of(&t, GraphPhase::Blocked);
    rejected(
        &transition(
            &blocked_ctx(Permission::Read, ADOPT, vec![]),
            &s,
            &ui("r", "bob", UiActionKind::Retry),
        ),
        &s,
    );
    // Once the dependency is replaced, a retry starts it.
    let t = transition(
        &gc(Permission::Write, ADOPT),
        &s,
        &ui("r", "alice", UiActionKind::Retry),
    );
    graph_of(&t, GraphPhase::Running);
    // Cancel and close still work while blocked.
    let t = transition(&failed, &s, &ui("c", "alice", UiActionKind::Cancel));
    graph_of(&t, GraphPhase::Cancelled);
    let t = transition(&failed, &s, &closed("c", issue_item(), false));
    graph_of(&t, GraphPhase::Cancelled);
}

#[test]
fn a_dependency_added_later_blocks_before_implement_only() {
    let c = gc(Permission::Write, ADOPT);
    let t = start(&c);
    let t = transition(
        &c,
        &t.next,
        &seg(
            "e2",
            "post_plan",
            &plan_summary("PLAN"),
            json!({"plan": {"decision": "plan"}}),
        ),
    );
    // A new "Blocked by" while planning does not interrupt the flow.
    let late = blocked_ctx(
        Permission::Write,
        ADOPT,
        vec![blocker(5, "Triaging", "", false)],
    );
    let t = transition(&late, &t.next, &ok("e3", JobKind::PostComment, json!({})));
    let at_wait = graph_of(&t, GraphPhase::Waiting);
    assert_eq!(at_wait.data.graph.node, "approval");
    // The approval would start prepare → implement: blocked there, resumed there.
    let t = transition(&late, &t.next, &approval_comment("e4", "alice"));
    let g = graph_of(&t, GraphPhase::Blocked);
    assert_eq!(g.data.blocked_resume.as_deref(), Some("prepare"));
    assert!(kinds(&t).iter().all(|k| *k != JobKind::AgentSegment));
    let t = transition(&c, &t.next, &deps_changed("d1"));
    let g = graph_of(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "prepare");
    assert_eq!(g.data.graph.visits["prepare"], 1);
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["graph"]["start"], "prepare");
}
