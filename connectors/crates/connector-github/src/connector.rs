use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use enzarb_connector::{
    Actor, ChangeRequestSpec, CheckSummary, CiIgnore, CiInfo, CloseReason, CommentKind, CommentRef,
    CommitRef, Connector, CredScope, Error, Event, ItemRef, MergeOutcome, MergeReadiness,
    Permission, Progress, ProgressTarget, ReadCred, RepoKey, Result, ShortLivedCred, Status,
    StatusState, StatusSummary, Thread, ThreadComment, UpdateBranchOutcome, WorkItemKey,
    aggregate_ci, aggregate_statuses,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{BotIdentity, ConfigError, GitHubConfig};
use crate::http::{Http, Resp, expect};
use crate::normalize::{
    Branch, Comment, CommitStatusChange, Issue, NormalizeError, Normalized, PullRequest, SOURCE,
    User, ci_events,
};

/// Installation tokens are refreshed once they are within this long of expiry.
const TOKEN_REFRESH_MARGIN: Duration = Duration::minutes(5);
const PER_PAGE: usize = 100;
const MAX_PAGES: usize = 200;
/// Pages of recent issues searched for an idempotency marker before creating an issue.
const MARKER_SCAN_PAGES: usize = 5;
/// Delimiters of the task list that stands in for native sub-issues in a parent's body.
const TASKS_START: &str = "<!-- enzarb:subissues:start -->";
const TASKS_END: &str = "<!-- enzarb:subissues:end -->";

#[derive(Serialize)]
struct AppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
struct CachedToken {
    token: String,
    expires_at: DateTime<Utc>,
}

/// GitHub App implementation of [`Connector`].
///
/// Cheap to share behind an `Arc`; installation ids and tokens are cached
/// in memory.
pub struct GitHubConnector {
    http: Http,
    app_id: String,
    key: EncodingKey,
    bot: BotIdentity,
    ci_ignore: CiIgnore,
    installations: Mutex<HashMap<String, u64>>,
    tokens: Mutex<HashMap<u64, CachedToken>>,
}

impl std::fmt::Debug for GitHubConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubConnector")
            .field("app_id", &self.app_id)
            .field("bot", &self.bot)
            .finish_non_exhaustive()
    }
}

fn github_repo(repo: &RepoKey) -> Result<()> {
    if repo.connector == SOURCE {
        Ok(())
    } else {
        Err(Error::Invalid(format!("not a github repo: {repo}")))
    }
}

fn idem_marker(key: &str) -> Result<String> {
    if key.is_empty() || key.contains("-->") || key.contains(['\n', '\r']) {
        return Err(Error::Invalid(format!("bad idempotency key {key:?}")));
    }
    Ok(format!("<!-- enzarb:idem:{key} -->"))
}

/// `body` with `line` in its managed task list, or `None` when the list already has it. The
/// section is created at the end of the body when missing.
fn with_task(body: &str, line: &str) -> Option<String> {
    if let (Some(s), Some(e)) = (body.find(TASKS_START), body.find(TASKS_END))
        && s < e
    {
        let inner = &body[s + TASKS_START.len()..e];
        if inner.lines().any(|l| l.trim() == line) {
            return None;
        }
        let inner = inner.trim_end_matches('\n');
        return Some(format!(
            "{}{inner}\n{line}\n{}",
            &body[..s + TASKS_START.len()],
            &body[e..]
        ));
    }
    let sep = if body.trim().is_empty() { "" } else { "\n\n" };
    Some(format!(
        "{}{sep}{TASKS_START}\n{line}\n{TASKS_END}",
        body.trim_end()
    ))
}

fn reaction_content(p: Progress) -> &'static str {
    match p {
        Progress::Received => "eyes",
        Progress::Working => "rocket",
        Progress::Done => "+1",
        Progress::Failed => "confused",
    }
}

const PROGRESS_REACTIONS: [&str; 4] = ["eyes", "rocket", "+1", "confused"];

impl GitHubConnector {
    pub fn new(config: GitHubConfig) -> Result<Self> {
        let key = EncodingKey::from_rsa_pem(config.private_key_pem.as_bytes())
            .map_err(|e| Error::Invalid(format!("GITHUB_APP_PRIVATE_KEY: {e}")))?;
        Ok(Self {
            http: Http::new(&config.api_url, config.retry.clone())?,
            app_id: config.app_id,
            key,
            bot: config.bot,
            ci_ignore: config.ci_ignore,
            installations: Mutex::default(),
            tokens: Mutex::default(),
        })
    }

    /// Builds from `GITHUB_APP_ID`, `GITHUB_APP_PRIVATE_KEY`,
    /// `ENZARB_BOT_HANDLE` and `ENZARB_BOT_LOGIN` (contracts section 9).
    pub fn from_env() -> std::result::Result<Self, ConfigError> {
        let cfg = GitHubConfig::from_env()?;
        Self::new(cfg).map_err(|e| ConfigError::Invalid {
            var: "GITHUB_APP_PRIVATE_KEY",
            reason: e.to_string(),
        })
    }

    pub fn bot(&self) -> &BotIdentity {
        &self.bot
    }

    /// Normalizes a webhook delivery using this connector's bot identity.
    /// The body must already be signature-checked.
    pub fn normalize(
        &self,
        event_name: &str,
        delivery_id: &str,
        body: &[u8],
    ) -> std::result::Result<Normalized, NormalizeError> {
        self.bot.normalize(event_name, delivery_id, body)
    }

    // ---- auth ----

    fn app_jwt(&self) -> Result<String> {
        let now = Utc::now().timestamp();
        let claims = AppClaims {
            // Backdate to tolerate clock drift (GitHub's recommendation).
            iat: now - 60,
            exp: now + 9 * 60,
            iss: self.app_id.clone(),
        };
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map_err(|e| Error::Other(format!("signing app JWT: {e}")))
    }

    async fn installation_id(&self, repo: &RepoKey) -> Result<u64> {
        let name = repo.full_name();
        if let Some(id) = self.installations.lock().unwrap().get(&name) {
            return Ok(*id);
        }
        #[derive(Deserialize)]
        struct Inst {
            id: u64,
        }
        let jwt = self.app_jwt()?;
        let resp = self
            .http
            .send(
                Method::GET,
                &format!("/repos/{name}/installation"),
                &jwt,
                None,
            )
            .await?;
        let inst: Inst = expect(&format!("find installation for {name}"), resp)?.json()?;
        self.installations.lock().unwrap().insert(name, inst.id);
        Ok(inst.id)
    }

    /// A cached installation token (refreshed 5 minutes before expiry).
    async fn token(&self, repo: &RepoKey) -> Result<String> {
        let id = self.installation_id(repo).await?;
        let now = Utc::now();
        if let Some(t) = self.tokens.lock().unwrap().get(&id)
            && t.expires_at - TOKEN_REFRESH_MARGIN > now
        {
            return Ok(t.token.clone());
        }
        let t = self.create_token(id, None).await?;
        self.tokens.lock().unwrap().insert(
            id,
            CachedToken {
                token: t.token.clone(),
                expires_at: t.expires_at,
            },
        );
        Ok(t.token)
    }

    async fn create_token(
        &self,
        installation: u64,
        restrict: Option<Value>,
    ) -> Result<TokenResponse> {
        let jwt = self.app_jwt()?;
        let resp = self
            .http
            .send(
                Method::POST,
                &format!("/app/installations/{installation}/access_tokens"),
                &jwt,
                restrict.as_ref(),
            )
            .await?;
        expect("create installation token", resp)?.json()
    }

    // ---- request helpers ----

    async fn call(
        &self,
        repo: &RepoKey,
        what: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Resp> {
        let resp = self.call_raw(repo, method, path, body).await?;
        expect(what, resp)
    }

    /// Like [`call`](Self::call) but returns non-2xx responses as-is, for
    /// callers that give specific statuses their own meaning.
    async fn call_raw(
        &self,
        repo: &RepoKey,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Resp> {
        github_repo(repo)?;
        let token = self.token(repo).await?;
        let resp = self
            .http
            .send(method.clone(), path, &token, body.as_ref())
            .await?;
        if resp.status == reqwest::StatusCode::UNAUTHORIZED {
            // The cached token was revoked or expired early: drop it, retry once.
            let id = self.installation_id(repo).await?;
            self.tokens.lock().unwrap().remove(&id);
            let token = self.token(repo).await?;
            return self.http.send(method, path, &token, body.as_ref()).await;
        }
        Ok(resp)
    }

    /// The numeric id GitHub's dependency endpoints address an issue or pull request by.
    async fn issue_id(&self, item: &WorkItemKey) -> Result<u64> {
        #[derive(Deserialize)]
        struct Id {
            id: u64,
        }
        let path = format!("/repos/{}/issues/{}", item.repo.full_name(), item.number);
        Ok(self
            .call(&item.repo, "get issue", Method::GET, &path, None)
            .await?
            .json::<Id>()?
            .id)
    }

    /// GETs every page of an endpoint that wraps its list in an object
    /// (`{"total_count": n, "<key>": [...]}`). Returns the first page's whole
    /// body (for sibling fields such as the combined `state`) and all items.
    async fn get_all_wrapped<T: serde::de::DeserializeOwned>(
        &self,
        repo: &RepoKey,
        what: &str,
        path: &str,
        key: &str,
    ) -> Result<(Value, Vec<T>)> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut first = Value::Null;
        let mut total = 0u64;
        let mut out: Vec<T> = Vec::new();
        for page in 1..=MAX_PAGES {
            let p = format!("{path}{sep}per_page={PER_PAGE}&page={page}");
            let root: Value = self.call(repo, what, Method::GET, &p, None).await?.json()?;
            let items: Vec<T> =
                serde_json::from_value(root.get(key).cloned().unwrap_or(Value::Array(vec![])))
                    .map_err(|e| Error::Other(format!("unexpected GitHub response: {e}")))?;
            if page == 1 {
                total = root["total_count"].as_u64().unwrap_or(0);
                first = root;
            }
            let n = items.len();
            out.extend(items);
            if n < PER_PAGE || out.len() as u64 >= total {
                break;
            }
        }
        Ok((first, out))
    }

    /// `AlreadyMerged` if the pull request is merged, else `None`.
    async fn merged_outcome(&self, repo: &RepoKey, number: u64) -> Result<Option<MergeOutcome>> {
        let pr = self.pull_state(repo, number).await?;
        Ok(pr.merged.then_some(MergeOutcome::AlreadyMerged {
            sha: pr.merge_commit_sha,
        }))
    }

    async fn pull_state(&self, repo: &RepoKey, number: u64) -> Result<PullState> {
        self.call(
            repo,
            "get pull request",
            Method::GET,
            &format!("/repos/{}/pulls/{number}", repo.full_name()),
            None,
        )
        .await?
        .json()
    }

    /// Turns a `status` delivery into `CiStatusChanged` events for the open
    /// pull requests whose head is the status's commit. Empty when there are
    /// none (a branch without a PR, or a commit GitHub cannot find).
    pub async fn resolve_commit_status(&self, change: &CommitStatusChange) -> Result<Vec<Event>> {
        let r = &change.repo;
        let prs: Vec<PullRequest> = match self
            .get_all(
                r,
                "list pull requests for commit",
                &format!("/repos/{}/commits/{}/pulls", r.full_name(), change.sha),
            )
            .await
        {
            Ok(prs) => prs,
            Err(Error::NotFound(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let items: Vec<ItemRef> = prs
            .iter()
            .filter(|pr| {
                pr.state.as_deref() == Some("open")
                    && pr.head.sha.as_deref() == Some(change.sha.as_str())
            })
            .map(PullRequest::item)
            .collect();
        let ci = CiInfo {
            head_sha: change.sha.clone(),
            state: change.state,
            context: change.context.clone(),
            url: change.url.clone(),
        };
        Ok(ci_events(
            &change.delivery,
            r,
            &change.actor,
            change.time,
            &ci,
            items,
        ))
    }

    /// GETs every page of a list endpoint.
    async fn get_all<T: serde::de::DeserializeOwned>(
        &self,
        repo: &RepoKey,
        what: &str,
        path: &str,
    ) -> Result<Vec<T>> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let p = format!("{path}{sep}per_page={PER_PAGE}&page={page}");
            let items: Vec<T> = self.call(repo, what, Method::GET, &p, None).await?.json()?;
            let n = items.len();
            out.extend(items);
            if n < PER_PAGE {
                break;
            }
        }
        Ok(out)
    }

    async fn find_open_pr(&self, req: &ChangeRequestSpec) -> Result<Option<PullRequest>> {
        let r = &req.repo;
        let path = format!(
            "/repos/{}/pulls?state=open&head={}:{}&base={}",
            r.full_name(),
            r.owner,
            req.head_ref,
            req.base_ref
        );
        let prs: Vec<PullRequest> = self
            .call(r, "list pull requests", Method::GET, &path, None)
            .await?
            .json()?;
        Ok(prs.into_iter().next())
    }

    /// The number of the most recently created issue (not pull request) whose body carries
    /// `marker`. Scans the newest [`MARKER_SCAN_PAGES`] pages of the repository's issues:
    /// the issue list is consistent right after a create, unlike the search index.
    async fn find_idem_issue(&self, repo: &RepoKey, marker: &str) -> Result<Option<u64>> {
        #[derive(Deserialize)]
        struct Row {
            number: u64,
            #[serde(default)]
            body: Option<String>,
            #[serde(default)]
            pull_request: Option<Value>,
        }
        for page in 1..=MARKER_SCAN_PAGES {
            let path = format!(
                "/repos/{}/issues?state=all&sort=created&direction=desc&per_page={PER_PAGE}&page={page}",
                repo.full_name()
            );
            let rows: Vec<Row> = self
                .call(repo, "list issues", Method::GET, &path, None)
                .await?
                .json()?;
            let n = rows.len();
            if let Some(r) = rows.into_iter().find(|r| {
                r.pull_request.is_none() && r.body.as_deref().is_some_and(|b| b.contains(marker))
            }) {
                return Ok(Some(r.number));
            }
            if n < PER_PAGE {
                break;
            }
        }
        Ok(None)
    }

    /// Fallback link: list `child` as `- [ ] #N` in the managed task list of `parent`'s body.
    /// The list sits between [`TASKS_START`] and [`TASKS_END`], so repeating this changes
    /// nothing and the section can be updated in place.
    async fn append_task_list(&self, parent: &WorkItemKey, child: &WorkItemKey) -> Result<()> {
        #[derive(Deserialize)]
        struct Body {
            #[serde(default)]
            body: Option<String>,
        }
        let r = &parent.repo;
        let path = format!("/repos/{}/issues/{}", r.full_name(), parent.number);
        let current: Body = self
            .call(r, "get issue", Method::GET, &path, None)
            .await?
            .json()?;
        let line = format!("- [ ] #{}", child.number);
        let Some(body) = with_task(current.body.as_deref().unwrap_or(""), &line) else {
            return Ok(());
        };
        self.call(
            r,
            "update issue body",
            Method::PATCH,
            &path,
            Some(json!({ "body": body })),
        )
        .await?;
        Ok(())
    }

    async fn find_idem_comment(
        &self,
        item: &WorkItemKey,
        marker: &str,
    ) -> Result<Option<CommentRef>> {
        let r = &item.repo;
        let comments: Vec<Comment> = self
            .get_all(
                r,
                "list comments",
                &format!("/repos/{}/issues/{}/comments", r.full_name(), item.number),
            )
            .await?;
        Ok(comments
            .into_iter()
            .find(|c| c.body.contains(marker))
            .map(|c| c.comment_ref()))
    }
}

#[derive(Deserialize)]
struct ReviewComment {
    #[serde(flatten)]
    c: Comment,
    user: User,
}

#[derive(Deserialize)]
struct IssueComment {
    #[serde(flatten)]
    c: Comment,
    user: User,
}

/// The parts of `GET /pulls/{n}` that merging and readiness read.
#[derive(Deserialize)]
struct PullState {
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    merge_commit_sha: Option<String>,
    head: Branch,
    base: Branch,
    /// `null` while GitHub computes it.
    #[serde(default)]
    mergeable: Option<bool>,
    #[serde(default)]
    mergeable_state: Option<String>,
}

#[derive(Deserialize)]
struct CheckRun {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

#[derive(Deserialize)]
struct Reaction {
    id: u64,
    content: String,
    user: Option<User>,
}

#[async_trait]
impl Connector for GitHubConnector {
    async fn fetch_thread(&self, item: &WorkItemKey) -> Result<Thread> {
        let r = &item.repo;
        let name = r.full_name();
        let n = item.number;
        let issue: Issue = self
            .call(
                r,
                "get issue",
                Method::GET,
                &format!("/repos/{name}/issues/{n}"),
                None,
            )
            .await?
            .json()?;
        let is_pr = issue.pull_request.is_some();
        let item_ref = if is_pr {
            let pr: PullRequest = self
                .call(
                    r,
                    "get pull request",
                    Method::GET,
                    &format!("/repos/{name}/pulls/{n}"),
                    None,
                )
                .await?
                .json()?;
            pr.item()
        } else {
            issue.item()
        };

        let mut comments: Vec<ThreadComment> = self
            .get_all::<IssueComment>(
                r,
                "list comments",
                &format!("/repos/{name}/issues/{n}/comments"),
            )
            .await?
            .into_iter()
            .map(|c| ThreadComment {
                author: c.user.actor(),
                created_at: c.c.created_at,
                comment: c.c.comment_ref(),
            })
            .collect();
        if is_pr {
            let review = self
                .get_all::<ReviewComment>(
                    r,
                    "list review comments",
                    &format!("/repos/{name}/pulls/{n}/comments"),
                )
                .await?;
            comments.extend(review.into_iter().map(|c| ThreadComment {
                author: c.user.actor(),
                created_at: c.c.created_at,
                comment: c.c.comment_ref(),
            }));
        }
        // Oldest first; id breaks ties (and orders when timestamps are absent).
        comments.sort_by_key(|c| (c.created_at, c.comment.id));
        Ok(Thread {
            item: item_ref,
            comments,
        })
    }

    async fn post_comment(
        &self,
        item: &WorkItemKey,
        body: &str,
        idempotency_key: &str,
    ) -> Result<CommentRef> {
        let marker = idem_marker(idempotency_key)?;
        if let Some(existing) = self.find_idem_comment(item, &marker).await? {
            return Ok(existing);
        }
        let r = &item.repo;
        let resp = self
            .call(
                r,
                "post comment",
                Method::POST,
                &format!("/repos/{}/issues/{}/comments", r.full_name(), item.number),
                Some(json!({ "body": format!("{body}\n\n{marker}") })),
            )
            .await?;
        Ok(resp.json::<Comment>()?.comment_ref())
    }

    async fn open_change_request(
        &self,
        req: ChangeRequestSpec,
        _idempotency_key: &str,
    ) -> Result<ItemRef> {
        let r = &req.repo;
        let name = r.full_name();
        if let Some(pr) = self.find_open_pr(&req).await? {
            return Ok(pr.item());
        }
        let created = self
            .call(
                r,
                "open pull request",
                Method::POST,
                &format!("/repos/{name}/pulls"),
                Some(json!({
                    "title": req.title,
                    "body": req.body,
                    "head": req.head_ref,
                    "base": req.base_ref,
                    "draft": req.draft,
                })),
            )
            .await;
        match created {
            Ok(resp) => Ok(resp.json::<PullRequest>()?.item()),
            // Lost a race with a concurrent attempt: GitHub says one exists.
            Err(Error::Invalid(msg)) if msg.contains("already exists") => self
                .find_open_pr(&req)
                .await?
                .map(|pr| pr.item())
                .ok_or(Error::Invalid(msg)),
            Err(e) => Err(e),
        }
    }

    async fn set_status(
        &self,
        target: &CommitRef,
        status: Status,
        _idempotency_key: &str,
    ) -> Result<()> {
        let state = match status.state {
            StatusState::Pending => "pending",
            StatusState::Success => "success",
            StatusState::Failure => "failure",
            StatusState::Error => "error",
        };
        // GitHub limits descriptions to 140 characters.
        let description: String = status.description.chars().take(140).collect();
        let mut body = json!({
            "state": state,
            "context": status.context,
            "description": description,
        });
        if let Some(url) = status.target_url {
            body["target_url"] = url.into();
        }
        self.call(
            &target.repo,
            "set commit status",
            Method::POST,
            &format!("/repos/{}/statuses/{}", target.repo.full_name(), target.sha),
            Some(body),
        )
        .await?;
        Ok(())
    }

    async fn set_progress(
        &self,
        target: &ProgressTarget,
        progress: Progress,
        _idempotency_key: &str,
    ) -> Result<()> {
        let r = &target.item.repo;
        let name = r.full_name();
        let base = match target.comment_id {
            Some(c) if target.comment_kind == CommentKind::Review => {
                format!("/repos/{name}/pulls/comments/{c}/reactions")
            }
            Some(c) => format!("/repos/{name}/issues/comments/{c}/reactions"),
            None => format!("/repos/{name}/issues/{}/reactions", target.item.number),
        };
        let want = reaction_content(progress);
        // Add first so there is never a moment with no marker. POST is
        // idempotent: an existing reaction by the app returns 200.
        self.call(
            r,
            "add reaction",
            Method::POST,
            &base,
            Some(json!({ "content": want })),
        )
        .await?;
        let existing: Vec<Reaction> = self.get_all(r, "list reactions", &base).await?;
        for old in existing {
            let ours = old
                .user
                .as_ref()
                .is_some_and(|u| u.login.eq_ignore_ascii_case(&self.bot.login));
            if ours && old.content != want && PROGRESS_REACTIONS.contains(&old.content.as_str()) {
                self.call(
                    r,
                    "remove reaction",
                    Method::DELETE,
                    &format!("{base}/{}", old.id),
                    None,
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn merge_change_request(
        &self,
        item: &WorkItemKey,
        number: u64,
        expected_head_sha: &str,
        commit_title: &str,
        commit_body: Option<&str>,
        _idempotency_key: &str,
    ) -> Result<MergeOutcome> {
        let r = &item.repo;
        github_repo(r)?;
        // GitHub has no marker for an idempotency key; a merged PR is the record.
        if let Some(done) = self.merged_outcome(r, number).await? {
            return Ok(done);
        }
        let mut body = json!({
            "merge_method": "squash",
            "sha": expected_head_sha,
            "commit_title": commit_title,
        });
        if let Some(b) = commit_body {
            body["commit_message"] = b.into();
        }
        let resp = self
            .call_raw(
                r,
                Method::PUT,
                &format!("/repos/{}/pulls/{number}/merge", r.full_name()),
                Some(body),
            )
            .await?;
        match resp.status {
            reqwest::StatusCode::METHOD_NOT_ALLOWED | reqwest::StatusCode::CONFLICT => {
                // A concurrent or earlier attempt may have won.
                if let Some(done) = self.merged_outcome(r, number).await? {
                    return Ok(done);
                }
                if resp.status == reqwest::StatusCode::CONFLICT {
                    return Ok(MergeOutcome::HeadMoved);
                }
                let reason = serde_json::from_slice::<Value>(&resp.body)
                    .ok()
                    .and_then(|v| v["message"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "not mergeable".to_string());
                Ok(MergeOutcome::NotMergeable { reason })
            }
            _ => {
                #[derive(Deserialize)]
                struct Merged {
                    sha: String,
                }
                let resp = expect("merge pull request", resp)?;
                Ok(MergeOutcome::Merged {
                    sha: resp.json::<Merged>()?.sha,
                })
            }
        }
    }

    async fn update_branch(
        &self,
        item: &WorkItemKey,
        number: u64,
        expected_head_sha: &str,
    ) -> Result<UpdateBranchOutcome> {
        let r = &item.repo;
        github_repo(r)?;
        let resp = self
            .call_raw(
                r,
                Method::PUT,
                &format!("/repos/{}/pulls/{number}/update-branch", r.full_name()),
                Some(json!({ "expected_head_sha": expected_head_sha })),
            )
            .await?;
        match resp.status {
            // 422: merge conflict or the head is not `expected_head_sha`; 409 as for merges.
            reqwest::StatusCode::UNPROCESSABLE_ENTITY | reqwest::StatusCode::CONFLICT => {
                let reason = serde_json::from_slice::<Value>(&resp.body)
                    .ok()
                    .and_then(|v| v["message"].as_str().map(str::to_string))
                    .unwrap_or_else(|| "the branch could not be updated".to_string());
                Ok(UpdateBranchOutcome::Refused { reason })
            }
            _ => {
                expect("update pull request branch", resp)?;
                Ok(UpdateBranchOutcome::Updated)
            }
        }
    }

    async fn set_base(
        &self,
        item: &WorkItemKey,
        number: u64,
        new_base: &str,
        _idempotency_key: &str,
    ) -> Result<()> {
        let r = &item.repo;
        github_repo(r)?;
        // PATCH is idempotent: GitHub answers 200 when the base is already `new_base`.
        self.call(
            r,
            "set pull request base",
            Method::PATCH,
            &format!("/repos/{}/pulls/{number}", r.full_name()),
            Some(json!({ "base": new_base })),
        )
        .await?;
        Ok(())
    }

    async fn get_merge_readiness(&self, item: &WorkItemKey, number: u64) -> Result<MergeReadiness> {
        let r = &item.repo;
        let pr = self.pull_state(r, number).await?;
        let head_sha = pr
            .head
            .sha
            .ok_or_else(|| Error::Other(format!("pull request {number} has no head sha")))?;
        let name = r.full_name();
        let (combined, statuses): (Value, Vec<Value>) = self
            .get_all_wrapped(
                r,
                "get combined status",
                &format!("/repos/{name}/commits/{head_sha}/status"),
                "statuses",
            )
            .await?;
        let (_, runs): (Value, Vec<CheckRun>) = self
            .get_all_wrapped(
                r,
                "list check runs",
                &format!("/repos/{name}/commits/{head_sha}/check-runs"),
                "check_runs",
            )
            .await?;
        // Advisory checks (`ENZARB_CI_IGNORE_CHECKS`, e.g. Copilot code review) never count.
        let checks: Vec<CheckSummary> = runs
            .into_iter()
            .filter(|c| !self.ci_ignore.is_ignored(&c.name))
            .map(|c| CheckSummary {
                status: c.status,
                conclusion: c.conclusion,
            })
            .collect();
        let named = || {
            statuses.iter().map(|s| {
                (
                    s["context"].as_str().unwrap_or_default(),
                    s["state"].as_str().unwrap_or_default(),
                )
            })
        };
        let statuses = if named().any(|(c, _)| self.ci_ignore.is_ignored(c)) {
            aggregate_statuses(named(), &self.ci_ignore)
        } else {
            StatusSummary {
                total_count: combined["total_count"].as_u64().unwrap_or(0),
                state: combined["state"].as_str().unwrap_or_default().to_string(),
            }
        };
        Ok(MergeReadiness {
            head_sha,
            base_sha: pr.base.sha,
            mergeable: pr.mergeable,
            mergeable_state: pr.mergeable_state.unwrap_or_default(),
            ci: aggregate_ci(&checks, &statuses),
        })
    }

    async fn delete_branch(
        &self,
        repo: &RepoKey,
        branch: &str,
        _idempotency_key: &str,
    ) -> Result<()> {
        let path = format!(
            "/repos/{}/git/refs/heads/{}",
            repo.full_name(),
            encode_path(branch)
        );
        let resp = self.call_raw(repo, Method::DELETE, &path, None).await?;
        // 404: no such ref, 422: "Reference does not exist". Both mean it is already gone.
        if matches!(resp.status.as_u16(), 404 | 422) {
            return Ok(());
        }
        expect("delete branch", resp)?;
        Ok(())
    }

    async fn branch_head(&self, repo: &RepoKey, branch: &str) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct Commit {
            sha: String,
        }
        #[derive(Deserialize)]
        struct BranchInfo {
            commit: Commit,
        }
        let path = format!(
            "/repos/{}/branches/{}",
            repo.full_name(),
            encode_path(branch)
        );
        // Resolve auth first so a missing installation is not mistaken for a
        // missing branch below.
        github_repo(repo)?;
        self.token(repo).await?;
        match self
            .call(repo, "get branch", Method::GET, &path, None)
            .await
        {
            Ok(r) => Ok(Some(r.json::<BranchInfo>()?.commit.sha)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn ahead_by(&self, repo: &RepoKey, base: &str, head: &str) -> Result<Option<u64>> {
        #[derive(Deserialize)]
        struct Comparison {
            ahead_by: u64,
        }
        let path = format!(
            "/repos/{}/compare/{}...{}",
            repo.full_name(),
            encode_path(base),
            encode_path(head)
        );
        github_repo(repo)?;
        self.token(repo).await?;
        match self
            .call(repo, "compare commits", Method::GET, &path, None)
            .await
        {
            Ok(r) => Ok(Some(r.json::<Comparison>()?.ahead_by)),
            Err(Error::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn get_file(&self, repo: &RepoKey, git_ref: &str, path: &str) -> Result<Option<String>> {
        #[derive(Deserialize)]
        struct Content {
            #[serde(rename = "type")]
            kind: String,
            #[serde(default)]
            encoding: String,
            #[serde(default)]
            content: String,
        }
        let api = format!(
            "/repos/{}/contents/{}?ref={}",
            repo.full_name(),
            encode_path(path.trim_start_matches('/')),
            encode_path(git_ref)
        );
        // Resolve auth first so a missing installation is not mistaken for a
        // missing file below.
        github_repo(repo)?;
        self.token(repo).await?;
        let r = match self.call(repo, "get file", Method::GET, &api, None).await {
            Ok(r) => r,
            Err(Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        // A directory answers with an array.
        let v: serde_json::Value = r.json()?;
        if !v.is_object() {
            return Ok(None);
        }
        let c: Content = serde_json::from_value(v)
            .map_err(|e| Error::Other(format!("get file: unexpected response: {e}")))?;
        if c.kind != "file" {
            return Ok(None);
        }
        if c.encoding != "base64" {
            return Err(Error::Other(format!(
                "get file: {path} has encoding {:?} (too large?)",
                c.encoding
            )));
        }
        let bytes = decode_base64(&c.content)
            .ok_or_else(|| Error::Other(format!("get file: {path}: bad base64")))?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| Error::Other(format!("get file: {path} is not UTF-8")))
    }

    async fn list_dependencies(&self, item: &WorkItemKey) -> Result<Vec<WorkItemKey>> {
        #[derive(Deserialize)]
        struct Blocker {
            number: u64,
            #[serde(default)]
            repository_url: Option<String>,
        }
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!(
                "/repos/{}/issues/{}/dependencies/blocked_by?per_page={PER_PAGE}&page={page}",
                item.repo.full_name(),
                item.number
            );
            let rows: Vec<Blocker> = self
                .call(&item.repo, "list dependencies", Method::GET, &path, None)
                .await?
                .json()?;
            let n = rows.len();
            for b in rows {
                // `repository_url` is `<api>/repos/<owner>/<repo>`.
                let repo = b
                    .repository_url
                    .as_deref()
                    .and_then(|u| {
                        let mut parts = u.trim_end_matches('/').rsplit('/');
                        let repo = parts.next()?;
                        let owner = parts.next()?;
                        Some(RepoKey::new(SOURCE, owner, repo))
                    })
                    .unwrap_or_else(|| item.repo.clone());
                out.push(WorkItemKey::new(repo, b.number));
            }
            if n < PER_PAGE {
                break;
            }
        }
        Ok(out)
    }

    async fn add_dependency(
        &self,
        item: &WorkItemKey,
        depends_on: &WorkItemKey,
        _idempotency_key: &str,
    ) -> Result<()> {
        github_repo(&depends_on.repo)?;
        let id = self.issue_id(depends_on).await?;
        let path = format!(
            "/repos/{}/issues/{}/dependencies/blocked_by",
            item.repo.full_name(),
            item.number
        );
        // 422 means the relationship already exists (the request is idempotent by state).
        let resp = self
            .call_raw(
                &item.repo,
                Method::POST,
                &path,
                Some(json!({ "issue_id": id })),
            )
            .await?;
        if resp.status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            let existing = self.list_dependencies(item).await?;
            if existing.contains(depends_on) {
                return Ok(());
            }
        }
        expect("add dependency", resp).map(|_| ())
    }

    async fn remove_dependency(
        &self,
        item: &WorkItemKey,
        depends_on: &WorkItemKey,
        _idempotency_key: &str,
    ) -> Result<()> {
        github_repo(&depends_on.repo)?;
        let id = self.issue_id(depends_on).await?;
        let path = format!(
            "/repos/{}/issues/{}/dependencies/blocked_by/{id}",
            item.repo.full_name(),
            item.number
        );
        match self
            .call(&item.repo, "remove dependency", Method::DELETE, &path, None)
            .await
        {
            Ok(_) | Err(Error::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn create_issue(
        &self,
        repo: &RepoKey,
        title: &str,
        body: &str,
        labels: &[String],
        idempotency_key: &str,
    ) -> Result<WorkItemKey> {
        #[derive(Deserialize)]
        struct Created {
            number: u64,
        }
        github_repo(repo)?;
        let marker = idem_marker(idempotency_key)?;
        if let Some(number) = self.find_idem_issue(repo, &marker).await? {
            return Ok(WorkItemKey::new(repo.clone(), number));
        }
        let resp = self
            .call(
                repo,
                "create issue",
                Method::POST,
                &format!("/repos/{}/issues", repo.full_name()),
                Some(json!({
                    "title": title,
                    "body": format!("{body}\n\n{marker}"),
                    "labels": labels,
                })),
            )
            .await?;
        Ok(WorkItemKey::new(
            repo.clone(),
            resp.json::<Created>()?.number,
        ))
    }

    async fn close_issue(&self, item: &WorkItemKey, reason: CloseReason) -> Result<()> {
        github_repo(&item.repo)?;
        // PATCHing a closed issue to closed again is a no-op on GitHub, so retries are safe.
        self.call(
            &item.repo,
            "close issue",
            Method::PATCH,
            &format!("/repos/{}/issues/{}", item.repo.full_name(), item.number),
            Some(json!({ "state": "closed", "state_reason": reason.as_str() })),
        )
        .await?;
        Ok(())
    }

    async fn update_issue(&self, item: &WorkItemKey, body: &str) -> Result<()> {
        #[derive(Deserialize)]
        struct Current {
            #[serde(default)]
            body: Option<String>,
        }
        github_repo(&item.repo)?;
        let path = format!("/repos/{}/issues/{}", item.repo.full_name(), item.number);
        let current: Current = self
            .call(&item.repo, "get issue", Method::GET, &path, None)
            .await?
            .json()?;
        // Keep the idempotency marker `create_issue` appended, or a retried creation would
        // not find this issue and make another.
        let marker = current
            .body
            .as_deref()
            .and_then(|b| b.rfind("<!-- enzarb:idem:").map(|i| &b[i..]))
            .and_then(|t| t.find("-->").map(|e| &t[..e + 3]));
        let new = match marker {
            Some(m) => format!("{body}\n\n{m}"),
            None => body.to_string(),
        };
        if current.body.as_deref() == Some(new.as_str()) {
            return Ok(());
        }
        self.call(
            &item.repo,
            "update issue",
            Method::PATCH,
            &path,
            Some(json!({ "body": new })),
        )
        .await?;
        Ok(())
    }

    async fn link_sub_issue(&self, parent: &WorkItemKey, child: &WorkItemKey) -> Result<()> {
        github_repo(&child.repo)?;
        if parent.repo != child.repo {
            return Err(Error::Invalid(
                "sub-issues must be in one repository".into(),
            ));
        }
        let id = self.issue_id(child).await?;
        let r = &parent.repo;
        let path = format!(
            "/repos/{}/issues/{}/sub_issues",
            r.full_name(),
            parent.number
        );
        let resp = self
            .call_raw(r, Method::POST, &path, Some(json!({ "sub_issue_id": id })))
            .await?;
        if resp.status.is_success() {
            return Ok(());
        }
        // 422 also means "already a sub-issue": check the list before giving up on the API.
        if resp.status == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            #[derive(Deserialize)]
            struct Sub {
                number: u64,
            }
            let list = format!("{path}?per_page={PER_PAGE}");
            let subs: Vec<Sub> = self
                .call(r, "list sub-issues", Method::GET, &list, None)
                .await?
                .json()?;
            if subs.iter().any(|s| s.number == child.number) {
                return Ok(());
            }
        }
        // Sub-issues are unavailable here (older GitHub, missing permission, plan): the
        // task list in the parent's body still shows the breakdown.
        tracing::info!(
            parent = %parent, child = %child, status = %resp.status,
            "sub-issue link refused; falling back to a task list"
        );
        self.append_task_list(parent, child).await
    }

    async fn check_permission(&self, repo: &RepoKey, actor: &Actor) -> Result<Permission> {
        #[derive(Deserialize)]
        struct Perm {
            permission: String,
            #[serde(default)]
            role_name: Option<String>,
        }
        let path = format!(
            "/repos/{}/collaborators/{}/permission",
            repo.full_name(),
            actor.login
        );
        // Resolve auth first so a missing installation is not mistaken for a
        // missing user below.
        github_repo(repo)?;
        self.token(repo).await?;
        let perm: Perm = match self
            .call(repo, "check permission", Method::GET, &path, None)
            .await
        {
            Ok(r) => r.json()?,
            // Unknown user: no access.
            Err(Error::NotFound(_)) => return Ok(Permission::None),
            Err(e) => return Err(e),
        };
        // `permission` only distinguishes admin/write/read/none; `role_name`
        // also carries maintain and triage.
        let role = perm.role_name.as_deref().unwrap_or(&perm.permission);
        Ok(match role {
            "admin" => Permission::Admin,
            "maintain" => Permission::Maintain,
            "write" => Permission::Write,
            "triage" => Permission::Triage,
            "read" => Permission::Read,
            _ => match perm.permission.as_str() {
                "admin" => Permission::Admin,
                "write" => Permission::Write,
                "read" => Permission::Read,
                _ => Permission::None,
            },
        })
    }

    async fn mint_clone_credential(
        &self,
        repo: &RepoKey,
        scope: CredScope,
        _idempotency_key: &str,
    ) -> Result<ShortLivedCred> {
        github_repo(repo)?;
        let level = match scope {
            CredScope::Read => "read",
            CredScope::ReadWrite => "write",
        };
        let id = self.installation_id(repo).await?;
        let mut permissions = json!({ "contents": level, "pull_requests": level });
        // GitHub only offers `workflows: write` (no read level), so only
        // read-write tokens carry it; agents may edit `.github/workflows`.
        if scope == CredScope::ReadWrite {
            permissions["workflows"] = json!("write");
        }
        let t = self
            .create_token(
                id,
                Some(json!({
                    "repositories": [repo.repo],
                    "permissions": permissions,
                })),
            )
            .await?;
        Ok(ShortLivedCred {
            token: t.token,
            repo: repo.clone(),
            scope,
            expires_at: t.expires_at,
        })
    }

    async fn mint_read_credential(
        &self,
        repos: &[RepoKey],
        _purpose: &str,
    ) -> Result<Option<ReadCred>> {
        let Some(first) = repos.first() else {
            return Err(Error::Invalid("no repos for read credential".into()));
        };
        for r in repos {
            github_repo(r)?;
        }
        // One token has one installation (one owner): every repo must resolve
        // to the first repo's installation, otherwise it is outside it.
        let id = self.installation_id(first).await?;
        let mut names: Vec<String> = Vec::with_capacity(repos.len());
        for r in repos {
            if r.owner != first.owner || self.installation_id(r).await? != id {
                return Err(Error::Invalid(format!(
                    "{r} is outside the installation of {first}"
                )));
            }
            if !names.contains(&r.repo) {
                names.push(r.repo.clone());
            }
        }
        // `packages: read` lets the sandbox `npm ci` private packages from
        // npm.pkg.github.com. The App may not have been granted it yet (or the
        // installation not accepted it): GitHub then rejects the whole request,
        // so fall back to contents-only rather than break the deps token.
        let wide = json!({
            "repositories": names,
            "permissions": { "contents": "read", "packages": "read" },
        });
        let t = match self.create_token(id, Some(wide)).await {
            Ok(t) => t,
            Err(Error::Invalid(m) | Error::PermissionDenied(m)) if is_permission_rejection(&m) => {
                // Only the status/body from GitHub is logged, never a token.
                tracing::warn!(
                    "GitHub rejected packages:read for the deps credential; \
                     falling back to contents:read (grant the App Packages: read): {m}"
                );
                self.create_token(
                    id,
                    Some(json!({
                        "repositories": names,
                        "permissions": { "contents": "read" },
                    })),
                )
                .await?
            }
            Err(e) => return Err(e),
        };
        Ok(Some(ReadCred {
            token: t.token,
            repos: repos.to_vec(),
            expires_at: t.expires_at,
        }))
    }

    async fn mint_ci_read_credential(&self, repo: &RepoKey) -> Result<Option<ReadCred>> {
        github_repo(repo)?;
        let id = self.installation_id(repo).await?;
        // No contents access: only CI data of this one repository. The App may not
        // have been granted `actions: read` yet; GitHub then rejects the request and
        // the run goes without a token.
        let body = json!({
            "repositories": [repo.repo],
            "permissions": { "actions": "read", "checks": "read", "metadata": "read" },
        });
        match self.create_token(id, Some(body)).await {
            Ok(t) => Ok(Some(ReadCred {
                token: t.token,
                repos: vec![repo.clone()],
                expires_at: t.expires_at,
            })),
            Err(Error::Invalid(m) | Error::PermissionDenied(m)) if is_permission_rejection(&m) => {
                tracing::warn!(
                    "GitHub rejected actions:read for the CI read credential \
                     (grant the App Actions: read): {m}"
                );
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

/// Percent-encode everything except unreserved characters and `/`.
/// Standard base64 with padding; whitespace (GitHub wraps lines) is skipped.
fn decode_base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits = 0;
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\n' | b'\r' | b' ' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Whether an error from the token endpoint is GitHub refusing the requested
/// permissions (422 or 403 whose body names permissions), as opposed to a bad
/// JWT or an unknown installation.
fn is_permission_rejection(detail: &str) -> bool {
    (detail.contains(": 422 ") || detail.contains(": 403 "))
        && detail.to_ascii_lowercase().contains("permission")
}

#[cfg(test)]
mod tests {
    use super::with_task;

    #[test]
    fn task_list_section_is_created_extended_and_not_repeated() {
        let created = with_task("Epic body.", "- [ ] #1").unwrap();
        assert_eq!(
            created,
            "Epic body.\n\n<!-- enzarb:subissues:start -->\n- [ ] #1\n<!-- enzarb:subissues:end -->"
        );
        let extended = with_task(&created, "- [ ] #2").unwrap();
        assert!(extended.contains("- [ ] #1\n- [ ] #2\n<!-- enzarb:subissues:end -->"));
        assert_eq!(with_task(&extended, "- [ ] #2"), None);
        assert_eq!(
            with_task("", "- [ ] #1").unwrap(),
            "<!-- enzarb:subissues:start -->\n- [ ] #1\n<!-- enzarb:subissues:end -->"
        );
    }
}
