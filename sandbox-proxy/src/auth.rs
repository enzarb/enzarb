//! Session JWT validation (contracts §7).

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;

/// Expected `aud` claim.
pub const AUDIENCE: &str = "enzarb-sandbox-proxy";
/// Tokens live at most 5 minutes after issue, so `exp` can never be further
/// than this from now; anything longer-lived is refused.
pub const MAX_LIFETIME_SECS: u64 = 300;
const LEEWAY_SECS: u64 = 5;

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Claims {
    /// GitHub login of the user.
    pub sub: String,
    pub project: String,
    pub exp: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("invalid token: {0}")]
    InvalidToken(String),
    #[error("token is not valid for this project")]
    ProjectMismatch,
}

#[derive(Clone)]
pub struct JwtValidator {
    key: DecodingKey,
    validation: Validation,
}

impl JwtValidator {
    pub fn new(secret: &[u8]) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_audience(&[AUDIENCE]);
        validation.set_required_spec_claims(&["exp", "aud", "sub"]);
        validation.leeway = LEEWAY_SECS;
        Self {
            key: DecodingKey::from_secret(secret),
            validation,
        }
    }

    /// Validate signature, `aud`, `exp` and that the `project` claim equals
    /// `path_project`.
    pub fn validate(&self, token: &str, path_project: &str) -> Result<Claims, AuthError> {
        self.validate_at(token, path_project, now_secs())
    }

    pub fn validate_at(
        &self,
        token: &str,
        path_project: &str,
        now: u64,
    ) -> Result<Claims, AuthError> {
        let data = decode::<Claims>(token, &self.key, &self.validation)
            .map_err(|e| AuthError::InvalidToken(format!("{:?}", e.kind())))?;
        let c = data.claims;
        if c.exp > now + MAX_LIFETIME_SECS + LEEWAY_SECS {
            return Err(AuthError::InvalidToken("lifetime too long".into()));
        }
        if c.sub.is_empty() || c.project != path_project {
            return Err(AuthError::ProjectMismatch);
        }
        Ok(c)
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Authorization: only the Project's owner may connect.
pub fn authorize(sub: &str, owner: &str) -> bool {
    !owner.is_empty() && sub == owner
}

/// Project names are DNS labels of at most 54 characters.
pub fn valid_project_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 54
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;

    #[derive(Serialize)]
    struct Out<'a> {
        sub: &'a str,
        project: &'a str,
        aud: &'a str,
        exp: u64,
    }

    const SECRET: &[u8] = b"test-secret-test-secret-test-secret";

    fn tok(secret: &[u8], aud: &str, project: &str, exp: u64) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            &Out {
                sub: "ruckc",
                project,
                aud,
                exp,
            },
            &EncodingKey::from_secret(secret),
        )
        .unwrap()
    }

    #[test]
    fn accepts_valid() {
        let now = now_secs();
        let v = JwtValidator::new(SECRET);
        let c = v
            .validate(&tok(SECRET, AUDIENCE, "p1", now + 120), "p1")
            .unwrap();
        assert_eq!(c.sub, "ruckc");
    }

    #[test]
    fn rejects_bad_signature_aud_exp_project() {
        let now = now_secs();
        let v = JwtValidator::new(SECRET);
        assert!(matches!(
            v.validate(&tok(b"other-secret", AUDIENCE, "p1", now + 60), "p1"),
            Err(AuthError::InvalidToken(_))
        ));
        assert!(matches!(
            v.validate(&tok(SECRET, "someone-else", "p1", now + 60), "p1"),
            Err(AuthError::InvalidToken(_))
        ));
        assert!(matches!(
            v.validate(&tok(SECRET, AUDIENCE, "p1", now - 120), "p1"),
            Err(AuthError::InvalidToken(_))
        ));
        assert_eq!(
            v.validate(&tok(SECRET, AUDIENCE, "p1", now + 60), "p2"),
            Err(AuthError::ProjectMismatch)
        );
        assert!(matches!(
            v.validate(&tok(SECRET, AUDIENCE, "p1", now + 3600), "p1"),
            Err(AuthError::InvalidToken(_))
        ));
        assert!(v.validate("garbage", "p1").is_err());
    }

    #[test]
    fn rejects_alg_none_and_other_algs() {
        let now = now_secs();
        let v = JwtValidator::new(SECRET);
        let t = encode(
            &Header::new(Algorithm::HS384),
            &Out {
                sub: "ruckc",
                project: "p1",
                aud: AUDIENCE,
                exp: now + 60,
            },
            &EncodingKey::from_secret(SECRET),
        )
        .unwrap();
        assert!(v.validate(&t, "p1").is_err());
    }

    #[test]
    fn authorize_owner_only() {
        assert!(authorize("ruckc", "ruckc"));
        assert!(!authorize("mallory", "ruckc"));
        assert!(!authorize("", ""));
    }

    #[test]
    fn project_names() {
        assert!(valid_project_name("enzarb"));
        assert!(valid_project_name("a-b1"));
        assert!(!valid_project_name(""));
        assert!(!valid_project_name("Foo"));
        assert!(!valid_project_name("a/b"));
        assert!(!valid_project_name("-a"));
        assert!(!valid_project_name(&"a".repeat(55)));
    }
}
