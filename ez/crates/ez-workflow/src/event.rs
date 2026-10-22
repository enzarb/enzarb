//! Events (rows of `connectors.event_v`), `on` patterns, subjects and trigger matching.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::expr::Engine;
use crate::schema::WorkflowsFile;

/// GitHub webhook event names. An `on` name outside this list is only a warning: GitHub adds
/// events.
pub const KNOWN_EVENTS: &[&str] = &[
    "branch_protection_rule",
    "check_run",
    "check_suite",
    "code_scanning_alert",
    "commit_comment",
    "create",
    "delete",
    "dependabot_alert",
    "deployment",
    "deployment_status",
    "discussion",
    "discussion_comment",
    "fork",
    "gollum",
    "installation",
    "installation_repositories",
    "issue_comment",
    "issues",
    "label",
    "member",
    "merge_group",
    "meta",
    "milestone",
    "package",
    "ping",
    "public",
    "pull_request",
    "pull_request_review",
    "pull_request_review_comment",
    "pull_request_review_thread",
    "push",
    "registry_package",
    "release",
    "repository",
    "repository_dispatch",
    "secret_scanning_alert",
    "security_advisory",
    "star",
    "status",
    "sub_issues",
    "watch",
    "workflow_dispatch",
    "workflow_job",
    "workflow_run",
];

/// One logged delivery: `connectors.event_v`'s `event`, `action` and `payload`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// The `X-GitHub-Event` name (`issues`).
    pub name: String,
    /// `payload.action`, when the event has one.
    #[serde(default)]
    pub action: Option<String>,
    pub payload: Value,
}

impl Event {
    pub fn new(name: impl Into<String>, action: Option<String>, payload: Value) -> Self {
        Self {
            name: name.into(),
            action,
            payload,
        }
    }

    /// The `event` template variable: the payload with `name` and `action` set.
    pub fn context(&self) -> Value {
        let mut v = match &self.payload {
            Value::Object(m) => Value::Object(m.clone()),
            other => serde_json::json!({ "payload": other }),
        };
        if let Value::Object(m) = &mut v {
            m.insert("name".into(), Value::String(self.name.clone()));
            m.insert(
                "action".into(),
                self.action
                    .clone()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
        }
        v
    }

    /// Whether an `on` entry (`issues` or `issues.labeled`) matches this event.
    pub fn matches(&self, pattern: &str) -> bool {
        match pattern.split_once('.') {
            None => pattern == self.name,
            Some((name, action)) => name == self.name && self.action.as_deref() == Some(action),
        }
    }

    /// The issue or pull request the event is about.
    pub fn subject(&self) -> Option<Subject> {
        let p = &self.payload;
        let (obj, kind) = match self.name.as_str() {
            "issues" | "issue_comment" => {
                let issue = p.get("issue")?;
                let kind = if issue.get("pull_request").is_some_and(|v| !v.is_null()) {
                    SubjectKind::PullRequest
                } else {
                    SubjectKind::Issue
                };
                (issue, kind)
            }
            "pull_request"
            | "pull_request_review"
            | "pull_request_review_comment"
            | "pull_request_review_thread" => (p.get("pull_request")?, SubjectKind::PullRequest),
            _ => return None,
        };
        Some(Subject {
            kind,
            number: obj.get("number")?.as_u64()?,
            author: obj
                .pointer("/user/login")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            url: obj
                .get("html_url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Issue,
    PullRequest,
}

/// The issue or pull request a run belongs to (the `subject` template variable).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subject {
    pub kind: SubjectKind,
    pub number: u64,
    pub author: String,
    pub url: String,
}

impl Subject {
    /// The routing key: `owner/repo#number` (issues and pull requests share numbers).
    pub fn key(&self, repo: &str) -> String {
        format!("{}#{}", repo.to_ascii_lowercase(), self.number)
    }
}

/// Whether an `on` entry is well formed: `name` or `name.action`, lowercase and underscores.
pub fn valid_pattern(pattern: &str) -> bool {
    let part = |s: &str| {
        !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    };
    match pattern.split_once('.') {
        None => part(pattern),
        Some((n, a)) => part(n) && part(a),
    }
}

/// How one workflow's trigger judged an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerEval {
    pub workflow: String,
    pub matched: bool,
    /// The `on` entry that matched, if any.
    #[serde(default)]
    pub on: Option<String>,
    /// Why it matched or not, for people.
    pub reason: String,
}

/// The trigger's template context: `event` and `subject`.
pub fn trigger_context(event: &Event) -> Value {
    serde_json::json!({
        "event": event.context(),
        "subject": event.subject(),
    })
}

/// Checks every workflow's `on` and `if` against `event`, in file order.
pub fn evaluate_triggers(engine: &Engine, file: &WorkflowsFile, event: &Event) -> Vec<TriggerEval> {
    let ctx = trigger_context(event);
    file.workflows
        .iter()
        .map(|(id, wf)| {
            let Some(on) = wf.on.iter().find(|p| event.matches(p)) else {
                return TriggerEval {
                    workflow: id.clone(),
                    matched: false,
                    on: None,
                    reason: format!(
                        "`{}` is not in on = [{}]",
                        event_label(event),
                        wf.on.join(", ")
                    ),
                };
            };
            let (matched, reason) = match &wf.condition {
                None => (true, format!("on `{on}`")),
                Some(cond) => match engine.eval_bool(cond, &ctx) {
                    Ok(true) => (true, format!("on `{on}`, if `{cond}` is true")),
                    Ok(false) => (false, format!("if `{cond}` is false")),
                    Err(e) => (false, format!("if `{cond}` failed: {e}")),
                },
            };
            TriggerEval {
                workflow: id.clone(),
                matched,
                on: Some(on.clone()),
                reason,
            }
        })
        .collect()
}

/// `name.action` or `name`.
pub fn event_label(event: &Event) -> String {
    match &event.action {
        Some(a) => format!("{}.{a}", event.name),
        None => event.name.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn issue_event(action: &str) -> Event {
        Event::new(
            "issues",
            Some(action.into()),
            json!({
                "action": action,
                "label": {"name": "ez"},
                "issue": {"number": 12, "title": "Crash", "user": {"login": "alice"},
                          "html_url": "https://github.com/o/r/issues/12"}
            }),
        )
    }

    #[test]
    fn patterns_match_name_and_action() {
        let e = issue_event("labeled");
        assert!(e.matches("issues"));
        assert!(e.matches("issues.labeled"));
        assert!(!e.matches("issues.opened"));
        assert!(!e.matches("pull_request"));
        let push = Event::new("push", None, json!({}));
        assert!(push.matches("push"));
        assert!(!push.matches("push.created"));
    }

    #[test]
    fn validates_patterns() {
        assert!(valid_pattern("issues"));
        assert!(valid_pattern("pull_request.opened"));
        for bad in ["", "Issues", "issues.", ".x", "a.b.c", "is sues", "a-b"] {
            assert!(!valid_pattern(bad), "{bad}");
        }
    }

    #[test]
    fn derives_subjects() {
        let s = issue_event("opened").subject().unwrap();
        assert_eq!(s.kind, SubjectKind::Issue);
        assert_eq!((s.number, s.author.as_str()), (12, "alice"));
        assert_eq!(s.key("O/R"), "o/r#12");

        let c = Event::new(
            "issue_comment",
            Some("created".into()),
            json!({"issue": {"number": 3, "pull_request": {"url": "x"}, "user": {"login": "b"}}}),
        );
        assert_eq!(c.subject().unwrap().kind, SubjectKind::PullRequest);

        let pr = Event::new(
            "pull_request_review",
            Some("submitted".into()),
            json!({"pull_request": {"number": 9, "user": {"login": "c"}, "html_url": "u"}}),
        );
        assert_eq!(pr.subject().unwrap().number, 9);

        assert!(Event::new("push", None, json!({})).subject().is_none());
        assert!(
            Event::new("issues", None, json!({"issue": {}}))
                .subject()
                .is_none()
        );
    }

    #[test]
    fn context_has_name_and_action() {
        let ctx = issue_event("labeled").context();
        assert_eq!(ctx["name"], "issues");
        assert_eq!(ctx["action"], "labeled");
        assert_eq!(ctx["issue"]["number"], 12);
        let odd = Event::new("x", None, json!([1])).context();
        assert_eq!(odd["payload"], json!([1]));
        assert_eq!(odd["action"], Value::Null);
    }

    #[test]
    fn evaluates_triggers_in_order() {
        let file = crate::schema::parse(
            r#"
version = 1
[workflows.a]
on = ["issues.labeled"]
if = "event.label.name == 'ez'"
start = "e"
nodes.e = { type = "end", status = "done" }
[workflows.b]
on = ["pull_request"]
start = "e"
nodes.e = { type = "end", status = "done" }
[workflows.c]
on = ["issues"]
if = "event.nope.x"
start = "e"
nodes.e = { type = "end", status = "done" }
[workflows.d]
on = ["issues"]
if = "subject.author == 'bob'"
start = "e"
nodes.e = { type = "end", status = "done" }
"#,
        )
        .unwrap();
        let evals = evaluate_triggers(&Engine::new(), &file, &issue_event("labeled"));
        let got: Vec<(&str, bool)> = evals
            .iter()
            .map(|e| (e.workflow.as_str(), e.matched))
            .collect();
        assert_eq!(got, [("a", true), ("b", false), ("c", false), ("d", false)]);
        assert_eq!(evals[0].on.as_deref(), Some("issues.labeled"));
        assert!(evals[1].reason.contains("not in on"), "{}", evals[1].reason);
        assert!(evals[2].reason.contains("failed"), "{}", evals[2].reason);
        assert!(evals[3].reason.contains("is false"), "{}", evals[3].reason);
    }
}
