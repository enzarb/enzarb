use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Result};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, TokenData, Validation};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<CacheInner>>,
    url: String,
}

struct CacheInner {
    keys: HashMap<String, DecodingKey>,
    fetched_at: Instant,
}

#[derive(Debug, Deserialize)]
struct Jwks {
    keys: Vec<JwkKey>,
}

#[derive(Debug, Deserialize)]
struct JwkKey {
    kid: Option<String>,
    kty: String,
    #[serde(flatten)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: String,
    pub exp: u64,
    pub iat: u64,
    pub projects: HashMap<String, Vec<String>>,
}

impl JwksCache {
    pub async fn new(url: String) -> Result<Self> {
        let cache = JwksCache {
            inner: Arc::new(RwLock::new(CacheInner {
                keys: HashMap::new(),
                fetched_at: Instant::now() - JWKS_CACHE_TTL - Duration::from_secs(1),
            })),
            url,
        };
        cache.refresh().await?;
        Ok(cache)
    }

    pub async fn refresh(&self) -> Result<()> {
        let resp = reqwest::get(&self.url).await?;
        let jwks: Jwks = resp.json().await?;

        let mut keys = HashMap::new();
        for key in jwks.keys {
            if key.kty != "RSA" && key.kty != "EC" {
                continue;
            }
            let kid = key.kid.unwrap_or_else(|| "default".to_string());
            // Build DecodingKey from JWK params
            if let Ok(k) = build_decoding_key(&key.params) {
                keys.insert(kid, k);
            }
        }

        let mut inner = self.inner.write().await;
        inner.keys = keys;
        inner.fetched_at = Instant::now();
        Ok(())
    }

    async fn maybe_refresh(&self) {
        let needs_refresh = {
            let inner = self.inner.read().await;
            inner.fetched_at.elapsed() > JWKS_CACHE_TTL
        };
        if needs_refresh {
            let _ = self.refresh().await;
        }
    }

    pub async fn validate(&self, token: &str, required_project_id: &str) -> Result<Claims> {
        self.maybe_refresh().await;

        let header = decode_header(token)?;
        let kid = header.kid.unwrap_or_else(|| "default".to_string());

        let inner = self.inner.read().await;
        let key = inner.keys.get(&kid)
            .ok_or_else(|| anyhow!("unknown key id: {}", kid))?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&["enzarb-agent"]);

        let data: TokenData<Claims> = decode(token, key, &validation)?;
        let claims = data.claims;

        // Verify the token authorizes access to this specific project
        if !claims.projects.contains_key(required_project_id) {
            return Err(anyhow!("token does not authorize project {}", required_project_id));
        }

        Ok(claims)
    }
}

fn build_decoding_key(params: &serde_json::Value) -> Result<DecodingKey> {
    // RSA key from JWK n/e parameters
    if let (Some(n), Some(e)) = (
        params.get("n").and_then(|v| v.as_str()),
        params.get("e").and_then(|v| v.as_str()),
    ) {
        return Ok(DecodingKey::from_rsa_components(n, e)?);
    }
    Err(anyhow!("unsupported JWK key format"))
}
