//! Reads `.ez/workflows.toml` and `.ez/modes.toml` at the default branch head through the GitHub
//! App's installation token (ADR-0029 section 4). Tokens are minted per repository with
//! `contents: read` only and cached until shortly before they expire.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::header::HeaderMap;
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde_json::json;

pub const WORKFLOWS_PATH: &str = ".ez/workflows.toml";
pub const MODES_PATH: &str = ".ez/modes.toml";

const API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Installation tokens live an hour; refresh well before.
const TOKEN_TTL: Duration = Duration::from_secs(40 * 60);
const INSTALLATION_TTL: Duration = Duration::from_secs(60 * 60);
/// How long a repository snapshot (default-branch head plus the two `.ez` files) is reused before
/// the head is asked about again. A commit to the default branch is therefore seen within this
/// time. Concurrent events of one repository share a single fetch, and an unchanged head is
/// revalidated with `If-None-Match` (a 304 does not count against GitHub's rate limit).
pub const SNAPSHOT_TTL: Duration = Duration::from_secs(30);
/// Longest total time one call waits out GitHub rate limits (honouring `retry-after` /
/// `x-ratelimit-reset`) before it fails transiently; the router's sweep then retries the event.
pub const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(120);
/// Wait when GitHub reports a rate limit without saying for how long.
const DEFAULT_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);
/// Prefix of the message of every rate-limit failure; the router never counts these as attempts.
pub const RATE_LIMITED: &str = "GitHub rate limit";

/// Whether a response is a GitHub rate limit, and how long to wait. Primary and secondary limits
/// answer 403 or 429 carrying `retry-after`, `x-ratelimit-remaining: 0` or a body that says so.
/// A plain 403 (permissions) is `None`.
pub fn rate_limit_wait(status: StatusCode, headers: &HeaderMap, body: &str) -> Option<Duration> {
    if status != StatusCode::FORBIDDEN && status != StatusCode::TOO_MANY_REQUESTS {
        return None;
    }
    let num = |name: &str| -> Option<u64> { headers.get(name)?.to_str().ok()?.trim().parse().ok() };
    if let Some(secs) = num("retry-after") {
        return Some(Duration::from_secs(secs.max(1)));
    }
    if headers
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim() == "0")
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        return Some(match num("x-ratelimit-reset") {
            Some(reset) => Duration::from_secs(reset.saturating_sub(now).max(1)),
            None => DEFAULT_RATE_LIMIT_WAIT,
        });
    }
    if status == StatusCode::TOO_MANY_REQUESTS || body.to_ascii_lowercase().contains("rate limit") {
        return Some(DEFAULT_RATE_LIMIT_WAIT);
    }
    None
}

/// The files of one commit, pinned by `sha`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub sha: String,
    pub workflows: Option<String>,
    pub modes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// Retry later (network, rate limit, 5xx): no decision is recorded.
    Transient(String),
    /// Retrying will not help (App not installed, branch gone): recorded as the decision's error.
    Permanent(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient(m) | Self::Permanent(m) => f.write_str(m),
        }
    }
}

#[async_trait]
pub trait RepoSource: Send + Sync {
    /// Head of `branch` and the two `.ez` files at that commit.
    async fn snapshot(&self, repo: &str, branch: &str) -> Result<Snapshot, SourceError>;

    /// A file at commit `sha` (`None`: it does not exist there). Used for the instruction files
    /// ez folds into the prompt, pinned with the run.
    async fn file_at(
        &self,
        _repo: &str,
        _sha: &str,
        _path: &str,
    ) -> Result<Option<String>, SourceError> {
        Err(SourceError::Permanent(
            "reading files is not supported by this source".into(),
        ))
    }

    /// A short-lived installation token scoped to `repo` with `contents: read` (clone) or
    /// `contents: write` (`write`, push). Minted fresh, never cached, so it is short-lived.
    async fn clone_token(&self, _repo: &str, _write: bool) -> Result<String, SourceError> {
        Err(SourceError::Permanent(
            "minting tokens is not supported by this source".into(),
        ))
    }

    /// A fresh installation token scoped to `repo` with exactly `permissions` (for example
    /// `[("issues", "write")]`), for `action` nodes. It never enters the sandbox.
    async fn action_token(
        &self,
        _repo: &str,
        _permissions: &[(&str, &str)],
    ) -> Result<String, SourceError> {
        Err(SourceError::Permanent(
            "minting tokens is not supported by this source".into(),
        ))
    }
}

/// A source for installations without a GitHub App: every read is a permanent error.
pub struct NoApp;

#[async_trait]
impl RepoSource for NoApp {
    async fn snapshot(&self, _: &str, _: &str) -> Result<Snapshot, SourceError> {
        Err(SourceError::Permanent(
            "the GitHub App is not configured (ENZARB_GITHUB_APP_ID / ENZARB_GITHUB_APP_PRIVATE_KEY)"
                .into(),
        ))
    }
}

/// A cached snapshot with the ETag of the head request that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    pub snap: Snapshot,
    pub etag: Option<String>,
}

type Slot = Arc<tokio::sync::Mutex<Option<(Cached, Instant)>>>;

/// Per-key snapshot cache with a TTL. One fetch per key at a time: concurrent callers wait for the
/// running fetch and reuse its result. Errors are not cached.
pub struct SnapshotCache {
    ttl: Duration,
    slots: Mutex<HashMap<(String, String), Slot>>,
}

impl SnapshotCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// The fresh entry for `key`, or the result of `fetch`, which receives the stale entry (for a
    /// conditional request) and returns the new one.
    pub async fn get_or_fetch(
        &self,
        key: (String, String),
        fetch: impl AsyncFnOnce(Option<Cached>) -> Result<Cached, SourceError>,
    ) -> Result<Snapshot, SourceError> {
        let slot = self.slots.lock().unwrap().entry(key).or_default().clone();
        let mut guard = slot.lock().await;
        if let Some((c, at)) = guard.as_ref()
            && at.elapsed() < self.ttl
        {
            return Ok(c.snap.clone());
        }
        let stale = guard.as_ref().map(|(c, _)| c.clone());
        let fresh = fetch(stale).await?;
        let snap = fresh.snap.clone();
        *guard = Some((fresh, Instant::now()));
        Ok(snap)
    }
}

pub struct GithubApp {
    snapshots: SnapshotCache,
    max_wait: Duration,
    /// No request is sent before this instant (set by the last rate-limit answer).
    blocked_until: Mutex<Option<Instant>>,
    app_id: String,
    key: EncodingKey,
    http: Client,
    base: String,
    installations: Mutex<HashMap<String, (u64, Instant)>>,
    tokens: Mutex<HashMap<String, (String, Instant)>>,
}

#[derive(Deserialize)]
struct Id {
    id: u64,
}
#[derive(Deserialize)]
struct Token {
    token: String,
}

fn transient(what: &str, e: impl std::fmt::Display) -> SourceError {
    SourceError::Transient(format!("{what}: {e}"))
}

fn limited(what: &str, wait: Duration) -> SourceError {
    SourceError::Transient(format!(
        "{RATE_LIMITED} on {what}: retry in {}s",
        wait.as_secs().max(1)
    ))
}

/// Percent-encodes a URL path, keeping `/`.
fn enc_path(s: &str) -> String {
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

impl GithubApp {
    pub fn new(app_id: &str, private_key_pem: &str) -> anyhow::Result<Self> {
        Self::with_base(app_id, private_key_pem, API)
    }

    pub fn with_base(app_id: &str, private_key_pem: &str, base: &str) -> anyhow::Result<Self> {
        // Keys stored in env vars often carry literal `\n`.
        let pem = private_key_pem.replace("\\n", "\n");
        Ok(Self {
            app_id: app_id.to_string(),
            key: EncodingKey::from_rsa_pem(pem.as_bytes())?,
            http: Client::builder()
                .timeout(TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("enzarb-tracks")
                .build()?,
            base: base.trim_end_matches('/').to_string(),
            snapshots: SnapshotCache::new(SNAPSHOT_TTL),
            max_wait: MAX_RATE_LIMIT_WAIT,
            blocked_until: Mutex::new(None),
            installations: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    /// Overrides the snapshot TTL and the total rate-limit wait (tests).
    pub fn with_limits(mut self, ttl: Duration, max_wait: Duration) -> Self {
        self.snapshots = SnapshotCache::new(ttl);
        self.max_wait = max_wait;
        self
    }

    /// Sends the request `build` makes, waiting out rate limits (`retry-after`,
    /// `x-ratelimit-reset`) for at most `max_wait` in total; beyond that it fails with a transient
    /// [`RATE_LIMITED`] error without waiting, so the event stays undecided for the sweep. While
    /// a limit is known to be active no request is sent at all.
    async fn send(
        &self,
        what: &str,
        build: impl Fn() -> RequestBuilder,
    ) -> Result<Response, SourceError> {
        let mut waited = Duration::ZERO;
        loop {
            let blocked = self
                .blocked_until
                .lock()
                .unwrap()
                .map(|t| t.saturating_duration_since(Instant::now()))
                .unwrap_or_default();
            if !blocked.is_zero() {
                if waited + blocked > self.max_wait {
                    return Err(limited(what, blocked));
                }
                tokio::time::sleep(blocked).await;
                waited += blocked;
            }
            let res = build().send().await.map_err(|e| transient(what, e))?;
            let status = res.status();
            if status != StatusCode::FORBIDDEN && status != StatusCode::TOO_MANY_REQUESTS {
                return Ok(res);
            }
            let headers = res.headers().clone();
            let body = res.text().await.unwrap_or_default();
            let Some(wait) = rate_limit_wait(status, &headers, &body) else {
                return Err(SourceError::Permanent(format!(
                    "{what}: GitHub answered {status}"
                )));
            };
            *self.blocked_until.lock().unwrap() = Some(Instant::now() + wait);
            if waited + wait > self.max_wait {
                return Err(limited(what, wait));
            }
            tracing::warn!(
                what,
                wait_secs = wait.as_secs(),
                "GitHub rate limit; waiting"
            );
            tokio::time::sleep(wait).await;
            waited += wait;
        }
    }

    fn jwt(&self) -> Result<String, SourceError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| transient("clock", e))?
            .as_secs();
        let claims =
            json!({ "iat": now.saturating_sub(60), "exp": now + 9 * 60, "iss": self.app_id });
        jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map_err(|e| SourceError::Permanent(format!("could not sign the App JWT: {e}")))
    }

    fn classify(status: StatusCode, what: &str) -> SourceError {
        let msg = format!("{what}: GitHub answered {status}");
        if status.is_server_error()
            || status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::UNAUTHORIZED
        {
            SourceError::Transient(msg)
        } else {
            SourceError::Permanent(msg)
        }
    }

    async fn installation(&self, repo: &str) -> Result<u64, SourceError> {
        if let Some((id, until)) = self.installations.lock().unwrap().get(repo)
            && *until > Instant::now()
        {
            return Ok(*id);
        }
        let jwt = self.jwt()?;
        let res = self
            .send("installation lookup", || {
                self.http
                    .get(format!(
                        "{}/repos/{}/installation",
                        self.base,
                        enc_path(repo)
                    ))
                    .bearer_auth(&jwt)
                    .header("Accept", "application/vnd.github+json")
            })
            .await?;
        match res.status() {
            s if s.is_success() => {}
            StatusCode::NOT_FOUND => {
                return Err(SourceError::Permanent(format!(
                    "the GitHub App is not installed on {repo}"
                )));
            }
            s => return Err(Self::classify(s, "installation lookup")),
        }
        let id = res
            .json::<Id>()
            .await
            .map_err(|e| transient("installation lookup", e))?
            .id;
        self.installations
            .lock()
            .unwrap()
            .insert(repo.to_string(), (id, Instant::now() + INSTALLATION_TTL));
        Ok(id)
    }

    async fn mint(
        &self,
        repo: &str,
        permissions: serde_json::Value,
    ) -> Result<String, SourceError> {
        let id = self.installation(repo).await?;
        let name = repo.split_once('/').map_or(repo, |(_, n)| n);
        let jwt = self.jwt()?;
        let res = self
            .send("token mint", || {
                self.http
                    .post(format!(
                        "{}/app/installations/{id}/access_tokens",
                        self.base
                    ))
                    .bearer_auth(&jwt)
                    .header("Accept", "application/vnd.github+json")
                    .json(&json!({ "repositories": [name], "permissions": permissions }))
            })
            .await?;
        if !res.status().is_success() {
            self.installations.lock().unwrap().remove(repo);
            return Err(Self::classify(res.status(), "token mint"));
        }
        Ok(res
            .json::<Token>()
            .await
            .map_err(|e| transient("token mint", e))?
            .token)
    }

    async fn token(&self, repo: &str) -> Result<String, SourceError> {
        if let Some((t, until)) = self.tokens.lock().unwrap().get(repo)
            && *until > Instant::now()
        {
            return Ok(t.clone());
        }
        let id = self.installation(repo).await?;
        let name = repo.split_once('/').map_or(repo, |(_, n)| n);
        let jwt = self.jwt()?;
        let res = self
            .send("token mint", || {
                self.http
                    .post(format!(
                        "{}/app/installations/{id}/access_tokens",
                        self.base
                    ))
                    .bearer_auth(&jwt)
                    .header("Accept", "application/vnd.github+json")
                    .json(&json!({ "repositories": [name], "permissions": { "contents": "read" } }))
            })
            .await?;
        if !res.status().is_success() {
            // A stale installation id: look it up again next time.
            self.installations.lock().unwrap().remove(repo);
            return Err(Self::classify(res.status(), "token mint"));
        }
        let t = res
            .json::<Token>()
            .await
            .map_err(|e| transient("token mint", e))?
            .token;
        self.tokens
            .lock()
            .unwrap()
            .insert(repo.to_string(), (t.clone(), Instant::now() + TOKEN_TTL));
        Ok(t)
    }

    /// One head request (conditional on the stale entry's ETag); the `.ez` files are read again
    /// only when the head moved, since a commit's files never change.
    async fn fetch_snapshot(
        &self,
        repo: &str,
        branch: &str,
        stale: Option<Cached>,
    ) -> Result<Cached, SourceError> {
        let token = self.token(repo).await?;
        let etag = stale.as_ref().and_then(|c| c.etag.clone());
        let res = self
            .send("default branch head", || {
                let mut rb = self
                    .http
                    .get(format!(
                        "{}/repos/{}/commits/{}",
                        self.base,
                        enc_path(repo),
                        enc_path(branch)
                    ))
                    .bearer_auth(&token)
                    .header("Accept", "application/vnd.github.sha");
                if let Some(e) = &etag {
                    rb = rb.header("If-None-Match", e);
                }
                rb
            })
            .await?;
        match res.status() {
            StatusCode::NOT_MODIFIED => {
                if let Some(c) = stale {
                    return Ok(c);
                }
                return Err(SourceError::Transient(
                    "unexpected 304 without a cache".into(),
                ));
            }
            s if s.is_success() => {}
            StatusCode::NOT_FOUND | StatusCode::UNPROCESSABLE_ENTITY => {
                return Err(SourceError::Permanent(format!(
                    "branch {branch} of {repo} was not found"
                )));
            }
            s => {
                if s == StatusCode::UNAUTHORIZED {
                    self.tokens.lock().unwrap().remove(repo);
                }
                return Err(Self::classify(s, "default branch head"));
            }
        }
        let new_etag = res
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let sha = res
            .text()
            .await
            .map_err(|e| transient("default branch head", e))?
            .trim()
            .to_string();
        if sha.len() < 7 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(SourceError::Transient(format!(
                "unexpected head sha {sha:?}"
            )));
        }
        if let Some(c) = stale
            && c.snap.sha == sha
        {
            return Ok(Cached {
                snap: c.snap,
                etag: new_etag,
            });
        }
        let workflows = self.file(repo, &token, &sha, WORKFLOWS_PATH).await?;
        let modes = self.file(repo, &token, &sha, MODES_PATH).await?;
        Ok(Cached {
            snap: Snapshot {
                sha,
                workflows,
                modes,
            },
            etag: new_etag,
        })
    }

    async fn file(
        &self,
        repo: &str,
        token: &str,
        sha: &str,
        path: &str,
    ) -> Result<Option<String>, SourceError> {
        let res = self
            .send(path, || {
                self.http
                    .get(format!(
                        "{}/repos/{}/contents/{path}",
                        self.base,
                        enc_path(repo)
                    ))
                    .query(&[("ref", sha)])
                    .bearer_auth(token)
                    .header("Accept", "application/vnd.github.raw+json")
            })
            .await?;
        match res.status() {
            s if s.is_success() => Ok(Some(res.text().await.map_err(|e| transient(path, e))?)),
            StatusCode::NOT_FOUND => Ok(None),
            s => Err(Self::classify(s, path)),
        }
    }
}

#[async_trait]
impl RepoSource for GithubApp {
    async fn snapshot(&self, repo: &str, branch: &str) -> Result<Snapshot, SourceError> {
        self.snapshots
            .get_or_fetch((repo.to_string(), branch.to_string()), async |stale| {
                self.fetch_snapshot(repo, branch, stale).await
            })
            .await
    }

    async fn file_at(
        &self,
        repo: &str,
        sha: &str,
        path: &str,
    ) -> Result<Option<String>, SourceError> {
        let token = self.token(repo).await?;
        self.file(repo, &token, sha, path).await
    }

    async fn clone_token(&self, repo: &str, write: bool) -> Result<String, SourceError> {
        let p = if write { "write" } else { "read" };
        self.mint(repo, json!({ "contents": p })).await
    }

    async fn action_token(
        &self,
        repo: &str,
        permissions: &[(&str, &str)],
    ) -> Result<String, SourceError> {
        let map: serde_json::Map<String, serde_json::Value> = permissions
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v)))
            .collect();
        self.mint(repo, serde_json::Value::Object(map)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_encoding_keeps_slashes() {
        assert_eq!(enc_path("o/r"), "o/r");
        assert_eq!(enc_path("feat/a b#1"), "feat/a%20b%231");
    }

    #[test]
    fn statuses_are_classified() {
        assert!(matches!(
            GithubApp::classify(StatusCode::BAD_GATEWAY, "x"),
            SourceError::Transient(_)
        ));
        assert!(matches!(
            GithubApp::classify(StatusCode::TOO_MANY_REQUESTS, "x"),
            SourceError::Transient(_)
        ));
        assert!(matches!(
            GithubApp::classify(StatusCode::GONE, "x"),
            SourceError::Permanent(_)
        ));
    }

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn rate_limits_are_recognised() {
        let f = StatusCode::FORBIDDEN;
        assert_eq!(
            rate_limit_wait(f, &hm(&[("retry-after", "7")]), ""),
            Some(Duration::from_secs(7))
        );
        let reset = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 30;
        let w = rate_limit_wait(
            f,
            &hm(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", &reset.to_string()),
            ]),
            "",
        )
        .unwrap();
        assert!((28..=30).contains(&w.as_secs()), "{w:?}");
        assert_eq!(
            rate_limit_wait(f, &hm(&[("x-ratelimit-remaining", "0")]), ""),
            Some(DEFAULT_RATE_LIMIT_WAIT)
        );
        assert!(
            rate_limit_wait(f, &hm(&[]), "You have exceeded a secondary rate limit.").is_some()
        );
        assert!(rate_limit_wait(StatusCode::TOO_MANY_REQUESTS, &hm(&[]), "").is_some());
    }

    #[test]
    fn plain_forbidden_is_not_a_rate_limit() {
        assert_eq!(
            rate_limit_wait(
                StatusCode::FORBIDDEN,
                &hm(&[("x-ratelimit-remaining", "4999")]),
                "Resource not accessible by integration"
            ),
            None
        );
        assert_eq!(
            rate_limit_wait(StatusCode::OK, &hm(&[("retry-after", "5")]), ""),
            None
        );
        assert_eq!(
            rate_limit_wait(StatusCode::NOT_FOUND, &hm(&[]), "rate limit"),
            None
        );
    }

    fn cached(sha: &str) -> Cached {
        Cached {
            snap: Snapshot {
                sha: sha.into(),
                workflows: None,
                modes: None,
            },
            etag: Some("e".into()),
        }
    }

    fn key() -> (String, String) {
        ("o/r".into(), "main".into())
    }

    #[tokio::test]
    async fn cache_hits_misses_and_expires() {
        use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
        let c = SnapshotCache::new(Duration::from_millis(60));
        let calls = AtomicU32::new(0);
        let get = async |sha: &'static str| {
            c.get_or_fetch(key(), async |_| {
                calls.fetch_add(1, SeqCst);
                Ok(cached(sha))
            })
            .await
            .unwrap()
            .sha
        };
        assert_eq!(get("a").await, "a");
        assert_eq!(get("b").await, "a", "hit within the TTL");
        assert_eq!(calls.load(SeqCst), 1);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(get("b").await, "b", "expired");
        assert_eq!(calls.load(SeqCst), 2);
        // The stale entry is handed to the fetch for a conditional request.
        tokio::time::sleep(Duration::from_millis(80)).await;
        let seen = c
            .get_or_fetch(key(), async |stale| {
                assert_eq!(stale.unwrap().etag.as_deref(), Some("e"));
                Ok(cached("c"))
            })
            .await
            .unwrap();
        assert_eq!(seen.sha, "c");
    }

    #[tokio::test]
    async fn cache_does_not_keep_errors_and_keys_are_separate() {
        let c = SnapshotCache::new(Duration::from_secs(60));
        let e = c
            .get_or_fetch(key(), async |_| Err(SourceError::Transient("x".into())))
            .await;
        assert!(e.is_err());
        assert!(
            c.get_or_fetch(key(), async |_| Ok(cached("a")))
                .await
                .is_ok()
        );
        let other = c
            .get_or_fetch(("o/r2".into(), "main".into()), async |_| Ok(cached("z")))
            .await
            .unwrap();
        assert_eq!(other.sha, "z");
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_fetch() {
        use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
        let c = Arc::new(SnapshotCache::new(Duration::from_secs(60)));
        let calls = Arc::new(AtomicU32::new(0));
        let mut hs = Vec::new();
        for _ in 0..10 {
            let (c, calls) = (c.clone(), calls.clone());
            hs.push(tokio::spawn(async move {
                c.get_or_fetch(key(), async |_| {
                    calls.fetch_add(1, SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(cached("a"))
                })
                .await
                .unwrap()
            }));
        }
        for h in hs {
            assert_eq!(h.await.unwrap().sha, "a");
        }
        assert_eq!(calls.load(SeqCst), 1);
    }

    #[test]
    fn bad_key_is_rejected() {
        assert!(GithubApp::new("1", "not a key").is_err());
    }
}
