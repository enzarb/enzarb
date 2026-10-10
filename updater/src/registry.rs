//! Confirms the chart version exists in the OCI registry (HEAD of the manifest).

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::{StatusCode, header};

#[async_trait]
pub trait Registry: Send + Sync {
    /// `Ok(true)` if the manifest exists, `Ok(false)` if the registry says 404.
    async fn chart_exists(&self, version: &str) -> Result<bool>;
}

pub struct OciRegistry {
    http: reqwest::Client,
    host: String,
    repo: String,
    token: Option<String>,
}

const ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.v2+json";

/// `oci://ghcr.io/enzarb/charts/enzarb` -> (`ghcr.io`, `enzarb/charts/enzarb`).
pub fn parse_chart_ref(r: &str) -> Result<(String, String)> {
    let rest = r
        .strip_prefix("oci://")
        .context("chart ref must start with oci://")?;
    let (host, repo) = rest
        .split_once('/')
        .context("chart ref needs a repository path")?;
    let repo = repo.trim_end_matches('/');
    if host.is_empty() || repo.is_empty() {
        bail!("chart ref needs a host and repository path");
    }
    Ok((host.into(), repo.into()))
}

impl OciRegistry {
    pub fn new(chart_ref: &str, token: Option<String>) -> Result<Self> {
        let (host, repo) = parse_chart_ref(chart_ref)?;
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self {
            http,
            host,
            repo,
            token,
        })
    }

    async fn head(&self, url: &str, bearer: Option<&str>) -> Result<reqwest::Response> {
        let mut req = self.http.head(url).header(header::ACCEPT, ACCEPT);
        if let Some(b) = bearer {
            req = req.bearer_auth(b);
        }
        Ok(req.send().await?)
    }

    /// Follow a `WWW-Authenticate: Bearer realm=...,service=...` challenge.
    async fn fetch_token(&self, challenge: &str) -> Result<String> {
        let param = |k: &str| {
            let needle = format!("{k}=\"");
            let s = challenge.find(&needle)? + needle.len();
            let e = challenge[s..].find('"')?;
            Some(challenge[s..s + e].to_string())
        };
        let realm = param("realm").ok_or_else(|| anyhow!("challenge has no realm"))?;
        let mut req = self
            .http
            .get(&realm)
            .query(&[("scope", format!("repository:{}:pull", self.repo))]);
        if let Some(s) = param("service") {
            req = req.query(&[("service", s)]);
        }
        if let Some(t) = &self.token {
            req = req.basic_auth("enzarb-updater", Some(t));
        }
        let resp = req
            .send()
            .await?
            .error_for_status()
            .context("registry token request")?;
        let v: serde_json::Value = resp.json().await?;
        v["token"]
            .as_str()
            .or_else(|| v["access_token"].as_str())
            .map(String::from)
            .ok_or_else(|| anyhow!("token response has no token"))
    }
}

#[async_trait]
impl Registry for OciRegistry {
    async fn chart_exists(&self, version: &str) -> Result<bool> {
        let url = format!("https://{}/v2/{}/manifests/{version}", self.host, self.repo);
        let mut resp = self.head(&url, None).await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            let challenge = resp
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|h| h.to_str().ok())
                .context("401 without WWW-Authenticate")?
                .to_string();
            let tok = self.fetch_token(&challenge).await?;
            resp = self.head(&url, Some(&tok)).await?;
        }
        match resp.status() {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => bail!("registry answered {s}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chart_ref() {
        assert_eq!(
            parse_chart_ref("oci://ghcr.io/enzarb/charts/enzarb").unwrap(),
            ("ghcr.io".into(), "enzarb/charts/enzarb".into())
        );
        assert!(parse_chart_ref("ghcr.io/x/y").is_err());
        assert!(parse_chart_ref("oci://ghcr.io").is_err());
    }
}
