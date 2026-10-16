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
//! The block is the **last thing** in the message: nothing may follow the closing fence (no
//! sign-off, no second block). `body` is a JSON string holding markdown;
//! it is what gets posted (a comment, or the pull request description for
//! `implement`). The machine reads it from the job output's `summary` (Claude's
//! result text, as delivered by `RunFinished`). Allowed `decision` values:
//!
//! | job kind | decisions |
//! |---|---|
//! | `agent_triage` | `questions` (body: the questions to post), `ready` (body: summary for planning), `epic` (body: summary for planning; too big for one pull request), `duplicate` (body: comment pointing at the original) |
//! | `agent_plan` | `plan` (body: the markdown plan; for an epic, the breakdown with an `enzarb-subissues` block, see [`parse_epic_plan`]) |
//! | `agent_implement` | `done` (body: pull request description), `blocked` (body: why) |
//! | `agent_review` | `approve`, `request_changes`, `comment` (body: the review) |
//! | `agent_address_comments` | `done` (body: what changed), `blocked` (body: why) |
//! | `agent_resolve_conflicts` | `done` (body: how it was resolved), `blocked` (body: the conflict) |
//! | `agent_task` | `reply`, `blocked` (body: the reply to post) |
//!
//! A missing block or invalid JSON first gets one recovery run (see [`recovery_prompt`]); if
//! that also fails, as do an unknown decision or an empty `body`, the job fails (the work item
//! moves to `Failed` and can be retried from the UI).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::JobKind;

pub const TRIAGE: &str = include_str!("../../prompts/triage.md");
pub const PLAN: &str = include_str!("../../prompts/plan.md");
pub const PLAN_EPIC: &str = include_str!("../../prompts/plan_epic.md");
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
    /// `{merge_base_sha}`: the pinned merge base of the source branch and `{base_ref}`.
    pub merge_base_sha: String,
    /// `{changed_files}`: the files the source branch changes since the merge base, one per
    /// line (capped by the sandbox, with a trailing note when the list is cut short).
    pub changed_files: String,
    /// `{affected}`: the components the change reaches, one directory per line.
    pub affected: String,
    /// `{branch}`: the branch to commit to.
    pub branch: String,
    /// `{head_ref}`: the change request's source branch.
    pub head_ref: String,
    /// `{head_sha}`.
    pub head_sha: String,
    /// Plan the issue as an epic: the plan run uses `plan_epic.md`.
    pub epic: bool,
    /// `{epic_allowed}`: `yes` or `no`; whether triage may answer `epic`.
    pub epic_allowed: String,
    /// `{max_subissues}`: the most sub-issues an epic breakdown may list.
    pub max_subissues: String,
    /// `{dependencies}`: what the item waits on (number, title, state), from the
    /// `dependency_status` view; filled by the executor like `thread`.
    pub dependencies: String,
    /// The round was started because CI failed on the head: the executor gives the run a
    /// read-only token so `gh run view --log-failed` works. Not a placeholder.
    pub ci_fix: bool,
    /// `{ci_failure}`: the failing checks of a CI-fix round (name, conclusion, url, run and
    /// job id).
    pub ci_failure: String,
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
            "merge_base_sha" => &self.merge_base_sha,
            "changed_files" => &self.changed_files,
            "affected" => &self.affected,
            "branch" => &self.branch,
            "head_ref" => &self.head_ref,
            "head_sha" => &self.head_sha,
            "epic_allowed" => &self.epic_allowed,
            "max_subissues" => &self.max_subissues,
            "dependencies" => &self.dependencies,
            "ci_failure" => &self.ci_failure,
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

/// The job kind whose template a graph `agent` node names with its `prompt` (ADR-0020):
/// `triage`, `plan`, `implement`, `review`, `address_comments`, `resolve_conflicts`, `task`
/// (and `plan_epic`, the breakdown plan, in [`render_graph_prompt`]).
pub fn named_prompt(name: &str) -> Option<JobKind> {
    Some(match name {
        "triage" => JobKind::AgentTriage,
        "plan" => JobKind::AgentPlan,
        "implement" => JobKind::AgentImplement,
        "review" => JobKind::AgentReview,
        "address_comments" => JobKind::AgentAddressComments,
        "resolve_conflicts" => JobKind::AgentResolveConflicts,
        "task" => JobKind::AgentTask,
        _ => return None,
    })
}

/// The text of a graph `agent` node's prompt: a named template, or the inline text with
/// the same `{placeholders}` substituted.
pub fn render_graph_prompt(prompt: &str, ctx: &PromptContext) -> String {
    if prompt.trim() == "plan_epic" {
        let epic = PromptContext {
            epic: true,
            ..ctx.clone()
        };
        return render_prompt(JobKind::AgentPlan, &epic);
    }
    match named_prompt(prompt.trim()) {
        Some(kind) => render_prompt(kind, ctx),
        None => render(prompt, ctx),
    }
}

/// The short prompt for the one recovery run of a job whose final message had no usable
/// `json` block. It is a re-run (the sandbox protocol has no way to resume the session), so it
/// tells the agent to change nothing and only report. Non-agent kinds get an empty string.
pub fn recovery_prompt(kind: JobKind) -> String {
    let decisions = Decision::allowed(kind);
    if decisions.is_empty() {
        return String::new();
    }
    let names = decisions
        .iter()
        .map(|d| format!("`{}`", d.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let example = match kind {
        JobKind::AgentImplement => {
            r###"{ "decision": "done", "title": "fix(scope): short description", "body": "## Summary\n..." }"###
        }
        JobKind::AgentReview => {
            r###"{ "decision": "comment", "body": "## Review\n...", "checks": [{ "name": "cargo test", "status": "not_run" }] }"###
        }
        _ => r#"{ "decision": "<decision>", "body": "<markdown>" }"#,
    };
    format!(
        "The work for this item was already done in an earlier run, but its final message did \
not end with a valid fenced `json` block, so the result could not be read.\n\n\
Do NOT redo the work. Make no changes, commits or pushes. If you need to remember what was \
done, look at the current branch and its commits; if there is nothing to go on, say what you \
can and use a cautious decision.\n\n\
# Required final message\n\n\
Reply with a short plain-text note, then end your message with exactly one fenced `json` \
block, and nothing after it:\n\n\
```json\n{example}\n```\n\n\
`decision` is one of {names}. `body` is a single JSON string holding markdown (escape newlines \
as `\\n` and quotes as `\\\"`). The block must be the very last thing in your message: write \
nothing after the closing fence.\n"
    )
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

/// Render the prompt for an agent job kind. Non-agent kinds render an empty string. The plan
/// of an epic (`ctx.epic`) uses its own template.
pub fn render_prompt(kind: JobKind, ctx: &PromptContext) -> String {
    let t = match kind {
        JobKind::AgentPlan if ctx.epic => Some(PLAN_EPIC),
        _ => template(kind),
    };
    t.map(|t| render(t, ctx)).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Epic breakdown
// ---------------------------------------------------------------------------

/// One sub-issue of an epic, from the `enzarb-subissues` block of the approved plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubIssue {
    /// Unique id within the plan, `[a-z0-9_-]{1,32}`; only used by `depends_on`.
    pub key: String,
    pub title: String,
    pub body: String,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpicPlanError {
    NoBlock,
    BadJson(String),
    Empty,
    TooMany {
        max: usize,
    },
    BadKey(String),
    DuplicateKey(String),
    EmptyTitle(String),
    UnknownDependency {
        key: String,
        depends_on: String,
    },
    Cycle(String),
    /// `{{issue:target}}` in the title or body of `key`, but no sub-issue has that key.
    UnknownRef {
        key: String,
        target: String,
    },
    /// A `#<digits>` in a sub-issue; the numbers are only known after creation.
    StrayIssueRef {
        key: String,
        text: String,
    },
    /// A title may not hold an issue reference or a placeholder.
    RefInTitle(String),
}

impl std::fmt::Display for EpicPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EpicPlanError::NoBlock => {
                write!(f, "the plan has no `enzarb-subissues` block")
            }
            EpicPlanError::BadJson(e) => write!(f, "the `enzarb-subissues` block is invalid: {e}"),
            EpicPlanError::Empty => write!(f, "the epic breakdown lists no sub-issues"),
            EpicPlanError::TooMany { max } => {
                write!(f, "the epic breakdown lists more than {max} sub-issues")
            }
            EpicPlanError::BadKey(k) => write!(f, "sub-issue key {k:?} is not [a-z0-9_-]{{1,32}}"),
            EpicPlanError::DuplicateKey(k) => write!(f, "sub-issue key {k:?} is used twice"),
            EpicPlanError::EmptyTitle(k) => write!(f, "sub-issue {k:?} has an empty title"),
            EpicPlanError::UnknownDependency { key, depends_on } => {
                write!(f, "sub-issue {key:?} depends on unknown key {depends_on:?}")
            }
            EpicPlanError::Cycle(k) => write!(f, "sub-issue {k:?} is on a dependency cycle"),
            EpicPlanError::UnknownRef { key, target } => write!(
                f,
                "sub-issue {key:?} refers to {{{{issue:{target}}}}}, but no sub-issue has the key {target:?}"
            ),
            EpicPlanError::StrayIssueRef { key, text } => write!(
                f,
                "sub-issue {key:?} contains the issue number {text:?}: issue numbers are only known after creation, so refer to a sibling as {{{{issue:key}}}} and do not write #<number>"
            ),
            EpicPlanError::RefInTitle(k) => write!(
                f,
                "the title of sub-issue {k:?} contains an issue number or {{{{issue:key}}}} placeholder"
            ),
        }
    }
}
impl std::error::Error for EpicPlanError {}

const REF_OPEN: &str = "{{issue:";

/// The keys named by `{{issue:key}}` placeholders in `text`, in order of appearance.
pub fn issue_refs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(REF_OPEN) {
        let after = &rest[i + REF_OPEN.len()..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

/// `text` with every `{{issue:key}}` replaced by `f(key)`; a placeholder `f` declines
/// (returns `None`) is kept as written.
pub fn substitute_refs(text: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(REF_OPEN) {
        let after = &rest[i + REF_OPEN.len()..];
        let Some(end) = after.find("}}") else { break };
        out.push_str(&rest[..i]);
        match f(after[..end].trim()) {
            Some(s) => out.push_str(&s),
            None => out.push_str(&rest[i..i + REF_OPEN.len() + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// `text` with fenced code blocks and inline code spans blanked out.
fn without_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut fence: Option<&str> = None;
    for line in text.lines() {
        let t = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|m| t.starts_with(m));
        match (fence, marker) {
            (Some(open), Some(m)) if open == m => fence = None,
            (None, Some(m)) => fence = Some(m),
            (Some(_), _) => {}
            _ => {
                let mut in_code = false;
                for c in line.chars() {
                    if c == '`' {
                        in_code = !in_code;
                    } else if !in_code {
                        out.push(c);
                    }
                }
            }
        }
        out.push('\n');
    }
    out
}

/// The first `#<digits>` in `text` outside code that is not part of a word (`C#1`, `a/b#1`).
fn stray_issue_ref(text: &str) -> Option<String> {
    let text = without_code(text);
    let mut prev: Option<char> = None;
    for (i, c) in text.char_indices() {
        if c == '#' && prev.is_none_or(|p| !p.is_alphanumeric() && p != '/') {
            let digits: String = text[i + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if !digits.is_empty() {
                return Some(format!("#{digits}"));
            }
        }
        prev = Some(c);
    }
    None
}

const EPIC_BLOCK: &str = "```enzarb-subissues";

fn valid_sub_key(k: &str) -> bool {
    (1..=32).contains(&k.len())
        && k.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Parse and validate the last `enzarb-subissues` block of an epic plan body.
///
/// The list must have 1 to `max` entries with unique `[a-z0-9_-]{1,32}` keys and non-empty
/// titles; every `depends_on` names another entry; the dependencies have no cycle. The result
/// is in a creation order where every sub-issue comes after what it depends on (the plan's
/// own order is kept wherever that allows it), so numbers of blockers exist when needed.
pub fn parse_epic_plan(body: &str, max: usize) -> Result<Vec<SubIssue>, EpicPlanError> {
    let mut json = None;
    let mut rest = body;
    while let Some(i) = rest.find(EPIC_BLOCK) {
        let at_line_start = i == 0 || rest.as_bytes()[i - 1] == b'\n';
        let after = &rest[i + EPIC_BLOCK.len()..];
        rest = after;
        // The info string must end the line: not "```enzarb-subissues2".
        if !at_line_start || !after.starts_with(['\n', '\r']) {
            continue;
        }
        if let Some(end) = after.find("\n```") {
            json = Some(&after[..end]);
        }
    }
    let json = json.ok_or(EpicPlanError::NoBlock)?;
    let mut subs: Vec<SubIssue> =
        serde_json::from_str(json.trim()).map_err(|e| EpicPlanError::BadJson(e.to_string()))?;
    if subs.is_empty() {
        return Err(EpicPlanError::Empty);
    }
    if subs.len() > max {
        return Err(EpicPlanError::TooMany { max });
    }
    let mut seen = std::collections::HashSet::new();
    for s in &mut subs {
        s.title = s.title.trim().to_string();
        s.body = s.body.trim().to_string();
        if !valid_sub_key(&s.key) {
            return Err(EpicPlanError::BadKey(s.key.clone()));
        }
        if !seen.insert(s.key.clone()) {
            return Err(EpicPlanError::DuplicateKey(s.key.clone()));
        }
        if s.title.is_empty() {
            return Err(EpicPlanError::EmptyTitle(s.key.clone()));
        }
        if s.title.contains('\n') || s.title.chars().count() > 200 {
            return Err(EpicPlanError::EmptyTitle(s.key.clone()));
        }
        s.depends_on.sort();
        s.depends_on.dedup();
    }
    for s in &subs {
        for d in &s.depends_on {
            if !seen.contains(d) || *d == s.key {
                return if d == &s.key {
                    Err(EpicPlanError::Cycle(s.key.clone()))
                } else {
                    Err(EpicPlanError::UnknownDependency {
                        key: s.key.clone(),
                        depends_on: d.clone(),
                    })
                };
            }
        }
    }
    for s in &subs {
        if stray_issue_ref(&s.title).is_some() || !issue_refs(&s.title).is_empty() {
            return Err(EpicPlanError::RefInTitle(s.key.clone()));
        }
        for target in issue_refs(&s.body) {
            if !seen.contains(target) {
                return Err(EpicPlanError::UnknownRef {
                    key: s.key.clone(),
                    target: target.to_string(),
                });
            }
        }
        if let Some(text) = stray_issue_ref(&s.body) {
            return Err(EpicPlanError::StrayIssueRef {
                key: s.key.clone(),
                text,
            });
        }
    }
    // Kahn's algorithm, picking the earliest remaining entry each round.
    let mut ordered: Vec<SubIssue> = Vec::with_capacity(subs.len());
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut remaining = subs;
    while !remaining.is_empty() {
        let Some(i) = remaining
            .iter()
            .position(|s| s.depends_on.iter().all(|d| done.contains(d)))
        else {
            return Err(EpicPlanError::Cycle(remaining[0].key.clone()));
        };
        let s = remaining.remove(i);
        done.insert(s.key.clone());
        ordered.push(s);
    }
    Ok(ordered)
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
    /// Why a check was `not_run`: `refused`, `missing_tool`, `timeout`, `not_attempted` or
    /// `covered_by_ci`. Absent for older output and for checks that ran.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl CheckResult {
    /// A check skipped because CI runs it; counts as passed only while CI is green.
    fn covered_by_ci(&self) -> bool {
        self.status == "not_run" && self.reason == "covered_by_ci"
    }
}

impl AgentOutput {
    /// True when at least one check was reported and every one passed. A `not_run` check
    /// with reason `covered_by_ci` does not block while `ci_green` (CI succeeded on the
    /// head being reviewed); otherwise it does.
    pub fn checks_verified(&self, ci_green: bool) -> bool {
        !self.checks.is_empty()
            && self
                .checks
                .iter()
                .all(|c| c.status == "passed" || (ci_green && c.covered_by_ci()))
    }

    /// The checks that did not pass, for the "checks unverified" wait.
    pub fn unverified_checks(&self, ci_green: bool) -> Vec<&CheckResult> {
        self.checks
            .iter()
            .filter(|c| !(c.status == "passed" || (ci_green && c.covered_by_ci())))
            .collect()
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
    /// Triage: too big for one pull request.
    Epic,
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
            Decision::Epic => "epic",
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
            JobKind::AgentTriage => &[Questions, Ready, Epic, Duplicate],
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
            merge_base_sha: "MBASE-X".into(),
            changed_files: "FILES-X".into(),
            affected: "AFFECTED-X".into(),
            branch: "enzarb/issue-42".into(),
            head_ref: "feature".into(),
            head_sha: "abc123".into(),
            epic: false,
            epic_allowed: "yes".into(),
            max_subissues: "8".into(),
            dependencies: "DEPS-X".into(),
            ci_fix: false,
            ci_failure: "CI-FAILURE-X".into(),
        }
    }

    #[test]
    fn address_prompt_reads_the_ci_log_with_gh_and_reproduces_one_test() {
        let out = render_prompt(JobKind::AgentAddressComments, &full_ctx());
        assert!(out.contains("CI-FAILURE-X"));
        assert!(out.contains("gh run view <run id> --log-failed"));
        assert!(out.contains("cargo nextest run -E 'test(<name>)'"));
        assert!(out.contains("cargo test --test <file> <name>"));
        assert!(out.contains("Do not run the whole workspace"));
        assert!(out.contains("`blocked` body"));
        // Outside a CI-fix round the section renders `(none)`.
        let ctx = PromptContext {
            ci_failure: String::new(),
            ..full_ctx()
        };
        let out = render_prompt(JobKind::AgentAddressComments, &ctx);
        assert!(out.contains("<untrusted>\n(none)\n</untrusted>"));
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
    fn prompts_diff_against_the_merge_base_and_check_the_affected_components() {
        let ctx = full_ctx();
        for kind in [JobKind::AgentReview, JobKind::AgentAddressComments] {
            let out = render_prompt(kind, &ctx);
            assert!(out.contains("MBASE-X...HEAD"), "{kind:?}");
            assert!(!out.contains("main...HEAD"), "{kind:?}");
        }
        for kind in [
            JobKind::AgentReview,
            JobKind::AgentAddressComments,
            JobKind::AgentResolveConflicts,
            JobKind::AgentImplement,
        ] {
            let out = render_prompt(kind, &ctx);
            assert!(out.contains("AFFECTED-X"), "{kind:?}");
            assert!(!out.contains("affected.sh"), "{kind:?}");
        }
        let review = render_prompt(JobKind::AgentReview, &ctx);
        assert!(review.contains("never inspect, fetch or update the local `main`"));
        assert!(review.contains("Name the components you skipped as unaffected"));
        assert!(review.contains("`FILES-X`"));
    }

    #[test]
    fn old_contexts_without_the_prepared_fields_still_decode() {
        let old = serde_json::json!({"repo": "github:o/r", "base_ref": "main", "affected": "ui"});
        let ctx: PromptContext = serde_json::from_value(old).unwrap();
        assert_eq!(ctx.changed_files, "");
        assert_eq!(ctx.merge_base_sha, "");
        assert_eq!(ctx.affected, "ui");
    }

    #[test]
    fn missing_merge_base_and_affected_render_with_a_fallback() {
        let ctx = PromptContext {
            base_ref: "main".into(),
            ..Default::default()
        };
        for kind in [
            JobKind::AgentReview,
            JobKind::AgentAddressComments,
            JobKind::AgentResolveConflicts,
            JobKind::AgentImplement,
        ] {
            let out = render_prompt(kind, &ctx);
            assert!(out.contains("is `(none)`, it was not provided"), "{kind:?}");
            assert!(out.contains("fall back to the components"), "{kind:?}");
        }
        let review = render_prompt(JobKind::AgentReview, &ctx);
        assert!(review.contains("git merge-base HEAD origin/main"));
    }

    #[test]
    fn implement_never_commits_unchecked_work_silently() {
        let out = render_prompt(JobKind::AgentImplement, &full_ctx());
        assert!(out.contains("checks of the affected components, `AFFECTED-X`"));
        assert!(out.contains("Never commit work that you could not check without saying so"));
        assert!(out.contains("\"Not run\""));
    }

    #[test]
    fn templates_end_with_the_final_block_instruction() {
        for kind in AGENT_KINDS {
            let out = render_prompt(kind, &full_ctx());
            let tail = out.rsplit("# Required final message").next().unwrap();
            assert!(out.contains("# Required final message"), "{kind:?}");
            assert!(tail.contains("last thing"), "{kind:?}");
            assert!(tail.contains("nothing after"), "{kind:?}");
            for d in Decision::allowed(kind) {
                assert!(tail.contains(d.as_str()), "{kind:?} omits {d:?}");
            }
        }
    }

    #[test]
    fn prompts_instruct_plain_git_and_formatter_fallback() {
        let ctx = full_ctx();
        for kind in [JobKind::AgentImplement, JobKind::AgentAddressComments] {
            let out = render_prompt(kind, &ctx);
            assert!(out.contains("without `-C`"));
            assert!(out.contains("Not run"));
            assert!(out.contains("cargo fmt"));
        }
    }

    #[test]
    fn recovery_prompt_names_every_decision_and_forbids_changes() {
        for kind in AGENT_KINDS {
            let p = recovery_prompt(kind);
            assert!(
                p.contains("```json") && p.contains("nothing after"),
                "{kind:?}"
            );
            assert!(p.contains("Make no changes, commits or pushes"), "{kind:?}");
            for d in Decision::allowed(kind) {
                assert!(
                    p.contains(&format!("`{}`", d.as_str())),
                    "{kind:?} omits {d:?}"
                );
            }
        }
        assert!(recovery_prompt(JobKind::AgentImplement).contains("\"title\""));
        assert!(recovery_prompt(JobKind::AgentReview).contains("\"checks\""));
        assert_eq!(recovery_prompt(JobKind::PostComment), "");
    }

    #[test]
    fn checks_default_to_unverified() {
        let out =
            AgentOutput::parse("```json\n{\"decision\":\"approve\",\"body\":\"x\"}\n```").unwrap();
        assert!(out.checks.is_empty() && !out.checks_verified(true));
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
        assert_eq!(seen.len(), 11);
    }

    const ALL_DECISIONS: [Decision; 11] = [
        Decision::Questions,
        Decision::Ready,
        Decision::Epic,
        Decision::Duplicate,
        Decision::Plan,
        Decision::Done,
        Decision::Blocked,
        Decision::Approve,
        Decision::RequestChanges,
        Decision::Comment,
        Decision::Reply,
    ];

    /// Every prompt (the epic plan too) says which decisions it accepts, and names no
    /// decision that its job kind would reject.
    #[test]
    fn each_prompt_names_exactly_its_allowed_decisions() {
        let mut ctx = full_ctx();
        let mut prompts: Vec<(String, JobKind, String)> = AGENT_KINDS
            .iter()
            .map(|k| (format!("{k:?}"), *k, render_prompt(*k, &ctx)))
            .collect();
        ctx.epic = true;
        prompts.push((
            "epic plan".into(),
            JobKind::AgentPlan,
            render_prompt(JobKind::AgentPlan, &ctx),
        ));
        for (name, kind, out) in prompts {
            let tail = out.rsplit("# Required final message").next().unwrap();
            let line = tail
                .lines()
                .find(|l| l.starts_with("`decision` "))
                .unwrap_or_else(|| panic!("{name}: no decision line"));
            let allowed = Decision::allowed(kind);
            for d in ALL_DECISIONS {
                let named = line.contains(&format!("`{}`", d.as_str()));
                assert_eq!(named, allowed.contains(&d), "{name}: {d:?} in {line:?}");
            }
        }
    }

    #[test]
    fn triage_may_answer_epic_and_says_when_not() {
        let mut ctx = full_ctx();
        ctx.epic_allowed = "no".into();
        let out = render_prompt(JobKind::AgentTriage, &ctx);
        assert!(Decision::allowed(JobKind::AgentTriage).contains(&Decision::Epic));
        assert!(out.contains("Epics allowed for this item: `no`"));
        assert!(!Decision::allowed(JobKind::AgentPlan).contains(&Decision::Epic));
        let parsed = AgentOutput::from_job_output(
            JobKind::AgentTriage,
            &json!({ "summary": "```json\n{\"decision\":\"epic\",\"body\":\"big\"}\n```" }),
        )
        .unwrap();
        assert_eq!(parsed.0, Decision::Epic);
    }

    #[test]
    fn epic_plan_prompt_is_used_only_for_epics() {
        let mut ctx = full_ctx();
        ctx.max_subissues = "8".into();
        let plain = render_prompt(JobKind::AgentPlan, &ctx);
        assert!(!plain.contains("enzarb-subissues"));
        ctx.epic = true;
        let epic = render_prompt(JobKind::AgentPlan, &ctx);
        assert!(epic.contains("enzarb-subissues") && epic.contains("at most 8"));
        assert!(epic.contains("TITLE-X") && epic.contains("SUMMARY-X") && epic.contains("PLAN-X"));
        for p in placeholders(PLAN_EPIC) {
            assert!(ctx.lookup(&p).is_some(), "unknown {{{p}}}");
            assert!(!epic.contains(&format!("{{{p}}}")), "{p} left");
        }
    }

    fn plan_with(block: &str) -> String {
        format!("## Summary\nsplit\n\n```enzarb-subissues\n{block}\n```\n\nReply with feedback.")
    }

    fn sub(key: &str, deps: &[&str]) -> String {
        let deps = deps
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"key\":\"{key}\",\"title\":\"T {key}\",\"body\":\"B\",\"depends_on\":[{deps}]}}"
        )
    }

    #[test]
    fn epic_plan_parses_and_orders_by_dependency() {
        let block = format!(
            "[{},{},{}]",
            sub("ui", &["api"]),
            sub("api", &[]),
            sub("docs", &[])
        );
        let subs = parse_epic_plan(&plan_with(&block), 8).unwrap();
        let keys: Vec<_> = subs.iter().map(|s| s.key.as_str()).collect();
        // `ui` waits for `api`; `docs` keeps its place after it.
        assert_eq!(keys, ["api", "ui", "docs"]);
        assert_eq!(subs[1].depends_on, ["api"]);
        assert_eq!(subs[0].title, "T api");
    }

    #[test]
    fn epic_plan_uses_the_last_block() {
        let first = plan_with(&format!("[{}]", sub("old", &[])));
        let both = format!("{first}\n\n```enzarb-subissues\n[{}]\n```", sub("new", &[]));
        assert_eq!(parse_epic_plan(&both, 8).unwrap()[0].key, "new");
    }

    #[test]
    fn epic_plan_rejects_bad_lists() {
        let cases: Vec<(String, EpicPlanError)> = vec![
            ("no block".into(), EpicPlanError::NoBlock),
            (plan_with("not json"), EpicPlanError::BadJson(String::new())),
            (plan_with("[]"), EpicPlanError::Empty),
            (
                plan_with(&format!(
                    "[{},{},{}]",
                    sub("a", &[]),
                    sub("b", &[]),
                    sub("c", &[])
                )),
                EpicPlanError::TooMany { max: 2 },
            ),
            (
                plan_with(&format!("[{}]", sub("Bad Key", &[]))),
                EpicPlanError::BadKey("Bad Key".into()),
            ),
            (
                plan_with(&format!("[{},{}]", sub("a", &[]), sub("a", &[]))),
                EpicPlanError::DuplicateKey("a".into()),
            ),
            (
                plan_with(r#"[{"key":"a","title":"  ","body":"x"}]"#),
                EpicPlanError::EmptyTitle("a".into()),
            ),
            (
                plan_with(&format!("[{}]", sub("a", &["zzz"]))),
                EpicPlanError::UnknownDependency {
                    key: "a".into(),
                    depends_on: "zzz".into(),
                },
            ),
            (
                plan_with(&format!("[{}]", sub("a", &["a"]))),
                EpicPlanError::Cycle("a".into()),
            ),
            (
                plan_with(&format!("[{},{}]", sub("a", &["b"]), sub("b", &["a"]))),
                EpicPlanError::Cycle("a".into()),
            ),
        ];
        for (plan, want) in cases {
            let got = parse_epic_plan(&plan, 2).unwrap_err();
            match (&got, &want) {
                (EpicPlanError::BadJson(_), EpicPlanError::BadJson(_)) => {}
                _ => assert_eq!(got, want, "plan {plan:?}"),
            }
            assert!(!got.to_string().is_empty());
        }
    }

    #[test]
    fn epic_block_info_string_must_match_exactly() {
        let plan = format!("```enzarb-subissues2\n[{}]\n```", sub("a", &[]));
        assert_eq!(parse_epic_plan(&plan, 8), Err(EpicPlanError::NoBlock));
        let quoted = format!("see ```enzarb-subissues\n[{}]\n```", sub("a", &[]));
        assert_eq!(parse_epic_plan(&quoted, 8), Err(EpicPlanError::NoBlock));
    }

    fn plan_of(subs: &[(&str, &str, &str)]) -> String {
        let v: Vec<_> = subs
            .iter()
            .map(|(k, t, b)| serde_json::json!({ "key": k, "title": t, "body": b }))
            .collect();
        plan_with(&serde_json::to_string(&v).unwrap())
    }

    #[test]
    fn placeholders_between_siblings_are_accepted() {
        let plan = plan_of(&[
            ("a", "A", "Pairs with {{issue:b}} and {{issue:a}}."),
            (
                "b",
                "B",
                "Needs {{ issue:a }}? No: `#12` and\n```\n#13\n```\nare code; C#1 too.",
            ),
        ]);
        let subs = parse_epic_plan(&plan, 8).unwrap();
        assert_eq!(issue_refs(&subs[0].body), ["b", "a"]);
    }

    #[test]
    fn unknown_placeholder_is_rejected() {
        let plan = plan_of(&[("a", "A", "See {{issue:zzz}}.")]);
        assert_eq!(
            parse_epic_plan(&plan, 8),
            Err(EpicPlanError::UnknownRef {
                key: "a".into(),
                target: "zzz".into()
            })
        );
    }

    #[test]
    fn stray_numbers_are_rejected() {
        let plan = plan_of(&[("a", "A", "Waits for #12 (storage).")]);
        assert_eq!(
            parse_epic_plan(&plan, 8),
            Err(EpicPlanError::StrayIssueRef {
                key: "a".into(),
                text: "#12".into()
            })
        );
        let plan = plan_of(&[("a", "Fix #7", "ok")]);
        assert_eq!(
            parse_epic_plan(&plan, 8),
            Err(EpicPlanError::RefInTitle("a".into()))
        );
        let plan = plan_of(&[("a", "A", "ok"), ("b", "See {{issue:a}}", "ok")]);
        assert_eq!(
            parse_epic_plan(&plan, 8),
            Err(EpicPlanError::RefInTitle("b".into()))
        );
    }

    #[test]
    fn substitute_refs_replaces_known_keys_only() {
        let got = substitute_refs("{{issue:a}} and {{issue:q}} {{issue:a}}", |k| {
            (k == "a").then(|| "#5 (A)".to_string())
        });
        assert_eq!(got, "#5 (A) and {{issue:q}} #5 (A)");
        assert_eq!(
            substitute_refs("no refs {{issue:", |_| None),
            "no refs {{issue:"
        );
    }
}
