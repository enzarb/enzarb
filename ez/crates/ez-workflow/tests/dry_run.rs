//! Dry runs over the ADR-0029 example, as the designer drives them.

use ez_workflow::dry_run::ModeView;
use ez_workflow::schema::{EndStatus, NodeType};
use ez_workflow::{
    DryRunOptions, Engine, Event, ModeCatalog, Rendered, TraceEnd, Transition, dry_run, parse,
    wait_outputs,
};
use serde_json::json;
use std::collections::BTreeMap;

const WORKFLOWS: &str = include_str!("fixtures/workflows.toml");
const MODES: &str = include_str!("fixtures/modes.toml");

fn labeled(label: &str) -> Event {
    Event::new(
        "issues",
        Some("labeled".into()),
        json!({
            "action": "labeled",
            "label": {"name": label},
            "issue": {"number": 42, "title": "Crash", "body": "Boom", "user": {"login": "alice"}}
        }),
    )
}

fn run(event: &Event, opts: &DryRunOptions) -> ez_workflow::DryRun {
    let file = parse(WORKFLOWS).unwrap();
    let modes = ModeCatalog::from_modes_toml(MODES).unwrap();
    dry_run(&Engine::new(), &file, Some(&modes), event, opts)
}

#[test]
fn stops_at_the_first_unknown_outcome() {
    let d = run(&labeled("ez"), &DryRunOptions::default());
    assert!(d.report.is_ok());
    assert_eq!(d.triggers.len(), 1);
    assert!(d.triggers[0].matched);
    assert_eq!(d.traces.len(), 1);
    let t = &d.traces[0];
    assert!(!t.forced);
    assert_eq!(t.steps.len(), 1);
    let s = &t.steps[0];
    assert_eq!(
        (s.node.as_str(), s.node_type, s.visit),
        ("triage", NodeType::Agent, 1)
    );
    assert_eq!(
        s.mode,
        Some(ModeView {
            id: "triage".into(),
            tools: Some(vec!["Read".into(), "Glob".into(), "Grep".into()])
        })
    );
    assert!(
        matches!(&s.rendered, Some(Rendered::Agent { instruction, .. }) if instruction == "Issue #42: Crash\n\nBoom")
    );
    let TraceEnd::NeedsOutcome {
        node,
        outputs,
        edges,
        ..
    } = &t.end
    else {
        panic!("{:?}", t.end)
    };
    assert_eq!(node, "triage");
    assert_eq!(outputs, &["result", "git", "exit", "error"]);
    assert_eq!(edges.len(), 2);
    assert_eq!(
        edges[0].condition.as_deref(),
        Some("result.decision == 'fix'")
    );
}

#[test]
fn assumed_outcomes_walk_a_branch_to_its_end() {
    let reply = Event::new(
        "issue_comment",
        Some("created".into()),
        json!({"comment": {"user": {"login": "alice"}}, "issue": {"number": 42, "title": "Crash", "body": "Boom"}}),
    );
    let mut assume = BTreeMap::new();
    assume.insert(
        "triage".to_string(),
        json!({"result": {"decision": "fix", "summary": "S"}}),
    );
    assume.insert("implement".to_string(), json!({"git": {"pushed": true}}));
    assume.insert("open_pr".to_string(), json!({"number": 9, "url": "u"}));
    assume.insert("wait_reply".to_string(), wait_outputs(Some(&reply)));
    let d = run(
        &labeled("ez"),
        &DryRunOptions {
            assume,
            ..Default::default()
        },
    );
    let t = &d.traces[0];
    let path: Vec<&str> = t.steps.iter().map(|s| s.node.as_str()).collect();
    assert_eq!(path, ["triage", "implement", "open_pr", "end_done"]);
    assert_eq!(
        t.steps[0].transition,
        Some(Transition::Moved {
            from: "triage".into(),
            to: "implement".into(),
            edge: 0
        })
    );
    // Dry runs are test runs: nothing is pushed.
    assert!(matches!(
        t.steps[1].rendered,
        Some(Rendered::Agent { push: false, .. })
    ));
    assert!(
        matches!(&t.steps[2].rendered, Some(Rendered::Action { title: Some(x), .. }) if x == "Fix #42: Crash")
    );
    assert_eq!(
        t.end,
        TraceEnd::Ended {
            status: EndStatus::Done,
            node: Some("end_done".into()),
            reason: None
        }
    );
}

#[test]
fn a_loop_with_the_same_assumption_ends_at_max_visits() {
    let reply = Event::new(
        "issue_comment",
        Some("created".into()),
        json!({"comment": {"user": {"login": "alice"}}, "issue": {"number": 42, "title": "Crash", "body": "Boom"}}),
    );
    let mut assume = BTreeMap::new();
    assume.insert(
        "triage".to_string(),
        json!({"result": {"decision": "needs_info", "summary": "?"}}),
    );
    assume.insert("ask".to_string(), json!({"number": 1, "url": "u"}));
    assume.insert("wait_reply".to_string(), wait_outputs(Some(&reply)));
    let d = run(
        &labeled("ez"),
        &DryRunOptions {
            assume,
            ..Default::default()
        },
    );
    let t = &d.traces[0];
    assert_eq!(t.steps.len(), 15);
    let TraceEnd::Ended { status, reason, .. } = &t.end else {
        panic!("{:?}", t.end)
    };
    assert_eq!(*status, EndStatus::Failed);
    assert!(reason.as_deref().unwrap().contains("max_visits"));

    let mut assume2 = BTreeMap::new();
    assume2.insert(
        "triage".to_string(),
        json!({"result": {"decision": "needs_info", "summary": "?"}}),
    );
    assume2.insert("ask".to_string(), json!({"number": 1, "url": "u"}));
    assume2.insert("wait_reply".to_string(), wait_outputs(Some(&reply)));
    let d = run(
        &labeled("ez"),
        &DryRunOptions {
            assume: assume2,
            max_steps: 4,
            ..Default::default()
        },
    );
    assert_eq!(d.traces[0].end, TraceEnd::StepLimit { steps: 4 });
}

#[test]
fn unmatched_events_explain_why_and_can_be_forced() {
    let d = run(&labeled("bug"), &DryRunOptions::default());
    assert!(!d.triggers[0].matched);
    assert!(
        d.triggers[0].reason.contains("is false"),
        "{}",
        d.triggers[0].reason
    );
    assert!(d.traces.is_empty());

    let d = run(
        &labeled("bug"),
        &DryRunOptions {
            workflow: Some("fix-issue".into()),
            ..Default::default()
        },
    );
    assert_eq!(d.traces.len(), 1);
    assert!(d.traces[0].forced);

    let d = run(
        &labeled("bug"),
        &DryRunOptions {
            workflow: Some("ghost".into()),
            ..Default::default()
        },
    );
    assert!(
        matches!(&d.traces[0].end, TraceEnd::Ended { status: EndStatus::Failed, reason: Some(r), .. } if r.contains("ghost"))
    );
}

#[test]
fn render_failures_end_the_trace_at_that_node() {
    // The event has no issue body, which the triage instruction needs.
    let ev = Event::new(
        "issues",
        Some("labeled".into()),
        json!({"label": {"name": "ez"}, "issue": {"number": 1, "title": "t"}}),
    );
    let d = run(&ev, &DryRunOptions::default());
    let t = &d.traces[0];
    assert_eq!(t.steps.len(), 1);
    assert!(t.steps[0].rendered.is_none());
    let TraceEnd::Ended { status, reason, .. } = &t.end else {
        panic!("{:?}", t.end)
    };
    assert_eq!(*status, EndStatus::Failed);
    assert!(
        reason.as_deref().unwrap().contains("instruction"),
        "{reason:?}"
    );
}

#[test]
fn invalid_files_report_without_tracing() {
    let file = parse(&WORKFLOWS.replace("mode = \"implement\"", "mode = \"ghost\"")).unwrap();
    let modes = ModeCatalog::from_modes_toml(MODES).unwrap();
    let d = dry_run(
        &Engine::new(),
        &file,
        Some(&modes),
        &labeled("ez"),
        &DryRunOptions::default(),
    );
    assert!(!d.report.is_ok());
    assert!(d.triggers.is_empty() && d.traces.is_empty());
}

#[test]
fn results_serialize_for_the_ui() {
    let d = run(&labeled("ez"), &DryRunOptions::default());
    let v = serde_json::to_value(&d).unwrap();
    assert_eq!(v["traces"][0]["end"]["kind"], "needs_outcome");
    assert_eq!(v["traces"][0]["steps"][0]["rendered"]["type"], "agent");
    assert_eq!(v["traces"][0]["steps"][0]["node_type"], "agent");
    let opts: DryRunOptions =
        serde_json::from_value(json!({"assume": {"triage": {"result": {}}}})).unwrap();
    assert_eq!(opts.assume.len(), 1);
}
