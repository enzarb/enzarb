//! Every UI rule template (`ui/src/lib/rule-templates/templates.json`) is valid against the real
//! ez-core modes validation and ez-workflow schema, merged into an empty repository.

use ez_core::modes_api::{self, ModesDoc};
use ez_workflow::{Engine, ModeCatalog, WorkflowsFile};
use serde::Deserialize;
use std::collections::BTreeMap;

const TEMPLATES: &str = include_str!("../../ui/src/lib/rule-templates/templates.json");

#[derive(Deserialize)]
struct Template {
    id: String,
    title: String,
    description: String,
    #[serde(rename = "defaultMode")]
    default_mode: String,
    modes: Vec<modes_api::ModeDto>,
    workflow: serde_json::Value,
}

#[test]
fn every_template_is_valid() {
    let templates: Vec<Template> = serde_json::from_str(TEMPLATES).unwrap();
    assert!(templates.len() >= 3);
    let engine = Engine::new();
    for t in templates {
        assert!(!t.title.is_empty() && !t.description.is_empty(), "{}", t.id);
        let doc = ModesDoc {
            default_mode: t.default_mode.clone(),
            modes: t.modes.clone(),
        };
        let issues = modes_api::validate_doc(&doc, None);
        assert!(
            !issues.iter().any(|i| i.severity == "error"),
            "{} modes: {issues:?}",
            t.id
        );
        let modes_toml = ez_core::modes_edit::render(None, &doc).unwrap().text;
        let catalog = ModeCatalog::from_modes_toml(&modes_toml).unwrap();

        let mut workflows = BTreeMap::new();
        workflows.insert(t.id.clone(), t.workflow.clone());
        let file: WorkflowsFile =
            serde_json::from_value(serde_json::json!({ "version": 1, "workflows": workflows }))
                .unwrap_or_else(|e| panic!("{}: {e}", t.id));
        let text = ez_workflow::to_toml(&file).unwrap();
        let (parsed, report) = ez_workflow::check(&engine, &text, Some(&catalog));
        assert!(parsed.is_some(), "{} does not parse", t.id);
        let errors: Vec<_> = report.errors().collect();
        assert!(errors.is_empty(), "{}: {errors:?}", t.id);
    }
}

/// The example pair the rules chat shows the agent (`rules-chat.ts`) is valid.
#[test]
fn chat_example_is_valid() {
    let modes = include_str!("../../ui/src/lib/rule-templates/chat-example.modes.toml");
    let workflows = include_str!("../../ui/src/lib/rule-templates/chat-example.workflows.toml");
    let catalog = ModeCatalog::from_modes_toml(modes).unwrap();
    let (parsed, report) = ez_workflow::check(&Engine::new(), workflows, Some(&catalog));
    assert!(parsed.is_some(), "does not parse");
    let errors: Vec<_> = report.errors().collect();
    assert!(errors.is_empty(), "{errors:?}");
}

/// `fix-failing-ci` reacts to the repository's own workflow files, not to bots that also report
/// as workflow runs (Copilot code review has `path = "dynamic/..."`).
#[test]
fn fix_failing_ci_ignores_runs_that_are_not_repository_workflows() {
    let templates: Vec<Template> = serde_json::from_str(TEMPLATES).unwrap();
    let t = templates.iter().find(|t| t.id == "fix-failing-ci").unwrap();
    let cond = t.workflow["if"].as_str().unwrap();
    let engine = Engine::new();
    let eval = |conclusion: &str, path: &str| {
        let ctx = serde_json::json!({"event": {"workflow_run": {"conclusion": conclusion, "path": path}}});
        engine.eval_bool(cond, &ctx).unwrap()
    };
    assert!(eval("failure", ".github/workflows/ci.yml"));
    assert!(!eval("success", ".github/workflows/ci.yml"));
    assert!(!eval(
        "failure",
        "dynamic/agents/copilot-pull-request-reviewer"
    ));
}

/// The failure end of `fix-failing-ci` explains itself, with or without a submitted result.
#[test]
fn fix_failing_ci_failure_reason_renders() {
    let templates: Vec<Template> = serde_json::from_str(TEMPLATES).unwrap();
    let t = templates.iter().find(|t| t.id == "fix-failing-ci").unwrap();
    let reason = t.workflow["nodes"]["failed"]["reason"].as_str().unwrap();
    let engine = Engine::new();
    let render = |nodes: serde_json::Value| {
        engine
            .render(
                ez_workflow::expr::TemplateKind::Text,
                reason,
                &serde_json::json!({ "nodes": nodes }),
            )
            .unwrap()
    };
    assert_eq!(
        render(serde_json::json!({"fix": {"result": {"summary": "rate limited"}}})),
        "the CI fixer made no push: rate limited"
    );
    assert!(
        render(serde_json::json!({"fix": {"exit": "success"}}))
            .ends_with("no summary was submitted")
    );
    assert!(
        render(serde_json::json!({"fix": {"result": null}})).ends_with("no summary was submitted")
    );
}

/// `implement-labeled-issues` pauses for plan approval unless `vars.approve_plan` is false, and
/// the approval wait takes manual approve/revise and writers' `@enzarb approve|revise` comments.
#[test]
fn implement_template_gates_on_plan_approval() {
    let templates: Vec<Template> = serde_json::from_str(TEMPLATES).unwrap();
    let t = templates
        .iter()
        .find(|t| t.id == "implement-labeled-issues")
        .unwrap();
    assert_eq!(t.workflow["vars"]["approve_plan"], true);
    let nodes = &t.workflow["nodes"];
    let plan_next = nodes["plan"]["next"].as_array().unwrap();
    assert_eq!(plan_next[1]["if"], "vars.approve_plan");
    assert_eq!(plan_next[2]["to"], "implement");
    let wait = &nodes["approve_plan"];
    assert_eq!(wait["type"], "wait");
    assert_eq!(
        wait["on"],
        serde_json::json!(["manual", "issue_comment.created"])
    );
    assert_eq!(wait["timeout"], "14d");
    assert_eq!(nodes["plan_expired"]["reason"], "plan not approved in time");

    let cond = wait["if"].as_str().unwrap();
    let engine = Engine::new();
    let comment = |body: &str, kind: &str, assoc: &str, issue: u64| {
        serde_json::json!({"subject": {"number": 7}, "event": {"name": "issue_comment", "action": "created",
            "issue": {"number": issue},
            "comment": {"body": body, "user": {"type": kind}, "author_association": assoc}}})
    };
    let manual = |action: &str| {
        serde_json::json!({"subject": {"number": 7}, "event": {"name": "manual", "action": action,
            "issue": {"number": 7}}})
    };
    assert!(engine.eval_bool(cond, &manual("approve")).unwrap());
    assert!(engine.eval_bool(cond, &manual("revise")).unwrap());
    assert!(!engine.eval_bool(cond, &manual("implement")).unwrap());
    let ok = |c| engine.eval_bool(cond, &c).unwrap();
    assert!(ok(comment("@enzarb approve", "User", "OWNER", 7)));
    assert!(ok(comment(
        "  @enzarb revise smaller diff",
        "User",
        "COLLABORATOR",
        7
    )));
    assert!(ok(comment(
        "thanks\n@enzarb approve\n",
        "User",
        "MEMBER",
        7
    )));
    assert!(!ok(comment("@enzarb approve", "User", "NONE", 7)));
    assert!(!ok(comment("@enzarb approve", "Bot", "MEMBER", 7)));
    assert!(!ok(comment("@enzarb approve", "User", "OWNER", 8)));
    assert!(!ok(comment("lgtm", "User", "OWNER", 7)));
    assert!(!ok(comment("`@enzarb approve`", "User", "OWNER", 7)));
}

/// The triage and review agents continue to their comment only on success with a result; any
/// other outcome ends the run `failed` with the agent's real error (or a fallback).
#[test]
fn agent_failure_ends_with_the_agents_error() {
    let templates: Vec<Template> = serde_json::from_str(TEMPLATES).unwrap();
    let engine = Engine::new();
    for (id, node, next) in [
        ("review-pull-requests", "review", "comment"),
        ("triage-issues", "triage", "label"),
    ] {
        let t = templates.iter().find(|t| t.id == id).unwrap();
        let edges = t.workflow["nodes"][node]["next"].as_array().unwrap();
        assert_eq!(edges[0]["to"], next, "{id}");
        assert_eq!(edges[1]["to"], "failed", "{id}");
        let cond = edges[0]["if"].as_str().unwrap();
        let go = |exit: &str, result: serde_json::Value| {
            engine
                .eval_bool(cond, &serde_json::json!({"exit": exit, "result": result}))
                .unwrap()
        };
        assert!(go("success", serde_json::json!({"summary": "ok"})), "{id}");
        assert!(!go("failure", serde_json::Value::Null), "{id}");
        assert!(!go("success", serde_json::Value::Null), "{id}");
        assert!(
            !go("interrupted", serde_json::json!({"summary": "x"})),
            "{id}"
        );

        let reason = t.workflow["nodes"]["failed"]["reason"].as_str().unwrap();
        let render = |n: serde_json::Value| {
            engine
                .render(
                    ez_workflow::expr::TemplateKind::Text,
                    reason,
                    &serde_json::json!({ "nodes": { node: n } }),
                )
                .unwrap()
        };
        let msg = render(serde_json::json!({"exit": "failure", "result": null,
            "error": "git clone failed: No space left on device"}));
        assert!(
            msg.ends_with("git clone failed: No space left on device"),
            "{msg}"
        );
        let msg = render(serde_json::json!({"exit": "failure", "result": null, "error": null}));
        assert!(msg.ends_with("no result was submitted"), "{msg}");
    }
}

/// `implement-labeled-issues` never renders a plan that was not submitted.
#[test]
fn implement_plan_failure_ends_failed() {
    let templates: Vec<Template> = serde_json::from_str(TEMPLATES).unwrap();
    let t = templates
        .iter()
        .find(|t| t.id == "implement-labeled-issues")
        .unwrap();
    let edges = t.workflow["nodes"]["plan"]["next"].as_array().unwrap();
    assert_eq!(edges[0]["to"], "plan_failed");
    let engine = Engine::new();
    let cond = edges[0]["if"].as_str().unwrap();
    let ev = |exit: &str, result: serde_json::Value| {
        engine
            .eval_bool(cond, &serde_json::json!({"exit": exit, "result": result}))
            .unwrap()
    };
    assert!(ev("failure", serde_json::Value::Null));
    assert!(ev("success", serde_json::Value::Null));
    assert!(!ev("success", serde_json::json!({"plan": "p"})));
}
