use anyhow::{Result, anyhow};
use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes

#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<RwLock<CacheInner>>,
    jwks_url: String,
    revoked_url: String,
    issuer: String,
}

struct CacheInner {
    keys: HashMap<String, DecodingKey>,
    revoked_jtis: HashSet<String>,
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
    pub jti: String,
    pub exp: u64,
    pub iat: u64,
    pub projects: HashMap<String, Vec<String>>,
}

/// The set of permissions granted to the calling token for the validated project.
/// Stored in axum request Extensions by auth_middleware.
#[derive(Clone, Debug)]
pub struct ProjectPermissions(pub Vec<String>);

impl ProjectPermissions {
    pub fn require(&self, perm: &str) -> Result<(), axum::http::StatusCode> {
        if self.0.iter().any(|p| p == perm) {
            Ok(())
        } else {
            Err(axum::http::StatusCode::FORBIDDEN)
        }
    }
}

#[derive(Debug, Deserialize)]
struct RevokedJtis {
    revoked: Vec<String>,
}

impl JwksCache {
    pub async fn new(jwks_url: String, revoked_url: String, issuer: String) -> Result<Self> {
        let cache = JwksCache {
            inner: Arc::new(RwLock::new(CacheInner {
                keys: HashMap::new(),
                revoked_jtis: HashSet::new(),
                fetched_at: Instant::now() - JWKS_CACHE_TTL - Duration::from_secs(1),
            })),
            jwks_url,
            revoked_url,
            issuer,
        };
        cache.refresh().await?;
        Ok(cache)
    }

    pub async fn refresh(&self) -> Result<()> {
        let (jwks_resp, revoked_resp) = tokio::join!(
            reqwest::get(&self.jwks_url),
            reqwest::get(&self.revoked_url)
        );
        let jwks: Jwks = jwks_resp?.json().await?;

        let mut keys = HashMap::new();
        for key in jwks.keys {
            if key.kty != "RSA" && key.kty != "EC" {
                continue;
            }
            let kid = key.kid.unwrap_or_else(|| "default".to_string());
            if let Ok(k) = build_decoding_key(&key.params) {
                keys.insert(kid, k);
            }
        }

        let revoked_jtis: HashSet<String> = match revoked_resp {
            Ok(resp) => match resp.json::<RevokedJtis>().await {
                Ok(r) => r.revoked.into_iter().collect(),
                Err(_) => HashSet::new(),
            },
            Err(_) => HashSet::new(),
        };

        let mut inner = self.inner.write().await;
        inner.keys = keys;
        inner.revoked_jtis = revoked_jtis;
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
        let key = inner
            .keys
            .get(&kid)
            .ok_or_else(|| anyhow!("unknown key id: {}", kid))?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&["enzarb-agent"]);
        validation.set_issuer(&[self.issuer.as_str()]);

        let data: TokenData<Claims> = decode(token, key, &validation)?;
        let claims = data.claims;

        if inner.revoked_jtis.contains(&claims.jti) {
            return Err(anyhow!("token has been revoked"));
        }

        // Verify the token authorizes access to this specific project
        if !claims.projects.contains_key(required_project_id) {
            return Err(anyhow!(
                "token does not authorize project {}",
                required_project_id
            ));
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
