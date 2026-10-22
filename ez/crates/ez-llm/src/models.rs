//! Model discovery: list the models an endpoint offers.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::time::Duration;

/// Which wire protocol to use for discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKind {
    Anthropic,
    /// OpenAI-compatible (OpenAI, Ollama, OpenRouter, vLLM, LM Studio, ...).
    Openai,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

const MAX_PAGES: usize = 50;

fn error_text(status: reqwest::StatusCode, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .or_else(|| v["message"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| body.trim().chars().take(300).collect());
    if detail.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
    }
}

async fn get_json(rb: reqwest::RequestBuilder) -> Result<Value> {
    let resp = rb.send().await.context("request failed")?;
    let status = resp.status();
    let body = resp.text().await.context("reading response")?;
    if !status.is_success() {
        bail!("{}", error_text(status, &body));
    }
    serde_json::from_str(&body).context("response is not valid JSON")
}

/// List the models available at `base_url`.
///
/// Anthropic: `GET {base}/v1/models?limit=1000` (following `has_more`/`last_id`), kept in the
/// order returned (newest first). OpenAI-compatible: `GET {base}/models`, sorted alphabetically.
pub async fn list_models(
    kind: ApiKind,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Vec<ModelInfo>> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("ez/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;
    let base = base_url.trim_end_matches('/');
    let mut out: Vec<ModelInfo> = vec![];
    match kind {
        ApiKind::Anthropic => {
            let mut after: Option<String> = None;
            for _ in 0..MAX_PAGES {
                let mut url = format!("{base}/v1/models?limit=1000");
                if let Some(a) = &after {
                    url.push_str("&after_id=");
                    url.push_str(a);
                }
                let mut rb = client.get(&url).header("anthropic-version", "2023-06-01");
                if let Some(k) = api_key {
                    rb = rb.header("x-api-key", k);
                }
                let v = get_json(rb).await?;
                let data = v["data"]
                    .as_array()
                    .context("no `data` array in response")?;
                for m in data {
                    if let Some(id) = m["id"].as_str() {
                        out.push(ModelInfo {
                            id: id.to_string(),
                            display_name: m["display_name"].as_str().map(String::from),
                        });
                    }
                }
                match (v["has_more"].as_bool(), v["last_id"].as_str()) {
                    (Some(true), Some(last)) => after = Some(last.to_string()),
                    _ => break,
                }
            }
        }
        ApiKind::Openai => {
            let mut rb = client.get(format!("{base}/models"));
            if let Some(k) = api_key {
                rb = rb.bearer_auth(k);
            }
            let v = get_json(rb).await?;
            let data = v["data"]
                .as_array()
                .context("no `data` array in response")?;
            for m in data {
                if let Some(id) = m["id"].as_str() {
                    out.push(ModelInfo {
                        id: id.to_string(),
                        display_name: m["display_name"].as_str().map(String::from),
                    });
                }
            }
            out.sort_by(|a, b| a.id.cmp(&b.id));
            out.dedup_by(|a, b| a.id == b.id);
        }
    }
    Ok(out)
}
