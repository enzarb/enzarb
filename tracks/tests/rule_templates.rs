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
