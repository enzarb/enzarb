//! Reads `.ez/workflows.toml` and `.ez/modes.toml` at the default branch head through the GitHub
//! App's installation token (ADR-0029 section 4). Tokens are minted per repository with
//! `contents: read` only and cached until shortly before they expire.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_json::json;

pub const WORKFLOWS_PATH: &str = ".ez/workflows.toml";
pub const MODES_PATH: &str = ".ez/modes.toml";

const API: &str = "https://api.github.com";
const TIMEOUT: Duration = Duration::from_secs(15);
/// Installation tokens live an hour; refresh well before.
const TOKEN_TTL: Duration = Duration::from_secs(40 * 60);
const INSTALLATION_TTL: Duration = Duration::from_secs(60 * 60);

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

pub struct GithubApp {
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
            installations: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
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
            || status == StatusCode::FORBIDDEN
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
        let res = self
            .http
            .get(format!(
                "{}/repos/{}/installation",
                self.base,
                enc_path(repo)
            ))
            .bearer_auth(self.jwt()?)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|e| transient("installation lookup", e))?;
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
        let res = self
            .http
            .post(format!(
                "{}/app/installations/{id}/access_tokens",
                self.base
            ))
            .bearer_auth(self.jwt()?)
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "repositories": [name], "permissions": permissions }))
            .send()
            .await
            .map_err(|e| transient("token mint", e))?;
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
        let res = self
            .http
            .post(format!(
                "{}/app/installations/{id}/access_tokens",
                self.base
            ))
            .bearer_auth(self.jwt()?)
            .header("Accept", "application/vnd.github+json")
            .json(&json!({ "repositories": [name], "permissions": { "contents": "read" } }))
            .send()
            .await
            .map_err(|e| transient("token mint", e))?;
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

    async fn file(
        &self,
        repo: &str,
        token: &str,
        sha: &str,
        path: &str,
    ) -> Result<Option<String>, SourceError> {
        let res = self
            .http
            .get(format!(
                "{}/repos/{}/contents/{path}",
                self.base,
                enc_path(repo)
            ))
            .query(&[("ref", sha)])
            .bearer_auth(token)
            .header("Accept", "application/vnd.github.raw+json")
            .send()
            .await
            .map_err(|e| transient(path, e))?;
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
        let token = self.token(repo).await?;
        let res = self
            .http
            .get(format!(
                "{}/repos/{}/commits/{}",
                self.base,
                enc_path(repo),
                enc_path(branch)
            ))
            .bearer_auth(&token)
            .header("Accept", "application/vnd.github.sha")
            .send()
            .await
            .map_err(|e| transient("default branch head", e))?;
        match res.status() {
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
        let workflows = self.file(repo, &token, &sha, WORKFLOWS_PATH).await?;
        let modes = self.file(repo, &token, &sha, MODES_PATH).await?;
        Ok(Snapshot {
            sha,
            workflows,
            modes,
        })
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

    #[test]
    fn bad_key_is_rejected() {
        assert!(GithubApp::new("1", "not a key").is_err());
    }
}
