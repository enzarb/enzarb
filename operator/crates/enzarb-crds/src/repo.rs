//! Repo keys: `<connector>:<owner>/<repo>`, e.g. `github:enzarb/enzarb-ui`.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepoKeyError {
    #[error("unsupported or malformed repository url: {0}")]
    BadUrl(String),
    #[error("malformed repo key (want <connector>:<owner>/<repo>): {0}")]
    BadKey(String),
}

/// A normalized repo key. Owner and repo are compared case-insensitively
/// (stored lowercased), matching GitHub semantics.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepoKey(String);

impl RepoKey {
    /// Parse a canonical https URL (`https://github.com/a/b` or `.../a/b.git`)
    /// into a key using the connector implied by the host.
    pub fn from_url(url: &str) -> Result<Self, RepoKeyError> {
        let bad = || RepoKeyError::BadUrl(url.to_string());
        let rest = url.trim().strip_prefix("https://").ok_or_else(bad)?;
        let (host, path) = rest.split_once('/').ok_or_else(bad)?;
        let connector = match host.to_ascii_lowercase().as_str() {
            "github.com" | "www.github.com" => "github",
            _ => return Err(bad()),
        };
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        Self::build(connector, path).ok_or_else(bad)
    }

    /// Build a key from an explicit connector name and URL, honoring the
    /// connector field of a `Repository` (host is still validated by shape only).
    pub fn from_connector_url(connector: &str, url: &str) -> Result<Self, RepoKeyError> {
        let bad = || RepoKeyError::BadUrl(url.to_string());
        let rest = url.trim().strip_prefix("https://").ok_or_else(bad)?;
        let (_, path) = rest.split_once('/').ok_or_else(bad)?;
        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        Self::build(connector, path).ok_or_else(bad)
    }

    /// Parse and normalize an existing key string (`github:A/B` -> `github:a/b`).
    pub fn parse(key: &str) -> Result<Self, RepoKeyError> {
        let bad = || RepoKeyError::BadKey(key.to_string());
        let (connector, path) = key.trim().split_once(':').ok_or_else(bad)?;
        Self::build(connector, path).ok_or_else(bad)
    }

    fn build(connector: &str, path: &str) -> Option<Self> {
        let mut it = path.split('/');
        let (owner, repo) = (it.next()?, it.next()?);
        if it.next().is_some() || connector.is_empty() || owner.is_empty() || repo.is_empty() {
            return None;
        }
        let ok = |s: &str| {
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        };
        if ![connector, owner, repo].iter().all(|s| ok(s)) {
            return None;
        }
        Some(Self(
            format!("{connector}:{owner}/{repo}").to_ascii_lowercase(),
        ))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
