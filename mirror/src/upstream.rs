use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reqwest::header::{ACCEPT, AUTHORIZATION, WWW_AUTHENTICATE};
use serde::Deserialize;

/// Client for anonymous pulls from public registries, handling the
/// distribution token flow: a 401 carries a WWW-Authenticate header pointing
/// at a token realm; we fetch an anonymous pull token and retry.
pub struct Upstream {
    http: reqwest::Client,
    scheme: &'static str,
    /// (host, repo) → (token, expiry).
    tokens: Mutex<HashMap<(String, String), (String, Instant)>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    token: String,
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires_in: Option<u64>,
}

/// Parsed `Bearer realm="…",service="…",scope="…"` challenge.
struct Challenge {
    realm: String,
    params: Vec<(String, String)>,
}

fn parse_challenge(header: &str) -> Option<Challenge> {
    let rest = header.strip_prefix("Bearer ")?;
    let mut realm = None;
    let mut params = Vec::new();
    for part in rest.split(',') {
        let (k, v) = part.trim().split_once('=')?;
        let v = v.trim_matches('"').to_string();
        if k == "realm" {
            realm = Some(v);
        } else {
            params.push((k.to_string(), v));
        }
    }
    Some(Challenge {
        realm: realm?,
        params,
    })
}

impl Upstream {
    pub fn new() -> Self {
        Self::with_scheme("https")
    }

    fn with_scheme(scheme: &'static str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()
                .expect("build http client"),
            scheme,
            tokens: Mutex::new(HashMap::new()),
        }
    }

    fn cached_token(&self, host: &str, repo: &str) -> Option<String> {
        let tokens = self.tokens.lock().unwrap();
        let (tok, exp) = tokens.get(&(host.to_string(), repo.to_string()))?;
        (Instant::now() < *exp).then(|| tok.clone())
    }

    async fn fetch_token(&self, host: &str, repo: &str, challenge: &Challenge) -> Result<String> {
        let mut req = self.http.get(&challenge.realm);
        for (k, v) in &challenge.params {
            req = req.query(&[(k.as_str(), v.as_str())]);
        }
        let resp = req.send().await.context("token request")?;
        if !resp.status().is_success() {
            bail!("token endpoint returned {}", resp.status());
        }
        let tr: TokenResponse = resp.json().await.context("parse token response")?;
        let token = if tr.token.is_empty() {
            tr.access_token
        } else {
            tr.token
        };
        if token.is_empty() {
            bail!("token endpoint returned no token");
        }
        // Default per distribution spec is 60s; renew slightly early.
        let ttl = Duration::from_secs(tr.expires_in.unwrap_or(60).saturating_sub(10).max(10));
        self.tokens.lock().unwrap().insert(
            (host.to_string(), repo.to_string()),
            (token.clone(), Instant::now() + ttl),
        );
        Ok(token)
    }

    /// GET or HEAD `https://<host>/v2/<path>`, transparently acquiring an
    /// anonymous pull token on a 401 challenge. `repo` scopes the token cache.
    pub async fn request(
        &self,
        method: reqwest::Method,
        host: &str,
        repo: &str,
        path: &str,
        accept: Option<&str>,
    ) -> Result<reqwest::Response> {
        let url = format!("{}://{host}/v2/{path}", self.scheme);
        let build = |token: Option<String>| {
            let mut req = self.http.request(method.clone(), &url);
            if let Some(a) = accept {
                req = req.header(ACCEPT, a);
            }
            if let Some(t) = token {
                req = req.header(AUTHORIZATION, format!("Bearer {t}"));
            }
            req
        };

        let resp = build(self.cached_token(host, repo)).send().await?;
        if resp.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(resp);
        }
        let Some(challenge) = resp
            .headers()
            .get(WWW_AUTHENTICATE)
            .and_then(|h| h.to_str().ok())
            .and_then(parse_challenge)
        else {
            return Ok(resp); // 401 without a usable challenge: pass through
        };
        let token = self.fetch_token(host, repo, &challenge).await?;
        Ok(build(Some(token)).send().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parses_bearer_challenge() {
        let c = parse_challenge(
            r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/alpine:pull""#,
        )
        .unwrap();
        assert_eq!(c.realm, "https://auth.docker.io/token");
        assert_eq!(c.params.len(), 2);
    }

    #[tokio::test]
    async fn acquires_token_on_401_and_retries() {
        let server = MockServer::start().await;
        let realm = format!("{}/token", server.uri());

        Mock::given(method("GET"))
            .and(path("/v2/library/alpine/manifests/latest"))
            .and(header("authorization", "Bearer tok123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("manifest"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v2/library/alpine/manifests/latest"))
            .respond_with(ResponseTemplate::new(401).insert_header(
                "www-authenticate",
                format!(r#"Bearer realm="{realm}",service="reg",scope="repository:library/alpine:pull""#).as_str(),
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/token"))
            .and(query_param("service", "reg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"token": "tok123", "expires_in": 300})),
            )
            .mount(&server)
            .await;

        let up = Upstream::with_scheme("http");
        let host = server.uri().strip_prefix("http://").unwrap().to_string();
        let resp = up
            .request(
                reqwest::Method::GET,
                &host,
                "library/alpine",
                "library/alpine/manifests/latest",
                None,
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await.unwrap(), "manifest");
        assert_eq!(up.cached_token(&host, "library/alpine").unwrap(), "tok123");
    }
}
