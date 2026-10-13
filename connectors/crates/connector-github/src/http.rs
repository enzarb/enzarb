//! Thin reqwest wrapper: GitHub headers, retries, status -> `Error` mapping.

use std::time::Duration;

use enzarb_connector::Error;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::RetryPolicy;

pub(crate) struct Http {
    client: reqwest::Client,
    base: String,
    retry: RetryPolicy,
}

pub(crate) struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        serde_json::from_slice(&self.body)
            .map_err(|e| Error::Other(format!("unexpected GitHub response: {e}")))
    }

    fn text(&self) -> String {
        let t = String::from_utf8_lossy(&self.body);
        t.chars().take(500).collect()
    }
}

fn is_rate_limited(status: StatusCode, headers: &HeaderMap, body: &str) -> bool {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return true;
    }
    if status != StatusCode::FORBIDDEN {
        return false;
    }
    headers.contains_key(RETRY_AFTER)
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            == Some("0")
        || body.to_ascii_lowercase().contains("rate limit")
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

impl Http {
    pub fn new(base: &str, retry: RetryPolicy) -> Result<Self, Error> {
        let client = reqwest::Client::builder()
            .user_agent(concat!(
                "enzarb-connector-github/",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Other(format!("http client: {e}")))?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_string(),
            retry,
        })
    }

    /// Sends a request with retries on network errors, 5xx and rate limits.
    /// Returns any other response as-is; map failures with [`expect`].
    pub async fn send(
        &self,
        method: Method,
        path: &str,
        bearer: &str,
        json: Option<&Value>,
    ) -> Result<Resp, Error> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut rb = self
                .client
                .request(method.clone(), &url)
                .bearer_auth(bearer)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28");
            if let Some(j) = json {
                rb = rb.json(j);
            }
            let last = attempt >= self.retry.max_attempts;
            let backoff = (self.retry.base_delay * 2u32.saturating_pow(attempt - 1))
                .min(self.retry.max_delay);
            match rb.send().await {
                Err(e) => {
                    if last {
                        return Err(Error::Transient(format!("{method} {path}: {e}")));
                    }
                    tokio::time::sleep(backoff).await;
                }
                Ok(r) => {
                    let status = r.status();
                    let headers = r.headers().clone();
                    let body = r
                        .bytes()
                        .await
                        .map_err(|e| Error::Transient(format!("{method} {path}: {e}")))?
                        .to_vec();
                    let resp = Resp {
                        status,
                        headers,
                        body,
                    };
                    let limited = is_rate_limited(status, &resp.headers, &resp.text());
                    if status.is_server_error() || limited {
                        if last {
                            return Err(if limited {
                                Error::RateLimited {
                                    retry_after: retry_after(&resp.headers),
                                }
                            } else {
                                Error::Transient(format!(
                                    "{method} {path}: {status}: {}",
                                    resp.text()
                                ))
                            });
                        }
                        let wait = retry_after(&resp.headers)
                            .map(|d| d.min(self.retry.max_delay))
                            .filter(|_| limited)
                            .unwrap_or(backoff);
                        tokio::time::sleep(wait).await;
                        continue;
                    }
                    return Ok(resp);
                }
            }
        }
    }
}

/// Maps a non-success response to an [`Error`]; passes success through.
pub(crate) fn expect(what: &str, resp: Resp) -> Result<Resp, Error> {
    if resp.status.is_success() {
        return Ok(resp);
    }
    let detail = format!("{what}: {}: {}", resp.status, resp.text());
    Err(match resp.status {
        StatusCode::NOT_FOUND | StatusCode::GONE => Error::NotFound(detail),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Error::PermissionDenied(detail),
        StatusCode::UNPROCESSABLE_ENTITY | StatusCode::BAD_REQUEST => Error::Invalid(detail),
        _ => Error::Other(detail),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Building a client and starting a TLS connection needs a rustls provider. With
    /// `rustls-no-provider` this panics if none is installed or if several are ambiguous; here
    /// the connect fails cleanly instead.
    #[tokio::test]
    async fn https_request_uses_ring_provider() {
        // Err means a provider is already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let retry = RetryPolicy {
            max_attempts: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
        };
        let http = Http::new("https://127.0.0.1:1", retry).unwrap();
        let err = http.send(Method::GET, "/", "t", None).await.err().unwrap();
        assert!(matches!(err, Error::Transient(_)), "{err:?}");
    }
}
