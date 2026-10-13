//! Reads the OCI registry: does a chart version exist (HEAD of the manifest) and which tags exist.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::{StatusCode, header};

#[async_trait]
pub trait Registry: Send + Sync {
    /// `Ok(true)` if the manifest exists, `Ok(false)` if the registry says 404.
    async fn chart_exists(&self, version: &str) -> Result<bool>;
    /// Every tag of the chart repository (all pages of the tags list).
    async fn list_tags(&self) -> Result<Vec<String>>;
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

    /// One request, retried once with a bearer token when the registry answers 401 with a challenge.
    async fn send(&self, method: reqwest::Method, url: &str) -> Result<reqwest::Response> {
        let req = |bearer: Option<&str>| {
            let mut r = self
                .http
                .request(method.clone(), url)
                .header(header::ACCEPT, ACCEPT);
            if let Some(b) = bearer {
                r = r.bearer_auth(b);
            }
            r
        };
        let resp = req(None).send().await?;
        if resp.status() != StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        let challenge = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|h| h.to_str().ok())
            .context("401 without WWW-Authenticate")?
            .to_string();
        let tok = self.fetch_token(&challenge).await?;
        Ok(req(Some(&tok)).send().await?)
    }

    /// Follow a `WWW-Authenticate: Bearer realm=...,service=...` challenge. The registry token
    /// is only ever sent to the registry's own host over https.
    async fn fetch_token(&self, challenge: &str) -> Result<String> {
        let param = |k: &str| {
            let needle = format!("{k}=\"");
            let s = challenge.find(&needle)? + needle.len();
            let e = challenge[s..].find('"')?;
            Some(challenge[s..s + e].to_string())
        };
        let realm = param("realm").ok_or_else(|| anyhow!("challenge has no realm"))?;
        if !same_host_https(&realm, &self.host) {
            bail!("token realm {realm:?} is not https on {}", self.host);
        }
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

/// `https://<host>/...` (host compared exactly, so `ghcr.io.evil.example` does not match).
fn same_host_https(url: &str, host: &str) -> bool {
    url.strip_prefix("https://")
        .and_then(|r| r.split(['/', '?', '#']).next())
        .is_some_and(|h| h == host)
}

/// The `rel="next"` target of a `Link` header, made absolute on `host`. A next link that points
/// anywhere else is an error: the bearer token must not follow it.
pub fn next_link(link: &str, host: &str) -> Result<Option<String>> {
    for part in link.split(',') {
        let part = part.trim();
        if !part.contains("rel=\"next\"") && !part.contains("rel=next") {
            continue;
        }
        let target = part
            .strip_prefix('<')
            .and_then(|r| r.split_once('>'))
            .map(|(t, _)| t)
            .context("malformed Link header")?;
        if target.starts_with('/') && !target.starts_with("//") {
            return Ok(Some(format!("https://{host}{target}")));
        }
        if same_host_https(target, host) {
            return Ok(Some(target.into()));
        }
        bail!("Link next target {target:?} is not on {host}");
    }
    Ok(None)
}

/// Pagination guard: a registry that never stops paginating must not wedge the poller.
const MAX_PAGES: usize = 50;

#[async_trait]
impl Registry for OciRegistry {
    async fn chart_exists(&self, version: &str) -> Result<bool> {
        let url = format!("https://{}/v2/{}/manifests/{version}", self.host, self.repo);
        let resp = self.send(reqwest::Method::HEAD, &url).await?;
        match resp.status() {
            s if s.is_success() => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            s => bail!("registry answered {s}"),
        }
    }

    async fn list_tags(&self) -> Result<Vec<String>> {
        let mut url = format!("https://{}/v2/{}/tags/list?n=1000", self.host, self.repo);
        let mut tags = vec![];
        for _ in 0..MAX_PAGES {
            let resp = self.send(reqwest::Method::GET, &url).await?;
            let status = resp.status();
            if !status.is_success() {
                bail!("registry tags list answered {status}");
            }
            let next = match resp
                .headers()
                .get(header::LINK)
                .and_then(|h| h.to_str().ok())
            {
                Some(l) => next_link(l, &self.host)?,
                None => None,
            };
            let v: serde_json::Value = resp.json().await?;
            tags.extend(
                v["tags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str().map(String::from)),
            );
            match next {
                Some(n) => url = n,
                None => return Ok(tags),
            }
        }
        bail!("registry tags list has more than {MAX_PAGES} pages")
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

    #[test]
    fn token_realm_must_be_https_on_the_registry_host() {
        assert!(same_host_https("https://ghcr.io/token", "ghcr.io"));
        assert!(!same_host_https("http://ghcr.io/token", "ghcr.io"));
        assert!(!same_host_https(
            "https://ghcr.io.evil.example/token",
            "ghcr.io"
        ));
        assert!(!same_host_https("https://evil.example/ghcr.io", "ghcr.io"));
    }

    #[test]
    fn next_link_is_followed_only_on_the_registry_host() {
        let h = "ghcr.io";
        assert_eq!(
            next_link("<https://ghcr.io/x>; rel=\"prev\"", h).unwrap(),
            None
        );
        assert_eq!(
            next_link("</v2/a/tags/list?last=1&n=2>; rel=\"next\"", h).unwrap(),
            Some("https://ghcr.io/v2/a/tags/list?last=1&n=2".into())
        );
        assert_eq!(
            next_link("<https://ghcr.io/v2/a?x=1>; rel=\"next\"", h).unwrap(),
            Some("https://ghcr.io/v2/a?x=1".into())
        );
        assert!(next_link("<https://evil.example/v2>; rel=\"next\"", h).is_err());
        assert!(next_link("<//evil.example/v2>; rel=\"next\"", h).is_err());
    }
}
