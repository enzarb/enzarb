//! Prompt templates and the agent output contract.
//!
//! # Agent output contract
//! The agent's **final message must end with one fenced `json` block**:
//!
//! ````text
//! ```json
//! { "decision": "<one of the kind's decisions>", "body": "<markdown>" }
//! ```
//! ````
//!
//! Nothing may follow the closing fence. `body` is a JSON string holding markdown;
//! it is what gets posted (a comment, or the pull request description for
//! `implement`). The machine reads it from the job output's `summary` (Claude's
//! result text, as delivered by `RunFinished`). Allowed `decision` values:
//!
//! | job kind | decisions |
//! |---|---|
//! | `agent_triage` | `questions` (body: the questions to post), `ready` (body: summary for planning), `duplicate` (body: comment pointing at the original) |
//! | `agent_plan` | `plan` (body: the markdown plan) |
//! | `agent_implement` | `done` (body: pull request description), `blocked` (body: why) |
//! | `agent_review` | `approve`, `request_changes`, `comment` (body: the review) |
//! | `agent_address_comments` | `done` (body: what changed), `blocked` (body: why) |
//! | `agent_resolve_conflicts` | `done` (body: how it was resolved), `blocked` (body: the conflict) |
//! | `agent_task` | `reply`, `blocked` (body: the reply to post) |
//!
//! A missing block, invalid JSON, an unknown decision or an empty `body` fails the
//! job (the work item moves to `Failed` and can be retried from the UI).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::JobKind;

pub const TRIAGE: &str = include_str!("../../prompts/triage.md");
pub const PLAN: &str = include_str!("../../prompts/plan.md");
pub const IMPLEMENT: &str = include_str!("../../prompts/implement.md");
pub const REVIEW: &str = include_str!("../../prompts/review.md");
pub const ADDRESS_COMMENTS: &str = include_str!("../../prompts/address_comments.md");
pub const RESOLVE_CONFLICTS: &str = include_str!("../../prompts/resolve_conflicts.md");
pub const TASK: &str = include_str!("../../prompts/task.md");

/// Values for the `{placeholders}` in the templates. Empty values render as
/// `(none)`. Everything here except `thread` is known to the pure machine; the
/// executor fills `thread` from `Connector::fetch_thread` before re-rendering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptContext {
    /// `{bot_handle}`: the handle people mention.
    pub bot_handle: String,
    /// `{repo}`: repo key, e.g. `github:enzarb/enzarb-ui`.
    pub repo: String,
    /// `{repo_name}`: `owner/repo`.
    pub repo_name: String,
    /// `{item_kind}`: `issue` or `change request`.
    pub item_kind: String,
    /// `{number}`.
    pub number: String,
    /// `{url}`.
    pub url: String,
    /// `{title}`.
    pub title: String,
    /// `{body}`: the item description.
    pub body: String,
    /// `{author}`.
    pub author: String,
    /// `{thread}`: the comments so far, oldest first.
    pub thread: String,
    /// `{trigger}`: the comment, review or mention that started this run.
    pub trigger: String,
    /// `{triage_summary}`: the triage summary (plan and implement runs).
    pub triage_summary: String,
    /// `{plan}`: the latest plan (re-plan and implement runs).
    pub plan: String,
    /// `{base_ref}`.
    pub base_ref: String,
    /// `{branch}`: the branch to commit to.
    pub branch: String,
    /// `{head_ref}`: the change request's source branch.
    pub head_ref: String,
    /// `{head_sha}`.
    pub head_sha: String,
}

impl PromptContext {
    fn lookup(&self, name: &str) -> Option<&str> {
        Some(match name {
            "bot_handle" => &self.bot_handle,
            "repo" => &self.repo,
            "repo_name" => &self.repo_name,
            "item_kind" => &self.item_kind,
            "number" => &self.number,
            "url" => &self.url,
            "title" => &self.title,
            "body" => &self.body,
            "author" => &self.author,
            "thread" => &self.thread,
            "trigger" => &self.trigger,
            "triage_summary" => &self.triage_summary,
            "plan" => &self.plan,
            "base_ref" => &self.base_ref,
            "branch" => &self.branch,
            "head_ref" => &self.head_ref,
            "head_sha" => &self.head_sha,
            _ => return None,
        })
    }
}

/// The template for an agent job kind.
pub fn template(kind: JobKind) -> Option<&'static str> {
    Some(match kind {
        JobKind::AgentTriage => TRIAGE,
        JobKind::AgentPlan => PLAN,
        JobKind::AgentImplement => IMPLEMENT,
        JobKind::AgentReview => REVIEW,
        JobKind::AgentAddressComments => ADDRESS_COMMENTS,
        JobKind::AgentResolveConflicts => RESOLVE_CONFLICTS,
        JobKind::AgentTask => TASK,
        _ => return None,
    })
}

/// Substitute `{name}` placeholders in one pass (substituted values are never
/// re-scanned). Unknown `{...}` text, including JSON braces, is left as is.
pub fn render(template: &str, ctx: &PromptContext) -> String {
    let mut out = String::with_capacity(template.len() + 512);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let name_len = after
            .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_len];
        match (after[name_len..].starts_with('}'), ctx.lookup(name)) {
            (true, Some(v)) if !name.is_empty() => {
                out.push_str(if v.trim().is_empty() { "(none)" } else { v });
                rest = &after[name_len + 1..];
            }
            _ => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Render the prompt for an agent job kind. Non-agent kinds render an empty string.
pub fn render_prompt(kind: JobKind, ctx: &PromptContext) -> String {
    template(kind).map(|t| render(t, ctx)).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Output parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutput {
    pub decision: String,
    pub body: String,
    /// Conventional Commit PR title; only read for `done` on implement jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Checks the agent ran (review only). Absent means unverified.
    #[serde(default)]
    pub checks: Vec<CheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub name: String,
    /// `passed`, `failed` or `not_run`; anything but `passed` is unverified.
    pub status: String,
}

impl AgentOutput {
    /// True when at least one check was reported and every one passed.
    pub fn checks_verified(&self) -> bool {
        !self.checks.is_empty() && self.checks.iter().all(|c| c.status == "passed")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputError {
    /// The job output has no `summary` string.
    NoSummary,
    /// The message does not end with a fenced `json` block.
    NoJsonBlock,
    /// The block is not `{ "decision": str, "body": str }`.
    BadJson(String),
    UnknownDecision {
        kind: JobKind,
        decision: String,
    },
    EmptyBody,
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputError::NoSummary => write!(f, "the run produced no result text"),
            OutputError::NoJsonBlock => {
                write!(f, "the final message does not end with a fenced json block")
            }
            OutputError::BadJson(e) => write!(f, "the json block is invalid: {e}"),
            OutputError::UnknownDecision { kind, decision } => {
                write!(
                    f,
                    "decision {decision:?} is not valid for {}",
                    kind.as_str()
                )
            }
            OutputError::EmptyBody => write!(f, "the json block has an empty body"),
        }
    }
}
impl std::error::Error for OutputError {}

/// Typed view of the decisions, for the machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Questions,
    Ready,
    Duplicate,
    Plan,
    Done,
    Blocked,
    Approve,
    RequestChanges,
    Comment,
    Reply,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Questions => "questions",
            Decision::Ready => "ready",
            Decision::Duplicate => "duplicate",
            Decision::Plan => "plan",
            Decision::Done => "done",
            Decision::Blocked => "blocked",
            Decision::Approve => "approve",
            Decision::RequestChanges => "request_changes",
            Decision::Comment => "comment",
            Decision::Reply => "reply",
        }
    }

    /// The decisions an agent job kind may return.
    pub fn allowed(kind: JobKind) -> &'static [Decision] {
        use Decision::*;
        match kind {
            JobKind::AgentTriage => &[Questions, Ready, Duplicate],
            JobKind::AgentPlan => &[Plan],
            JobKind::AgentImplement
            | JobKind::AgentAddressComments
            | JobKind::AgentResolveConflicts => &[Done, Blocked],
            JobKind::AgentReview => &[Approve, RequestChanges, Comment],
            JobKind::AgentTask => &[Reply, Blocked],
            _ => &[],
        }
    }
}

impl AgentOutput {
    /// Parse the trailing fenced `json` block of `text`.
    ///
    /// The closing fence must be the last thing in the message. Backticks inside
    /// `body` are fine: openers are tried last to first until one parses.
    pub fn parse(text: &str) -> Result<Self, OutputError> {
        let trimmed = text.trim_end();
        let inner_end = trimmed
            .strip_suffix("```")
            .ok_or(OutputError::NoJsonBlock)?
            .len();
        let head = &trimmed[..inner_end];
        let mut last_err = None;
        let mut search = head.len();
        while let Some(i) = head[..search].rfind("```json") {
            search = i;
            let at_line_start = i == 0 || head.as_bytes()[i - 1] == b'\n';
            if !at_line_start {
                continue;
            }
            let inner = &head[i + "```json".len()..];
            match serde_json::from_str::<AgentOutput>(inner.trim()) {
                Ok(out) => return Ok(out),
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        Err(match last_err {
            Some(e) => OutputError::BadJson(e),
            None => OutputError::NoJsonBlock,
        })
    }

    /// Parse and validate an agent job's `jobs.output` for `kind`.
    pub fn from_job_output(kind: JobKind, output: &Value) -> Result<(Decision, Self), OutputError> {
        let text = output
            .get("summary")
            .and_then(Value::as_str)
            .ok_or(OutputError::NoSummary)?;
        let out = Self::parse(text)?;
        let decision = Decision::allowed(kind)
            .iter()
            .copied()
            .find(|d| d.as_str() == out.decision)
            .ok_or_else(|| OutputError::UnknownDecision {
                kind,
                decision: out.decision.clone(),
            })?;
        if out.body.trim().is_empty() {
            return Err(OutputError::EmptyBody);
        }
        Ok((decision, out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AGENT_KINDS: [JobKind; 7] = [
        JobKind::AgentTriage,
        JobKind::AgentPlan,
        JobKind::AgentImplement,
        JobKind::AgentReview,
        JobKind::AgentAddressComments,
        JobKind::AgentResolveConflicts,
        JobKind::AgentTask,
    ];

    fn full_ctx() -> PromptContext {
        PromptContext {
            bot_handle: "enzarb".into(),
            repo: "github:enzarb/enzarb-ui".into(),
            repo_name: "enzarb/enzarb-ui".into(),
            item_kind: "issue".into(),
            number: "42".into(),
            url: "https://example.test/42".into(),
            title: "TITLE-X".into(),
            body: "BODY-X".into(),
            author: "ruckc".into(),
            thread: "THREAD-X".into(),
            trigger: "TRIGGER-X".into(),
            triage_summary: "SUMMARY-X".into(),
            plan: "PLAN-X".into(),
            base_ref: "main".into(),
            branch: "enzarb/issue-42".into(),
            head_ref: "feature".into(),
            head_sha: "abc123".into(),
        }
    }

    /// Every `{name}` in a template must be a known placeholder.
    fn placeholders(t: &str) -> Vec<String> {
        let mut v = Vec::new();
        let mut rest = t;
        while let Some(i) = rest.find('{') {
            let after = &rest[i + 1..];
            let n = after
                .find(|c: char| !(c.is_ascii_lowercase() || c == '_'))
                .unwrap_or(after.len());
            if n > 0 && after[n..].starts_with('}') {
                v.push(after[..n].to_string());
            }
            rest = after;
        }
        v
    }

    #[test]
    fn templates_use_only_known_placeholders_and_render_fully() {
        let ctx = full_ctx();
        for kind in AGENT_KINDS {
            let t = template(kind).unwrap();
            for p in placeholders(t) {
                assert!(ctx.lookup(&p).is_some(), "{kind:?}: unknown {{{p}}}");
            }
            let out = render_prompt(kind, &ctx);
            for p in placeholders(t) {
                assert!(!out.contains(&format!("{{{p}}}")), "{kind:?}: {p} left");
            }
            // The contract is part of every prompt.
            assert!(out.contains("```json"), "{kind:?} lacks the output block");
            assert!(out.contains("\"decision\""), "{kind:?}");
            for d in Decision::allowed(kind) {
                assert!(out.contains(d.as_str()), "{kind:?} omits {d:?}");
            }
        }
    }

    #[test]
    fn issue_templates_carry_the_item_context() {
        let ctx = full_ctx();
        for kind in [
            JobKind::AgentTriage,
            JobKind::AgentPlan,
            JobKind::AgentImplement,
        ] {
            let out = render_prompt(kind, &ctx);
            for needle in ["TITLE-X", "BODY-X", "enzarb/enzarb-ui", "ruckc"] {
                assert!(out.contains(needle), "{kind:?} lacks {needle}");
            }
        }
        let implement = render_prompt(JobKind::AgentImplement, &ctx);
        assert!(implement.contains("enzarb/issue-42") && implement.contains("PLAN-X"));
        assert!(implement.contains("Do not push"), "implement must not push");
    }

    #[test]
    fn empty_values_render_as_none_and_values_are_not_rescanned() {
        let ctx = PromptContext {
            title: "{body}".into(),
            ..Default::default()
        };
        let out = render("t={title} b={body} {x} {", &ctx);
        assert_eq!(out, "t={body} b=(none) {x} {");
    }

    #[test]
    fn prompts_instruct_running_the_repo_checks() {
        let ctx = full_ctx();
        for kind in [
            JobKind::AgentImplement,
            JobKind::AgentAddressComments,
            JobKind::AgentResolveConflicts,
            JobKind::AgentReview,
        ] {
            let out = render_prompt(kind, &ctx);
            assert!(out.contains("mise.toml") && out.contains("cargo clippy --all-targets"));
            assert!(out.contains("no `&&` or `;` chains"));
        }
        let review = render_prompt(JobKind::AgentReview, &ctx);
        assert!(review.contains("must NOT be `approve`") && review.contains("\"checks\""));
    }

    #[test]
    fn checks_default_to_unverified() {
        let out =
            AgentOutput::parse("```json\n{\"decision\":\"approve\",\"body\":\"x\"}\n```").unwrap();
        assert!(out.checks.is_empty() && !out.checks_verified());
    }

    #[test]
    fn non_agent_kinds_have_no_prompt() {
        assert_eq!(render_prompt(JobKind::PostComment, &full_ctx()), "");
        assert!(template(JobKind::SetProgress).is_none());
        assert!(Decision::allowed(JobKind::OpenChangeRequest).is_empty());
    }

    #[test]
    fn parses_trailing_block() {
        let text =
            "Did the thing.\n\n```json\n{ \"decision\": \"ready\", \"body\": \"ok\" }\n```\n";
        let out = AgentOutput::parse(text).unwrap();
        assert_eq!(out.decision, "ready");
        assert_eq!(out.body, "ok");
    }

    #[test]
    fn title_is_optional() {
        let with = "```json\n{\"decision\":\"done\",\"body\":\"b\",\"title\":\"fix(a): b\"}\n```";
        assert_eq!(
            AgentOutput::parse(with).unwrap().title.as_deref(),
            Some("fix(a): b")
        );
        let without = "```json\n{\"decision\":\"done\",\"body\":\"b\"}\n```";
        assert_eq!(AgentOutput::parse(without).unwrap().title, None);
    }

    #[test]
    fn body_may_contain_fences_and_json_openers() {
        let text =
            "x\n```json\n{\"decision\":\"plan\",\"body\":\"a\\n```json\\n{}\\n```\\nb\"}\n```";
        let out = AgentOutput::parse(text).unwrap();
        assert_eq!(out.body, "a\n```json\n{}\n```\nb");
    }

    #[test]
    fn the_last_block_wins() {
        let text = "```json\n{\"decision\":\"plan\",\"body\":\"old\"}\n```\nmore\n```json\n{\"decision\":\"plan\",\"body\":\"new\"}\n```";
        assert_eq!(AgentOutput::parse(text).unwrap().body, "new");
    }

    #[test]
    fn rejects_malformed_output() {
        assert_eq!(
            AgentOutput::parse("no block"),
            Err(OutputError::NoJsonBlock)
        );
        // Text after the block.
        let t = "```json\n{\"decision\":\"plan\",\"body\":\"x\"}\n```\nthanks!";
        assert_eq!(AgentOutput::parse(t), Err(OutputError::NoJsonBlock));
        // Fence that is not json.
        let t = "```\n{\"decision\":\"plan\",\"body\":\"x\"}\n```";
        assert_eq!(AgentOutput::parse(t), Err(OutputError::NoJsonBlock));
        // Opener not at line start.
        let t = "see ```json\n{\"decision\":\"plan\",\"body\":\"x\"}\n```";
        assert_eq!(AgentOutput::parse(t), Err(OutputError::NoJsonBlock));
        let t = "```json\n{\"decision\": 1}\n```";
        assert!(matches!(
            AgentOutput::parse(t),
            Err(OutputError::BadJson(_))
        ));
    }

    #[test]
    fn validates_decisions_per_kind() {
        let ok = |d: &str, b: &str| json!({ "summary": format!("hi\n```json\n{{\"decision\":\"{d}\",\"body\":\"{b}\"}}\n```") });
        let (d, o) = AgentOutput::from_job_output(JobKind::AgentTriage, &ok("ready", "s")).unwrap();
        assert_eq!((d, o.body.as_str()), (Decision::Ready, "s"));
        assert_eq!(
            AgentOutput::from_job_output(JobKind::AgentPlan, &ok("ready", "s")),
            Err(OutputError::UnknownDecision {
                kind: JobKind::AgentPlan,
                decision: "ready".into()
            })
        );
        assert_eq!(
            AgentOutput::from_job_output(JobKind::AgentPlan, &ok("plan", " ")),
            Err(OutputError::EmptyBody)
        );
        assert_eq!(
            AgentOutput::from_job_output(JobKind::AgentPlan, &json!({})),
            Err(OutputError::NoSummary)
        );
        for e in [
            OutputError::NoSummary,
            OutputError::NoJsonBlock,
            OutputError::BadJson("x".into()),
            OutputError::EmptyBody,
        ] {
            assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn every_decision_has_a_distinct_name() {
        let mut seen = std::collections::HashSet::new();
        for kind in AGENT_KINDS {
            for d in Decision::allowed(kind) {
                seen.insert(d.as_str());
            }
        }
        assert_eq!(seen.len(), 10);
    }
}
