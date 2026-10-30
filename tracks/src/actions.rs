//! `action` nodes (ADR-0029 section 2): one GitHub call with the App's installation token.
//!
//! * **Token.** Minted per call, for the one repository, with only the permissions the action
//!   needs (`issues: write` / `pull_requests: write`); it never enters the sandbox.
//! * **Idempotency.** The executor writes the node's intent (`node_runs.intent`) before calling.
//!   When it finds that row on a resume (`replay`), the call may already have happened, so each
//!   action first looks for its own effect: a comment carries a hidden marker
//!   (`<!-- ez-tracks:<run>:<node>:<visit> -->`) that is searched for in the subject's comments;
//!   `open_pull_request` looks up the pull request of the run branch (and treats GitHub's "already
//!   exists" answer the same way); labels are idempotent by nature.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ez_workflow::schema::ActionKind;
use ez_workflow::{Subject, SubjectKind};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};

use crate::github::{RepoSource, SourceError};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Comment pages searched for the marker on a replay (100 comments each).
const MAX_COMMENT_PAGES: u32 = 30;

/// Everything one action needs, already rendered.
#[derive(Debug, Clone)]
pub struct Call<'a> {
    pub repo: &'a str,
    /// The default branch (`open_pull_request` base).
    pub base: &'a str,
    /// The run branch (`open_pull_request` head).
    pub branch: &'a str,
    pub subject: Option<&'a Subject>,
    pub action: ActionKind,
    pub title: Option<&'a str>,
    pub body: Option<&'a str>,
    pub labels: &'a [String],
    pub draft: bool,
    /// Hidden marker identifying this node visit.
    pub marker: &'a str,
    /// The node was started before: look for its effect first.
    pub replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    /// Try again later.
    Transient(String),
    /// The node fails: retrying will not help.
    Permanent(String),
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(m) | Self::Permanent(m) => f.write_str(m),
        }
    }
}

impl From<SourceError> for ActionError {
    fn from(e: SourceError) -> Self {
        match e {
            SourceError::Transient(m) => Self::Transient(m),
            SourceError::Permanent(m) => Self::Permanent(m),
        }
    }
}

/// The permissions the installation token gets for `action` on `subject`.
pub fn permissions(
    action: ActionKind,
    subject: Option<&Subject>,
) -> Vec<(&'static str, &'static str)> {
    match action {
        ActionKind::OpenPullRequest => vec![("pull_requests", "write"), ("contents", "read")],
        _ => match subject.map(|s| s.kind) {
            Some(SubjectKind::PullRequest) => vec![("pull_requests", "write")],
            _ => vec![("issues", "write")],
        },
    }
}

/// The hidden marker of a node visit.
pub fn marker(run: &str, node: &str, visit: i32) -> String {
    format!("<!-- ez-tracks:{run}:{node}:{visit} -->")
}

#[async_trait]
pub trait Actions: Send + Sync {
    /// Performs the call; returns the node's outputs (`number`, `url`, ...).
    async fn perform(&self, call: &Call<'_>) -> Result<Value, ActionError>;
}

/// For installations without a GitHub App.
pub struct NoActions;

#[async_trait]
impl Actions for NoActions {
    async fn perform(&self, _: &Call<'_>) -> Result<Value, ActionError> {
        Err(ActionError::Permanent(
            "GitHub actions are not configured (no GitHub App)".into(),
        ))
    }
}

pub struct GithubActions {
    source: Arc<dyn RepoSource>,
    http: Client,
    base: String,
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-encodes one path segment (also `/`).
fn enc_seg(s: &str) -> String {
    enc(s).replace('/', "%2F")
}

fn transient(what: &str, e: impl std::fmt::Display) -> ActionError {
    ActionError::Transient(format!("{what}: {e}"))
}

impl GithubActions {
    pub fn new(source: Arc<dyn RepoSource>, api_base: &str) -> anyhow::Result<Self> {
        Ok(Self {
            source,
            http: Client::builder()
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("enzarb-tracks")
                .build()?,
            base: api_base.trim_end_matches('/').to_string(),
        })
    }

    /// One request; `Ok((status, body))` for 2xx and for the statuses in `allow`.
    async fn send(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
        allow: &[StatusCode],
        what: &str,
    ) -> Result<(StatusCode, Value), ActionError> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(|e| transient(what, e))?;
        let status = res.status();
        let text = res.text().await.map_err(|e| transient(what, e))?;
        let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status.is_success() || allow.contains(&status) {
            return Ok((status, json));
        }
        let detail = json
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let msg = format!("{what}: GitHub answered {status} {detail}")
            .trim()
            .to_string();
        let rate_limited =
            status == StatusCode::FORBIDDEN && detail.to_lowercase().contains("rate limit");
        Err(
            if status.is_server_error()
                || status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::UNAUTHORIZED
                || rate_limited
            {
                ActionError::Transient(msg)
            } else {
                ActionError::Permanent(msg)
            },
        )
    }

    async fn comment(&self, token: &str, c: &Call<'_>, n: u64) -> Result<Value, ActionError> {
        let Some(body) = c.body else {
            return Err(ActionError::Permanent("comment needs a body".into()));
        };
        let path = format!("/repos/{}/issues/{n}/comments", enc(c.repo));
        if c.replay {
            for page in 1..=MAX_COMMENT_PAGES {
                let (_, list) = self
                    .send(
                        token,
                        Method::GET,
                        &format!("{path}?per_page=100&page={page}"),
                        None,
                        &[],
                        "listing comments",
                    )
                    .await?;
                let items = list.as_array().cloned().unwrap_or_default();
                if let Some(found) = items.iter().find(|i| {
                    i.get("body")
                        .and_then(Value::as_str)
                        .is_some_and(|b| b.contains(c.marker))
                }) {
                    return Ok(comment_out(found, n));
                }
                if items.len() < 100 {
                    break;
                }
            }
        }
        let (_, created) = self
            .send(
                token,
                Method::POST,
                &path,
                Some(json!({ "body": format!("{body}\n\n{}", c.marker) })),
                &[],
                "creating the comment",
            )
            .await?;
        Ok(comment_out(&created, n))
    }

    async fn open_pull_request(&self, token: &str, c: &Call<'_>) -> Result<Value, ActionError> {
        let Some(title) = c.title else {
            return Err(ActionError::Permanent(
                "open_pull_request needs a title".into(),
            ));
        };
        let owner = c.repo.split('/').next().unwrap_or_default();
        let repo = enc(c.repo);
        // Commits on the branch? 404: the branch was never pushed.
        let (st, cmp) = self
            .send(
                token,
                Method::GET,
                &format!(
                    "/repos/{repo}/compare/{}...{}",
                    enc_seg(c.base),
                    enc_seg(c.branch)
                ),
                None,
                &[StatusCode::NOT_FOUND],
                "comparing the run branch",
            )
            .await?;
        if st == StatusCode::NOT_FOUND {
            return Err(ActionError::Permanent(format!(
                "branch {} has not been pushed: nothing to open a pull request for",
                c.branch
            )));
        }
        if cmp.get("ahead_by").and_then(Value::as_u64) == Some(0) {
            return Err(ActionError::Permanent(format!(
                "branch {} has no commits ahead of {}: nothing to open a pull request for",
                c.branch, c.base
            )));
        }
        // An earlier attempt may have opened it (state=all on a replay: it may have merged).
        if let Some(pr) = self
            .find_pull(token, c, owner, if c.replay { "all" } else { "open" })
            .await?
        {
            return Ok(pull_out(&pr));
        }
        let (st, created) = self
            .send(
                token,
                Method::POST,
                &format!("/repos/{repo}/pulls"),
                Some(json!({
                    "title": title,
                    "head": c.branch,
                    "base": c.base,
                    "body": c.body.unwrap_or_default(),
                    "draft": c.draft,
                })),
                &[StatusCode::UNPROCESSABLE_ENTITY],
                "opening the pull request",
            )
            .await?;
        if st == StatusCode::UNPROCESSABLE_ENTITY {
            // "A pull request already exists": a racing attempt opened it.
            if let Some(pr) = self.find_pull(token, c, owner, "all").await? {
                return Ok(pull_out(&pr));
            }
            let msg = created
                .get("errors")
                .and_then(|e| e.get(0))
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .or(created.get("message").and_then(Value::as_str))
                .unwrap_or("invalid request");
            return Err(ActionError::Permanent(format!(
                "opening the pull request: {msg}"
            )));
        }
        Ok(pull_out(&created))
    }

    async fn find_pull(
        &self,
        token: &str,
        c: &Call<'_>,
        owner: &str,
        state: &str,
    ) -> Result<Option<Value>, ActionError> {
        let (_, list) = self
            .send(
                token,
                Method::GET,
                &format!(
                    "/repos/{}/pulls?state={state}&head={}",
                    enc(c.repo),
                    enc_seg(&format!("{owner}:{}", c.branch))
                ),
                None,
                &[],
                "looking for the pull request",
            )
            .await?;
        Ok(list.as_array().and_then(|a| a.first().cloned()))
    }

    async fn labels(&self, token: &str, c: &Call<'_>, n: u64) -> Result<Value, ActionError> {
        if c.labels.is_empty() {
            return Err(ActionError::Permanent(format!(
                "{} needs labels",
                c.action.as_str()
            )));
        }
        let path = format!("/repos/{}/issues/{n}/labels", enc(c.repo));
        if c.action == ActionKind::AddLabels {
            self.send(
                token,
                Method::POST,
                &path,
                Some(json!({ "labels": c.labels })),
                &[],
                "adding labels",
            )
            .await?;
        } else {
            for l in c.labels {
                // 404: the label is not on the issue (or a retry already removed it).
                self.send(
                    token,
                    Method::DELETE,
                    &format!("{path}/{}", enc_seg(l)),
                    None,
                    &[StatusCode::NOT_FOUND],
                    "removing a label",
                )
                .await?;
            }
        }
        let url = c.subject.map(|s| s.url.clone()).unwrap_or_default();
        Ok(json!({ "number": n, "url": url, "labels": c.labels }))
    }
}

fn comment_out(c: &Value, issue: u64) -> Value {
    json!({
        "number": c.get("id").cloned().unwrap_or(Value::Null),
        "url": c.get("html_url").cloned().unwrap_or(Value::Null),
        "subject": issue,
    })
}

fn pull_out(p: &Value) -> Value {
    json!({
        "number": p.get("number").cloned().unwrap_or(Value::Null),
        "url": p.get("html_url").cloned().unwrap_or(Value::Null),
        "head": p.pointer("/head/ref").cloned().unwrap_or(Value::Null),
    })
}

#[async_trait]
impl Actions for GithubActions {
    async fn perform(&self, c: &Call<'_>) -> Result<Value, ActionError> {
        let n = match c.action {
            ActionKind::OpenPullRequest => 0,
            _ => c.subject.map(|s| s.number).ok_or_else(|| {
                ActionError::Permanent(format!(
                    "{} needs the run's subject issue or pull request, and this run has none",
                    c.action.as_str()
                ))
            })?,
        };
        let token = self
            .source
            .action_token(c.repo, &permissions(c.action, c.subject))
            .await?;
        match c.action {
            ActionKind::Comment => self.comment(&token, c, n).await,
            ActionKind::OpenPullRequest => self.open_pull_request(&token, c).await,
            ActionKind::AddLabels | ActionKind::RemoveLabels => self.labels(&token, c, n).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(kind: SubjectKind) -> Subject {
        Subject {
            kind,
            number: 1,
            author: "a".into(),
            url: "u".into(),
        }
    }

    #[test]
    fn permissions_are_minimal() {
        let issue = subject(SubjectKind::Issue);
        let pr = subject(SubjectKind::PullRequest);
        assert_eq!(
            permissions(ActionKind::Comment, Some(&issue)),
            [("issues", "write")]
        );
        assert_eq!(
            permissions(ActionKind::AddLabels, Some(&pr)),
            [("pull_requests", "write")]
        );
        assert_eq!(
            permissions(ActionKind::OpenPullRequest, None),
            [("pull_requests", "write"), ("contents", "read")]
        );
    }

    #[test]
    fn segments_are_encoded() {
        assert_eq!(enc_seg("ez/wf/1"), "ez%2Fwf%2F1");
        assert_eq!(enc_seg("good first"), "good%20first");
    }

    #[test]
    fn marker_is_a_hidden_comment() {
        assert_eq!(marker("r", "n", 2), "<!-- ez-tracks:r:n:2 -->");
    }
}
