//! Change request graphs (ADR-0020): the kernel's waits, actions and facts for
//! `graph:change_request`, against the example definition.

use super::*;
use crate::machine::graph::{GraphPhase, GraphSource, GraphTask};
use enzarb_proto::GraphResult;

const EXAMPLE: &str = include_str!("../../../../../../docs/workflows/examples/change_request.yaml");
const ISSUE_EXAMPLE: &str = include_str!("../../../../../../docs/workflows/examples/issue.yaml");
const BOT: &str = "enzarb[bot]";

fn source(name: &str, yaml: &str) -> GraphSource {
    GraphSource {
        name: name.into(),
        path: format!(".enzarb/workflows/{name}.yaml"),
        sha: "c0ffee".into(),
        source: yaml.into(),
        prompts: Default::default(),
    }
}

fn cctx(perm: Permission) -> Ctx {
    Ctx {
        graph: Some(source("change_request", EXAMPLE)),
        ..ctx_for(43, perm)
    }
}

/// The change request, opened by enzarb (so conflicts are resolved).
fn pr(sha: &str) -> ItemRef {
    ItemRef {
        author: BOT.into(),
        head_ref: Some("enzarb/issue-42".into()),
        ..pr_item(sha)
    }
}

fn pr_opened(id: &str) -> EngineEvent {
    let mut bot = actor(BOT);
    bot.is_bot = true;
    conn(id, EventType::ChangeRequestOpened, pr("sha1"), bot, None)
}

#[track_caller]
fn task(t: &Transition, phase: GraphPhase) -> GraphTask {
    assert!(t.accepted, "not accepted: {}", t.note);
    match &t.next {
        WorkflowState::Graph(g) => {
            assert_eq!(g.phase, phase, "note: {} at {}", t.note, g.data.graph.node);
            g.clone()
        }
        other => panic!("not a graph state: {other:?}"),
    }
}

fn node(t: &Transition) -> String {
    match &t.next {
        WorkflowState::Graph(g) => g.data.graph.node.clone(),
        _ => String::new(),
    }
}

fn wait_of(t: &Transition) -> (String, String) {
    let sd = t.next.state_data();
    (
        sd["wait"]["waiting_on"].as_str().unwrap_or("").into(),
        sd["wait"]["reason"].as_str().unwrap_or("").into(),
    )
}

fn review_summary(decision: &str, body: &str) -> String {
    let checks = json!([{"name": "cargo test", "status": "passed"}]);
    let block = json!({"decision": decision, "body": body, "checks": checks});
    format!("Reviewed.\n\n```json\n{block}\n```")
}

/// A segment from the current node that stopped at `at`, with `outputs` for its sandbox nodes.
fn segment(id: &str, at: &str, summary: &str, head: &str, outputs: Value) -> EngineEvent {
    let fin = enzarb_proto::RunFinished {
        run_id: "job-1".into(),
        outcome: enzarb_proto::RunOutcome::Succeeded,
        exit_code: Some(0),
        summary: summary.into(),
        branch: "enzarb/issue-42".into(),
        head_sha: head.into(),
        pushed: outputs.to_string().contains("\"pushed\":true"),
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

fn readiness_ok(id: &str, sha: &str, ci: &str, mergeable: Value) -> EngineEvent {
    ok(
        id,
        JobKind::CheckMergeReadiness,
        json!({"head_sha": sha, "ci": ci, "mergeable": mergeable, "mergeable_state": "clean"}),
    )
}

fn ci_event(id: &str, sha: &str, state: enzarb_connector::CiState, url: &str) -> EngineEvent {
    let mut e = conn(id, EventType::CiStatusChanged, pr(sha), actor("ci"), None);
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.ci = Some(enzarb_connector::CiInfo {
            head_sha: sha.into(),
            state,
            context: Some("build".into()),
            url: Some(url.into()).filter(|u: &String| !u.is_empty()),
        });
    }
    e
}

fn merge_click(id: &str, who: &str) -> EngineEvent {
    let mut e = ui(id, who, UiActionKind::Merge);
    if let EventPayload::Ui(u) = &mut e.payload {
        u.data = json!({"delete_branch": true});
    }
    e
}

fn pushed_by(id: &str, sha: &str, who: Actor) -> EngineEvent {
    conn(id, EventType::ChangeRequestUpdated, pr(sha), who, None)
}

fn unmergeable(id: &str, base: &str) -> EngineEvent {
    EngineEvent {
        id: id.into(),
        payload: EventPayload::Unmergeable {
            base_sha: base.into(),
        },
    }
}

fn pr_comment(id: &str, who: &str, text: &str) -> EngineEvent {
    conn(
        id,
        EventType::CommentAdded,
        pr("sha1"),
        actor(who),
        Some((77, text)),
    )
}

/// Opened, reviewed with `decision`, review posted.
fn reviewed(c: &Ctx, decision: &str, body: &str) -> Transition {
    let t = transition(
        c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    let t = transition(
        c,
        &t.next,
        &segment(
            "e2",
            "post_review",
            &review_summary(decision, body),
            "sha1",
            json!({}),
        ),
    );
    task(&t, GraphPhase::Running);
    assert_eq!(node(&t), "post_review");
    transition(
        c,
        &t.next,
        &ok("e3", JobKind::PostComment, json!({"id": 1})),
    )
}

/// Approved, CI green and mergeable: at the merge wait.
fn ready(c: &Ctx) -> Transition {
    let t = reviewed(c, "approve", "LGTM");
    assert_eq!(node(&t), "readiness");
    transition(
        c,
        &t.next,
        &readiness_ok("e4", "sha1", "success", json!(true)),
    )
}

#[test]
fn an_opened_change_request_starts_the_graph_with_a_review() {
    let c = cctx(Permission::Read);
    let t = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    let g = task(&t, GraphPhase::Running);
    assert_eq!(t.next.workflow_name(), "graph:change_request");
    assert_eq!(g.data.graph.node, "review");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    // The review runs on the change request's own branch and pushes nothing.
    assert_eq!(input["base_ref"], "enzarb/issue-42");
    assert_eq!(input["branch"], "enzarb/issue-42");
    assert_eq!(input["push"], false);
    assert_eq!(input["context"]["item_kind"], "change request");
    assert_eq!(input["context"]["head_sha"], "sha1");
    assert!(
        input["graph"]["prompts"]["review"]
            .as_str()
            .unwrap()
            .contains("enzarb/issue-42")
    );
    assert_eq!(wait_of(&t), ("enzarb".into(), "reviewing".into()));
    let sd = t.next.state_data();
    assert_eq!(sd["cr"]["data"]["head_sha"], "sha1");
    assert_eq!(sd["checklist"]["review"]["status"], "pending");
    // Stored and loaded again.
    let back =
        WorkflowState::from_row("graph:change_request", 1, &t.next.state_name(), &sd).unwrap();
    assert_eq!(back, t.next);
}

#[test]
fn without_a_usable_definition_the_compiled_machine_runs() {
    let ev = pr_opened("e1");
    let new = WorkflowState::New(WorkflowKind::ChangeRequest);
    let t = transition(&ctx_for(43, Permission::Read), &new, &ev);
    assert_cr(&t, CrPhase::Reviewing);
    // An issue definition is no change request definition.
    let c = Ctx {
        graph: Some(source("change_request", ISSUE_EXAMPLE)),
        ..ctx_for(43, Permission::Read)
    };
    let t = transition(&c, &new, &ev);
    assert_cr(&t, CrPhase::Reviewing);
    assert!(comment_bodies(&t).is_empty());
    // An issue-only action: compiled, with a comment naming the problem.
    let bad = EXAMPLE.replace(
        "events: [comment, review, pushed, conflict]",
        "events: [comment, children_done]",
    );
    let c = Ctx {
        graph: Some(source("change_request", &bad)),
        ..ctx_for(43, Permission::Read)
    };
    let t = transition(&c, &new, &ev);
    assert_cr(&t, CrPhase::Reviewing);
    let bodies = comment_bodies(&t);
    assert!(
        bodies[0].contains("waiting on `children_done` is not possible for a change request"),
        "{bodies:?}"
    );
}

#[test]
fn a_stacked_change_request_stays_on_the_compiled_machine() {
    let mut item = pr("sha1");
    item.base_ref = Some("enzarb/issue-41".into());
    item.body = "Adds it\n\n<!-- enzarb:stack parent=github:enzarb/enzarb-ui#41 ref=enzarb/issue-41 tip=abc -->".into();
    let mut bot = actor(BOT);
    bot.is_bot = true;
    let ev = conn("e1", EventType::ChangeRequestOpened, item, bot, None);
    let t = transition(
        &cctx(Permission::Read),
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &ev,
    );
    let d = assert_cr(&t, CrPhase::Reviewing);
    assert!(!d.stacked_on.is_empty());
    assert!(t.note.contains("stacked"), "{}", t.note);
}

#[test]
fn approve_green_mergeable_waits_for_a_human_merge_then_merges() {
    let c = cctx(Permission::Read);
    let t = reviewed(&c, "approve", "LGTM");
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.outputs["post_review"]["decision"], "approve");
    let (_, _, input) = job_of(&t, JobKind::CheckMergeReadiness);
    assert_eq!(input["head_sha"], "sha1");

    let t = ready(&c);
    task(&t, GraphPhase::Waiting);
    assert_eq!(node(&t), "ready");
    assert_eq!(wait_of(&t), ("human".into(), "ready_to_merge".into()));
    let sd = t.next.state_data();
    assert_eq!(sd["checklist"]["ci"]["status"], "ok");
    assert_eq!(sd["checklist"]["review"]["status"], "ok");

    // Nothing but a writer's click merges.
    rejected(&transition(&c, &t.next, &merge_click("m0", "bob")), &t.next);
    let open_deps = Ctx {
        open_dependencies: vec![OpenDependency {
            key: "github:enzarb/enzarb-ui#9".into(),
            label: "#9".into(),
            ..Default::default()
        }],
        ..cctx(Permission::Write)
    };
    rejected(
        &transition(&open_deps, &t.next, &merge_click("m1", "carol")),
        &t.next,
    );

    let w = cctx(Permission::Write);
    let m = transition(&w, &t.next, &merge_click("m2", "carol"));
    task(&m, GraphPhase::Running);
    // The stored click passes the gate wait at once.
    assert_eq!(node(&m), "merge");
    let (_, _, input) = job_of(&m, JobKind::MergeChangeRequest);
    assert_eq!(input["expected_head_sha"], "sha1");
    assert_eq!(input["actor"], "carol");
    assert_eq!(input["delete_branch"], "enzarb/issue-42");
    assert_eq!(wait_of(&m), ("github".into(), "merging".into()));

    let done = transition(
        &c,
        &m.next,
        &ok(
            "m3",
            JobKind::MergeChangeRequest,
            json!({"outcome": "merged", "actor": "carol"}),
        ),
    );
    task(&done, GraphPhase::Merged);
    assert_eq!(done.next.state_name(), "Merged");
    assert!(
        comment_bodies(&done)
            .iter()
            .any(|b| b == "enzarb: merged by @carol.")
    );
    // The webhook that follows changes nothing.
    rejected(
        &transition(&c, &done.next, &closed("m4", pr("sha1"), true)),
        &done.next,
    );
}

#[test]
fn a_merge_click_outside_the_merge_wait_is_rejected() {
    let c = cctx(Permission::Write);
    let t = reviewed(&c, "comment", "Looks fine but unverified");
    task(&t, GraphPhase::Waiting);
    rejected(
        &transition(&c, &t.next, &merge_click("m", "carol")),
        &t.next,
    );
}

#[test]
fn must_fix_findings_start_an_automatic_round_and_are_reviewed_again() {
    let c = cctx(Permission::Read);
    let t = reviewed(&c, "request_changes", "Fix the null check");
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "fix");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["push"], true);
    assert_eq!(input["context"]["trigger"], "Fix the null check");
    assert_eq!(g.data.cr.as_ref().unwrap().data.auto_rounds, 1);
    assert_eq!(wait_of(&t), ("enzarb".into(), "enzarb_fixing".into()));

    // The fix pushes; the same segment reviews the new head and stops at post_review.
    let t = transition(
        &c,
        &t.next,
        &segment(
            "e5",
            "post_review",
            &review_summary("approve", "Now good"),
            "sha2",
            json!({"fix": {"ok": true, "decision": "done", "pushed": true}}),
        ),
    );
    let g = task(&t, GraphPhase::Running);
    let d = &g.data.cr.as_ref().unwrap().data;
    assert_eq!(d.head_sha, "sha2");
    assert_eq!(d.reviewed_sha, "sha2");
    assert_eq!(d.last_review, "approve");
}

#[test]
fn a_downgraded_approval_is_a_must_fix_comment() {
    let c = cctx(Permission::Read);
    let t = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    let block = json!({"decision": "approve", "body": "LGTM", "checks": []});
    let summary = format!("Reviewed.\n\n```json\n{block}\n```");
    let t = transition(
        &c,
        &t.next,
        &segment("e2", "post_review", &summary, "sha1", json!({})),
    );
    assert!(comment_bodies(&t)[0].starts_with("### enzarb review: comment"));
    let g = task(&t, GraphPhase::Running);
    let c = g.data.cr.as_ref().unwrap();
    assert_eq!(c.data.last_review, "comment");
    assert_eq!(c.data.unverified_checks[0].name, "no checks reported");
    assert!(c.must_fix >= 1);
    // Once posted, the must-fix findings start an automatic round.
    let t = transition(
        &c_read(),
        &t.next,
        &ok("e3", JobKind::PostComment, json!({"id": 1})),
    );
    assert_eq!(node(&t), "fix");
}

fn c_read() -> Ctx {
    cctx(Permission::Read)
}

#[test]
fn a_ci_failure_at_the_merge_wait_starts_a_fix_round_with_the_failing_checks() {
    let c = cctx(Permission::Read);
    let t = ready(&c);
    let url = "https://github.com/enzarb/enzarb-ui/actions/runs/123/job/456";
    let t = transition(
        &c,
        &t.next,
        &ci_event("c1", "sha1", enzarb_connector::CiState::Failure, url),
    );
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "fix");
    let out = &g.data.graph.outputs["ready"];
    assert_eq!(out["event"], "ci");
    assert_eq!(out["status"], "failure");
    assert_eq!(out["failing"][0]["run_id"], "123");
    assert_eq!(out["failing"][0]["job_id"], "456");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["context"]["ci_fix"], true);
    assert!(
        input["context"]["ci_failure"]
            .as_str()
            .unwrap()
            .contains("run id: 123")
    );
    assert!(
        input["context"]["trigger"]
            .as_str()
            .unwrap()
            .contains("CI failed on sha1")
    );
    // The same failure again does nothing new.
    let again = ci_event("c2", "sha1", enzarb_connector::CiState::Failure, url);
    let t2 = transition(&c, &t.next, &again);
    assert!(jobs(&t2).iter().all(|j| j.0 != JobKind::AgentSegment));
}

fn named_ci(id: &str, name: &str, url: &str) -> EngineEvent {
    let mut e = ci_event(id, "sha1", enzarb_connector::CiState::Failure, url);
    if let EventPayload::Connector(c) = &mut e.payload {
        c.data.ci.as_mut().unwrap().context = Some(name.into());
    }
    e
}

#[test]
fn an_external_only_failure_waits_on_a_human_without_a_round() {
    let c = cctx(Permission::Read);
    let t = ready(&c);
    let t = transition(
        &c,
        &t.next,
        &named_ci("c1", "Vercel", "https://vercel.com/x"),
    );
    let g = task(&t, GraphPhase::Waiting);
    assert_eq!(g.data.graph.node, "ci_failed");
    let out = &g.data.graph.outputs["ready"];
    assert_eq!(out["status"], "failure");
    assert_eq!(out["actionable"], false);
    assert_eq!(out["external"], json!(["Vercel"]));
    assert_eq!(out["failing"], json!([]));
    assert!(jobs(&t).iter().all(|j| j.0 != JobKind::AgentSegment));
    assert_eq!(
        wait_of(&t),
        ("human".to_string(), "ci_failed_external".to_string())
    );
    assert_eq!(
        t.next.state_data()["wait"]["text"],
        "Vercel failed outside GitHub Actions; enzarb can't read it"
    );
    // `ci_fix_due` is false: the readiness path does not start a round either.
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("e8", "sha1", "failure", json!(true)),
    );
    assert_eq!(node(&t), "ci_failed");
    assert!(jobs(&t).iter().all(|j| j.0 != JobKind::AgentSegment));
    // An Actions job failing too: the round lists only it.
    let url = "https://github.com/enzarb/enzarb-ui/actions/runs/7/job/8";
    let t = transition(&c, &t.next, &named_ci("c2", "ci / test", url));
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "fix");
    assert_eq!(g.data.graph.outputs["ci_failed"]["actionable"], true);
}

#[test]
fn a_mixed_failure_lists_only_actions_jobs() {
    let c = cctx(Permission::Read);
    let t = ready(&c);
    let t = transition(
        &c,
        &t.next,
        &named_ci("c1", "Vercel", "https://vercel.com/x"),
    );
    // An Actions job fails as well: the round names only it.
    let url = "https://github.com/enzarb/enzarb-ui/actions/runs/7/job/8";
    let t = transition(&c, &t.next, &named_ci("c2", "ci / test", url));
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "fix");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    let f = input["context"]["ci_failure"].as_str().unwrap();
    assert!(
        f.contains("check: ci / test") && f.contains("run id: 7"),
        "{f}"
    );
    assert!(!f.contains("Vercel"), "{f}");
    assert!(
        !input["context"]["trigger"]
            .as_str()
            .unwrap()
            .contains("Vercel")
    );
}

#[test]
fn ignored_checks_never_reach_graph_ci() {
    let c = cctx(Permission::Read);
    let t = ready(&c);
    let url = "https://github.com/enzarb/enzarb-ui/actions/runs/1/job/2";
    let t2 = transition(&c, &t.next, &named_ci("c1", "Copilot code review", url));
    assert_eq!(node(&t2), "ready");
    assert!(jobs(&t2).is_empty());
    assert_eq!(cr_of(&t2).data.ci_failures.len(), 0);
}

#[test]
fn a_passing_check_asks_for_the_aggregate_and_a_green_result_resumes_the_ci_wait() {
    let c = cctx(Permission::Read);
    let t = reviewed(&c, "approve", "LGTM");
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("e4", "sha1", "pending", Value::Null),
    );
    task(&t, GraphPhase::Waiting);
    assert_eq!(node(&t), "checks");
    assert_eq!(wait_of(&t), ("ci".into(), "ci_pending".into()));
    let t = transition(
        &c,
        &t.next,
        &ci_event("c1", "sha1", enzarb_connector::CiState::Success, ""),
    );
    task(&t, GraphPhase::Waiting);
    let (_, _, input) = job_of(&t, JobKind::CheckMergeReadiness);
    assert_eq!(input["head_sha"], "sha1");
    // The background check is green and mergeable: readiness, then the merge wait.
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("c2", "sha1", "success", json!(true)),
    );
    assert_eq!(node(&t), "readiness");
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("c3", "sha1", "success", json!(true)),
    );
    assert_eq!(node(&t), "ready");
}

#[test]
fn a_conflict_is_resolved_twice_per_base_then_parked() {
    let c = cctx(Permission::Read);
    let t = reviewed(&c, "approve", "LGTM");
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("e4", "sha1", "pending", Value::Null),
    );
    let t = transition(&c, &t.next, &unmergeable("u1", "base1"));
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "resolve");
    assert_eq!(wait_of(&t), ("enzarb".into(), "resolving_conflict".into()));
    // The same base again is not a second conflict.
    rejected(
        &transition(&c, &t.next, &unmergeable("u2", "base1")),
        &t.next,
    );
    // Blocked twice: parked for a person.
    let blocked = json!({"decision": "blocked", "ok": true});
    let t = transition(
        &c,
        &t.next,
        &segment(
            "s1",
            "tell_parked",
            "Could not.\n\n```json\n{\"decision\":\"blocked\",\"body\":\"stuck\"}\n```",
            "sha1",
            json!({"resolve": blocked, "resolve_again": blocked}),
        ),
    );
    assert_eq!(node(&t), "tell_parked");
    let t = transition(
        &c,
        &t.next,
        &ok("s2", JobKind::PostComment, json!({"id": 2})),
    );
    task(&t, GraphPhase::Waiting);
    assert_eq!(wait_of(&t), ("human".into(), "conflict_parked".into()));
    // A new base commit starts again.
    let t = transition(&c, &t.next, &unmergeable("u3", "base2"));
    assert_eq!(node(&t), "resolve");
}

#[test]
fn conflicts_are_only_resolved_on_change_requests_enzarb_opened() {
    let c = cctx(Permission::Read);
    let mut ev = pr_opened("e1");
    if let EventPayload::Connector(e) = &mut ev.payload {
        e.data.item.author = "alice".into();
    }
    let t = transition(&c, &WorkflowState::New(WorkflowKind::ChangeRequest), &ev);
    rejected(
        &transition(&c, &t.next, &unmergeable("u1", "base1")),
        &t.next,
    );
}

#[test]
fn a_push_by_a_person_is_reviewed_and_feedback_is_addressed() {
    let c = cctx(Permission::Write);
    let t = reviewed(&c, "comment", "Please look at the docs");
    task(&t, GraphPhase::Waiting);
    assert_eq!(node(&t), "human");
    assert_eq!(wait_of(&t).0, "human");
    // A person's comment: a round with their text.
    let fb = transition(&c, &t.next, &pr_comment("f1", "carol", "Rename it"));
    let g = task(&fb, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "address");
    let (_, _, input) = job_of(&fb, JobKind::AgentSegment);
    assert_eq!(input["context"]["trigger"], "Rename it");
    assert_eq!(
        wait_of(&fb),
        ("enzarb".into(), "addressing_comments".into())
    );
    // Readers do not trigger rounds.
    rejected(
        &transition(
            &cctx(Permission::Read),
            &t.next,
            &pr_comment("f2", "dave", "x"),
        ),
        &t.next,
    );
    // A push by a person: reviewed.
    let p = transition(&c, &t.next, &pushed_by("p1", "sha9", actor("carol")));
    let g = task(&p, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "review");
    assert_eq!(g.data.graph.outputs["human"]["event"], "pushed");
}

#[test]
fn feedback_during_a_run_is_queued_and_taken_by_the_next_wait() {
    let c = cctx(Permission::Write);
    let t = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    let q = transition(&c, &t.next, &pr_comment("f1", "carol", "Also fix the typo"));
    task(&q, GraphPhase::Running);
    assert_eq!(q.note, "queued for the next round");
    let t = transition(
        &c,
        &q.next,
        &segment(
            "e2",
            "post_review",
            &review_summary("approve", "LGTM"),
            "sha1",
            json!({}),
        ),
    );
    let t = transition(
        &c,
        &t.next,
        &ok("e3", JobKind::PostComment, json!({"id": 1})),
    );
    // At the merge wait the queued comment comes first.
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("e4", "sha1", "success", json!(true)),
    );
    let g = task(&t, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "address");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["context"]["trigger"], "Also fix the typo");
}

#[test]
fn closing_ends_the_task_merged_or_closed() {
    let c = cctx(Permission::Read);
    let t = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    let m = transition(&c, &t.next, &closed("x1", pr("sha1"), true));
    task(&m, GraphPhase::Merged);
    assert!(cancels(&m));
    let cl = transition(&c, &t.next, &closed("x2", pr("sha1"), false));
    task(&cl, GraphPhase::Closed);
    // Reopened: reviewed again from the start.
    let r = transition(&c, &cl.next, &pr_opened("x3"));
    let g = task(&r, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "review");
}

#[test]
fn head_moved_and_updated_branch_go_back_to_review() {
    let c = cctx(Permission::Write);
    let t = ready(&c);
    let m = transition(&c, &t.next, &merge_click("m1", "carol"));
    let moved = transition(
        &c,
        &m.next,
        &ok(
            "m2",
            JobKind::MergeChangeRequest,
            json!({"outcome": "head_moved"}),
        ),
    );
    assert_eq!(node(&moved), "review");
    let g = task(&moved, GraphPhase::Running);
    assert!(!g.data.cr.as_ref().unwrap().data.merge_requested);
    // Behind the base: the branch was updated, the click is kept for the next ready head.
    let upd = transition(
        &c,
        &m.next,
        &ok(
            "m3",
            JobKind::MergeChangeRequest,
            json!({"outcome": "updated_branch"}),
        ),
    );
    let g = task(&upd, GraphPhase::Running);
    assert_eq!(g.data.graph.node, "review");
    assert!(g.data.cr.as_ref().unwrap().data.merge_requested);
    assert_eq!(wait_of(&upd), ("enzarb".into(), "updating_branch".into()));
    // Not mergeable: a person looks.
    let nm = transition(
        &c,
        &m.next,
        &ok(
            "m4",
            JobKind::MergeChangeRequest,
            json!({"outcome": "not_mergeable", "reason": "blocked"}),
        ),
    );
    task(&nm, GraphPhase::Waiting);
    assert_eq!(wait_of(&nm), ("human".into(), "conflict_parked".into()));
}

const TIMER: &str = r#"
version: 1
name: change_request
on: [change_request_opened]
start: w
nodes:
  w: {type: wait, events: [timer, comment], after: 2h, next: {'event == "timer"': late, else: done}}
  late: {type: action, action: post_comment, body: "Still here", next: done}
  done: {type: end, status: done}
"#;

#[test]
fn a_timer_wait_sets_wake_at_and_resumes_when_it_fires() {
    let c = Ctx {
        graph: Some(source("change_request", TIMER)),
        ..ctx_for(43, Permission::Read)
    };
    let t = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    task(&t, GraphPhase::Waiting);
    assert_eq!(wake(&t), Some(Some(now() + chrono::Duration::hours(2))));
    let f = transition(&c, &t.next, &timer("t1", None));
    task(&f, GraphPhase::Running);
    assert_eq!(node(&f), "late");
    assert_eq!(wake(&f), Some(None));
    rejected(&transition(&c, &f.next, &timer("t2", None)), &f.next);
}

#[test]
fn issue_graph_waits_say_why_they_wait() {
    let gated = r#"
version: 1
name: issue
on: [issue_opened]
start: post
nodes:
  post: {type: action, action: post_comment, body: "plan", next: approve}
  approve: {type: wait, events: [approval, comment], next: {"event == 'approval'": done, else: ask}}
  ask: {type: wait, events: [comment], next: done}
  done: {type: end, status: done}
"#;
    let c = Ctx {
        graph: Some(source("issue", gated)),
        ..ctx(Permission::Write)
    };
    let t = transition(&c, &WorkflowState::New(WorkflowKind::Issue), &opened("e1"));
    assert_eq!(wait_of(&t), ("enzarb".into(), "working".into()));
    let t = transition(
        &c,
        &t.next,
        &ok("e2", JobKind::PostComment, json!({"id": 1})),
    );
    assert_eq!(wait_of(&t), ("human".into(), "approval".into()));
    let t = transition(&c, &t.next, &comment_by("e3", "carol", 5, "what about X?"));
    assert_eq!(wait_of(&t), ("human".into(), "question".into()));
}

#[test]
fn a_re_review_after_a_push_is_a_delta_of_the_last_reviewed_head() {
    let c = cctx(Permission::Write);
    let t = reviewed(&c, "approve", "LGTM");
    let r = ok(
        "e4",
        JobKind::CheckMergeReadiness,
        json!({"head_sha": "sha1", "ci": "success", "mergeable": true, "base_sha": "b0"}),
    );
    let t = transition(&c, &t.next, &r);
    assert_eq!(node(&t), "ready");
    let k = cr_of(&t);
    assert_eq!(k.data.prior_review.sha, "sha1");
    assert_eq!(k.data.prior_review.base_sha, "b0");
    assert_eq!(k.data.stats.review_rounds, 1);
    let t = transition(&c, &t.next, &pushed_by("p1", "sha9", actor("carol")));
    assert_eq!(node(&t), "review");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["context"]["previous_reviewed_sha"], "sha1");
}

fn bot() -> Actor {
    let mut a = actor(BOT);
    a.is_bot = true;
    a
}

fn cr_of(t: &Transition) -> crate::machine::graph::CrGraph {
    match &t.next {
        WorkflowState::Graph(g) => g.data.cr.clone().unwrap(),
        _ => panic!("not a graph"),
    }
}

/// Ready at sha1, clicked, the merge job updated the branch (update commit sha2 by enzarb),
/// the new head reviewed and approved: at the stored click's replay.
fn clicked_then_updated(c: &Ctx) -> Transition {
    let t = ready(c);
    let m = transition(c, &t.next, &merge_click("m1", "carol"));
    assert_eq!(node(&m), "merge");
    let upd = pushed_by("m2", "sha2", bot());
    let m = transition(c, &m.next, &upd);
    let t = transition(
        c,
        &m.next,
        &ok(
            "m3",
            JobKind::MergeChangeRequest,
            json!({"outcome": "updated_branch"}),
        ),
    );
    assert_eq!(node(&t), "review");
    let k = cr_of(&t);
    assert!(k.data.merge_requested);
    assert_eq!(k.merge_head, "sha2");
    t
}

#[test]
fn the_click_follows_only_the_base_update_commit() {
    let c = cctx(Permission::Write);
    let t = clicked_then_updated(&c);
    let t = transition(
        &c,
        &t.next,
        &segment(
            "r1",
            "post_review",
            &review_summary("approve", "ok"),
            "sha2",
            json!({}),
        ),
    );
    let t = transition(
        &c,
        &t.next,
        &ok("r2", JobKind::PostComment, json!({"id": 3})),
    );
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("r3", "sha2", "success", json!(true)),
    );
    // No second click needed for the update commit.
    assert_eq!(node(&t), "merge");
    let (_, _, input) = job_of(&t, JobKind::MergeChangeRequest);
    assert_eq!(input["expected_head_sha"], "sha2");
}

#[test]
fn the_click_does_not_survive_enzarbs_own_later_commits() {
    let c = cctx(Permission::Write);
    let t = clicked_then_updated(&c);
    // A conflict is reported during the review; GitHub says the update does not merge.
    let t = transition(&c, &t.next, &unmergeable("u1", "base9"));
    let t = transition(
        &c,
        &t.next,
        &segment(
            "r1",
            "post_review",
            &review_summary("approve", "ok"),
            "sha2",
            json!({}),
        ),
    );
    let t = transition(
        &c,
        &t.next,
        &ok("r2", JobKind::PostComment, json!({"id": 3})),
    );
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("r3", "sha2", "success", json!(false)),
    );
    assert_eq!(node(&t), "resolve");
    // The resolve pushes sha3, which is reviewed and approved in the same segment.
    let t = transition(
        &c,
        &t.next,
        &segment(
            "r4",
            "post_review",
            &review_summary("approve", "ok"),
            "sha3",
            json!({"resolve": {"ok": true, "decision": "done", "pushed": true}}),
        ),
    );
    assert!(
        !cr_of(&t).data.merge_requested,
        "a new commit drops the click"
    );
    let t = transition(
        &c,
        &t.next,
        &ok("r5", JobKind::PostComment, json!({"id": 4})),
    );
    let t = transition(
        &c,
        &t.next,
        &readiness_ok("r6", "sha3", "success", json!(true)),
    );
    // Ready, and waiting for a new click: nothing merges.
    task(&t, GraphPhase::Waiting);
    assert_eq!(node(&t), "ready");
    assert!(jobs(&t).iter().all(|j| j.0 != JobKind::MergeChangeRequest));
}

#[test]
fn the_click_is_dropped_by_requested_changes_red_ci_and_open_dependencies() {
    let c = cctx(Permission::Write);
    // Requested changes on the update commit.
    let t = clicked_then_updated(&c);
    let rc = segment(
        "r1",
        "post_review",
        &review_summary("request_changes", "no"),
        "sha2",
        json!({}),
    );
    let t2 = transition(&c, &t.next, &rc);
    assert!(!cr_of(&t2).data.merge_requested);
    // A red CI in the readiness check.
    let ap = segment(
        "r1",
        "post_review",
        &review_summary("approve", "ok"),
        "sha2",
        json!({}),
    );
    let t2 = transition(&c, &t.next, &ap);
    let t2 = transition(
        &c,
        &t2.next,
        &ok("r2", JobKind::PostComment, json!({"id": 3})),
    );
    let red = transition(
        &c,
        &t2.next,
        &readiness_ok("r3", "sha2", "failure", json!(true)),
    );
    assert!(!cr_of(&red).data.merge_requested);
    // A dependency reopened since the click: ready, but no merge and no click left.
    let deps = Ctx {
        open_dependencies: vec![OpenDependency {
            key: "github:enzarb/enzarb-ui#9".into(),
            label: "#9".into(),
            ..Default::default()
        }],
        ..cctx(Permission::Write)
    };
    let d = transition(
        &deps,
        &t2.next,
        &readiness_ok("r3", "sha2", "success", json!(true)),
    );
    assert_eq!(node(&d), "ready");
    assert!(jobs(&d).iter().all(|j| j.0 != JobKind::MergeChangeRequest));
    assert!(!cr_of(&d).data.merge_requested);
}

#[test]
fn a_push_by_a_person_during_the_review_is_kept_and_reviewed() {
    let c = cctx(Permission::Write);
    let t = transition(
        &c,
        &WorkflowState::New(WorkflowKind::ChangeRequest),
        &pr_opened("e1"),
    );
    let t = transition(&c, &t.next, &pushed_by("p1", "sha9", actor("carol")));
    task(&t, GraphPhase::Running);
    // The review of sha1 ends; the sandbox reports the head it saw, nothing pushed.
    let t = transition(
        &c,
        &t.next,
        &segment(
            "e2",
            "post_review",
            &review_summary("approve", "ok"),
            "sha1",
            json!({}),
        ),
    );
    let k = cr_of(&t);
    assert_eq!(
        k.data.head_sha, "sha9",
        "the person's push is not rolled back"
    );
    assert_eq!(k.data.reviewed_sha, "sha1");
    // CI for the real head is taken.
    let ci = ci_event("c1", "sha9", enzarb_connector::CiState::Pending, "");
    assert!(transition(&c, &t.next, &ci).accepted);
    // Once posted, the review is of an older commit: sha9 is reviewed next.
    let t = transition(
        &c,
        &t.next,
        &ok("e3", JobKind::PostComment, json!({"id": 1})),
    );
    assert_eq!(node(&t), "review");
    let (_, _, input) = job_of(&t, JobKind::AgentSegment);
    assert_eq!(input["context"]["head_sha"], "sha9");
}
