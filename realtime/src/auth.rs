//! Browser JWT (contracts section 7 pattern): HS256, `aud = "enzarb-realtime"`, short-lived.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, errors::ErrorKind};
use serde::{Deserialize, Serialize};

pub const AUDIENCE: &str = "enzarb-realtime";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Claims {
    pub sub: String,
    /// Project names the user can see.
    pub projects: Vec<String>,
    pub exp: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iat: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    Invalid,
    Expired,
    TtlTooLong,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Validate signature (HS256 only), audience and expiry (no leeway).
/// When `iat` is present, `exp - iat` must not exceed `max_ttl`.
pub fn validate(token: &str, secret: &[u8], max_ttl: Duration) -> Result<Claims, AuthError> {
    let mut v = Validation::new(Algorithm::HS256);
    v.set_audience(&[AUDIENCE]);
    v.set_required_spec_claims(&["exp", "aud", "sub"]);
    v.leeway = 0;
    let claims = decode::<Claims>(token, &DecodingKey::from_secret(secret), &v)
        .map_err(|e| match e.kind() {
            ErrorKind::ExpiredSignature => AuthError::Expired,
            _ => AuthError::Invalid,
        })?
        .claims;
    if let Some(iat) = claims.iat
        && claims.exp.saturating_sub(iat) > max_ttl.as_secs()
    {
        return Err(AuthError::TtlTooLong);
    }
    Ok(claims)
}

/// When the connection must close: the token's `exp`, but never later than `max_ttl` from now
/// (so a token minted with a far-future `exp` and no `iat` still cannot hold a socket open).
pub fn lifetime(claims: &Claims, max_ttl: Duration) -> Duration {
    Duration::from_secs(claims.exp.saturating_sub(now())).min(max_ttl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::json;

    const SECRET: &[u8] = b"test-secret";
    const TTL: Duration = Duration::from_secs(300);

    fn mint(secret: &[u8], aud: &str, exp: u64, iat: Option<u64>) -> String {
        let mut c = json!({"sub":"alice","projects":["enzarb"],"aud":aud,"exp":exp});
        if let Some(i) = iat {
            c["iat"] = json!(i);
        }
        encode(&Header::default(), &c, &EncodingKey::from_secret(secret)).unwrap()
    }

    #[test]
    fn accepts_valid() {
        let t = mint(SECRET, AUDIENCE, now() + 60, Some(now()));
        let c = validate(&t, SECRET, TTL).unwrap();
        assert_eq!(c.sub, "alice");
        assert_eq!(c.projects, ["enzarb"]);
    }

    #[test]
    fn rejects_wrong_audience() {
        let t = mint(SECRET, "enzarb-sandbox-proxy", now() + 60, None);
        assert_eq!(validate(&t, SECRET, TTL), Err(AuthError::Invalid));
    }

    #[test]
    fn rejects_expired() {
        let t = mint(SECRET, AUDIENCE, now() - 5, None);
        assert_eq!(validate(&t, SECRET, TTL), Err(AuthError::Expired));
    }

    #[test]
    fn rejects_bad_signature() {
        let t = mint(b"other", AUDIENCE, now() + 60, None);
        assert_eq!(validate(&t, SECRET, TTL), Err(AuthError::Invalid));
    }

    #[test]
    fn rejects_garbage_and_wrong_alg() {
        assert_eq!(validate("nope", SECRET, TTL), Err(AuthError::Invalid));
        let c = json!({"sub":"a","projects":[],"aud":AUDIENCE,"exp":now()+60});
        let t = encode(
            &Header::new(Algorithm::HS512),
            &c,
            &EncodingKey::from_secret(SECRET),
        )
        .unwrap();
        assert_eq!(validate(&t, SECRET, TTL), Err(AuthError::Invalid));
    }

    #[test]
    fn rejects_missing_projects() {
        let c = json!({"sub":"a","aud":AUDIENCE,"exp":now()+60});
        let t = encode(&Header::default(), &c, &EncodingKey::from_secret(SECRET)).unwrap();
        assert_eq!(validate(&t, SECRET, TTL), Err(AuthError::Invalid));
    }

    #[test]
    fn rejects_long_ttl_and_caps_lifetime() {
        let t = mint(SECRET, AUDIENCE, now() + 3600, Some(now()));
        assert_eq!(validate(&t, SECRET, TTL), Err(AuthError::TtlTooLong));
        let c = Claims {
            sub: "a".into(),
            projects: vec![],
            exp: now() + 3600,
            iat: None,
        };
        assert_eq!(lifetime(&c, TTL), TTL);
    }
}
