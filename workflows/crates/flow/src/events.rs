//! The standard event catalog (ADR-0022 §5). Connectors map provider
//! webhooks into these objects; waits and bindings match them by `type`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// An event as delivered to a task: envelope fields plus a typed payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Event {
    #[serde(flatten)]
    pub payload: Payload,
    /// Dedupe id assigned by the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `owner/repo` the event is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,
    /// The GitHub object the event is about, used for routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectRef>,
    /// RFC 3339 time the event happened. The interpreter's only clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Event {
    pub fn new(payload: Payload) -> Self {
        Self {
            payload,
            id: None,
            repo: None,
            actor: None,
            subject: None,
            at: None,
        }
    }

    /// The matchable type name: the catalog `type`, or a custom event's `name`.
    pub fn type_name(&self) -> &str {
        match &self.payload {
            Payload::Custom(c) => &c.name,
            p => p.kind(),
        }
    }

    pub fn can_write(&self) -> bool {
        self.actor.as_ref().is_some_and(|a| a.can_write)
    }
}

/// Who caused an event.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Actor {
    pub login: String,
    /// Whether the actor has write access to the repo. Supplied by the engine
    /// from GitHub, never from the payload sender.
    #[serde(default)]
    pub can_write: bool,
    #[serde(default)]
    pub bot: bool,
}

/// A GitHub object: `{kind: issue|pull_request, number}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectRef {
    pub kind: SubjectKind,
    pub number: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Issue,
    PullRequest,
}

macro_rules! catalog {
    ($( $(#[$m:meta])* $variant:ident ( $ty:ident ) = $name:literal ),* $(,)?) => {
        /// The typed payload, tagged by `type`.
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(tag = "type", rename_all = "snake_case")]
        pub enum Payload {
            $( $(#[$m])* $variant($ty), )*
        }

        impl Payload {
            pub fn kind(&self) -> &'static str {
                match self { $( Payload::$variant(_) => $name, )* }
            }
        }

        /// Every catalog type name.
        pub const CATALOG: &[&str] = &[$($name),*];

        /// `(type name, struct name, schema)` for each catalog type.
        pub fn catalog_schemas() -> Vec<(&'static str, &'static str, schemars::Schema)> {
            vec![$( ($name, stringify!($ty), schemars::schema_for!($ty)) ),*]
        }
    };
}

catalog! {
    /// A comment on an issue or pull request.
    Comment(Comment) = "comment",
    /// An issue was opened, edited, closed or reopened.
    Issue(Issue) = "issue",
    /// A label was added or removed.
    Label(Label) = "label",
    /// Commits were pushed to a branch.
    Push(Push) = "push",
    /// A pull request changed.
    PullRequest(PullRequest) = "pull_request",
    /// A commit status changed.
    CiStatus(CiStatus) = "ci_status",
    /// A check run changed.
    CheckRun(CheckRun) = "check_run",
    /// A pull request review was submitted.
    Review(Review) = "review",
    /// A release changed.
    Release(Release) = "release",
    /// A tag was created or deleted.
    Tag(Tag) = "tag",
    /// A package version was published.
    Package(Package) = "package",
    /// A deployment status changed.
    Deployment(Deployment) = "deployment",
    /// A CI workflow run changed.
    WorkflowRun(WorkflowRun) = "workflow_run",
    /// A cron binding fired.
    Schedule(Schedule) = "schedule",
    /// The task was created by the adopt pass for an existing object.
    Adopted(Adopted) = "adopted",
    /// A task this task depends on changed status.
    DependencyChanged(DependencyChanged) = "dependency_changed",
    /// A child task ended.
    ChildEnded(ChildEnded) = "child_ended",
    /// Every child task has ended.
    AllChildrenEnded(AllChildrenEnded) = "all_children_ended",
    /// A project-declared event sent through `POST /v1/events/<project>`.
    Custom(Custom) = "custom",
}

/// `command`/`args` are parsed from the first line (`/word a b`,
/// optionally after a leading `@bot` mention).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Comment {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Comment {
    /// A comment whose command is parsed from `body` (`bot` is the login a
    /// leading mention must name, if any).
    pub fn parsed(body: &str, bot: Option<&str>) -> Self {
        let (command, args) = parse_command(body, bot).unzip();
        Self {
            body: body.into(),
            command,
            args: args.unwrap_or_default(),
            url: None,
        }
    }
}

/// Parses `/word args…` from the first non-empty line, allowing one leading
/// `@bot` mention. Command words are lower-cased.
pub fn parse_command(body: &str, bot: Option<&str>) -> Option<(String, Vec<String>)> {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty())?;
    let mut words = line.split_whitespace().peekable();
    if let Some(w) = words.peek()
        && let Some(name) = w.strip_prefix('@')
    {
        match bot {
            Some(b) if name.eq_ignore_ascii_case(b) => {
                words.next();
            }
            _ => return None,
        }
    }
    let cmd = words.next()?.strip_prefix('/')?;
    if cmd.is_empty()
        || !cmd
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some((
        cmd.to_ascii_lowercase(),
        words.map(str::to_string).collect(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    /// `opened`, `edited`, `closed`, `reopened`, …
    pub action: String,
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Label {
    /// `added` or `removed`.
    pub action: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Push {
    pub branch: String,
    #[serde(default)]
    pub before: String,
    pub after: String,
    #[serde(default)]
    pub commits: u32,
    #[serde(default)]
    pub forced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct PullRequest {
    /// `opened`, `synchronize`, `closed`, `ready_for_review`, …
    pub action: String,
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub base: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub draft: bool,
    /// Issues this PR references (closing keywords or Development links).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct CiStatus {
    pub sha: String,
    /// `pending`, `success`, `failure` or `error`; for the combined status of `sha`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct CheckRun {
    pub sha: String,
    pub name: String,
    /// `queued`, `in_progress`, `completed`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Review {
    pub number: u64,
    /// `approved`, `changes_requested` or `commented`.
    pub state: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Release {
    /// `published`, `created`, `edited`, …
    pub action: String,
    pub tag: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub prerelease: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Tag {
    /// `created` or `deleted`.
    pub action: String,
    pub name: String,
    #[serde(default)]
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Package {
    pub action: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub ecosystem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Deployment {
    pub environment: String,
    pub state: String,
    #[serde(default)]
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRun {
    pub name: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub sha: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Schedule {
    pub cron: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Adopted {
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct DependencyChanged {
    pub task: String,
    /// The dependency's engine phase or end status.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<SubjectRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct ChildEnded {
    pub task: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct AllChildrenEnded {
    /// End status per child task id.
    pub statuses: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Custom {
    /// Declared event name (the type waits match on).
    pub name: String,
    /// Payload, validated against the declared schema.
    #[serde(default)]
    pub payload: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_commands() {
        type Case<'a> = (&'a str, Option<&'a str>, Option<(&'a str, &'a [&'a str])>);
        let cases: &[Case] = &[
            ("/plan", None, Some(("plan", &[]))),
            (
                "  \n/Plan now please",
                None,
                Some(("plan", &["now", "please"])),
            ),
            ("@enzarb /stop", Some("enzarb"), Some(("stop", &[]))),
            ("@Enzarb /stop", Some("enzarb"), Some(("stop", &[]))),
            ("@alice /stop", Some("enzarb"), None),
            ("@enzarb /stop", None, None),
            ("please /plan", None, None),
            ("/", None, None),
            ("/a.b", None, None),
            ("", None, None),
        ];
        for (body, bot, want) in cases {
            let got = parse_command(body, *bot);
            let want = want.map(|(c, a)| {
                (
                    c.to_string(),
                    a.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                )
            });
            assert_eq!(got, want, "{body:?}");
        }
    }

    #[test]
    fn envelope_roundtrip_and_type_names() {
        let v = json!({"type": "comment", "body": "/plan", "command": "plan", "actor": {"login": "a", "can_write": true, "bot": false}});
        let e: Event = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(e.type_name(), "comment");
        assert!(e.can_write());
        assert_eq!(serde_json::to_value(&e).unwrap(), v);
        let c: Event = serde_json::from_value(
            json!({"type": "custom", "name": "deployed", "payload": {"x": 1}}),
        )
        .unwrap();
        assert_eq!(c.type_name(), "deployed");
        assert_eq!(CATALOG.len(), catalog_schemas().len());
    }
}
