use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use enzarb_connector::{
    Actor, ChangeRequestSpec, CommentRef, CommitRef, Connector, CredScope, Error, ItemRef,
    Permission, Progress, ProgressTarget, RepoKey, Result, ShortLivedCred, Status, StatusState,
    Thread, ThreadComment, WorkItemKey,
};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{BotIdentity, ConfigError, GitHubConfig};
use crate::http::{Http, Resp, expect};
use crate::normalize::{Comment, Issue, NormalizeError, Normalized, PullRequest, SOURCE, User};

/// Installation tokens are refreshed once they are within this long of expiry.
const TOKEN_REFRESH_MARGIN: Duration = Duration::minutes(5);
const PER_PAGE: usize = 100;
const MAX_PAGES: usize = 200;

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
        github_repo(repo)?;
        let token = self.token(repo).await?;
        let resp = self.http.send(method, path, &token, body.as_ref()).await?;
        expect(what, resp)
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
        let t = self
            .create_token(
                id,
                Some(json!({
                    "repositories": [repo.repo],
                    "permissions": { "contents": level, "pull_requests": level },
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
}
