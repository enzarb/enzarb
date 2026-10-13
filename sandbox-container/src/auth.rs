//! Bearer-token authentication of the mux upgrade.

use std::path::Path;

use subtle::ConstantTimeEq;

/// Why a request was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// No/malformed `Authorization` header or wrong token.
    Unauthorized,
    /// The token file is missing or empty; every request is refused.
    NotConfigured,
}

/// Extract the token from an `Authorization: Bearer <token>` header value.
pub fn bearer(header: &str) -> Option<&str> {
    let (scheme, rest) = header.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| rest.trim())
        .filter(|t| !t.is_empty())
}

/// Constant-time equality of the presented and expected tokens.
pub fn tokens_match(presented: &str, expected: &str) -> bool {
    presented.as_bytes().ct_eq(expected.as_bytes()).into()
}

/// Check `header` (the raw `Authorization` value, if any) against the token
/// file. The file is read on every call, so a rotated Secret takes effect
/// without a restart.
pub fn check(header: Option<&str>, token_file: &Path) -> Result<(), AuthError> {
    let expected = match std::fs::read_to_string(token_file) {
        Ok(s) => s.trim().to_owned(),
        Err(e) => {
            tracing::error!(path = %token_file.display(), error = %e, "cannot read auth token file");
            return Err(AuthError::NotConfigured);
        }
    };
    if expected.is_empty() {
        tracing::error!(path = %token_file.display(), "auth token file is empty");
        return Err(AuthError::NotConfigured);
    }
    match header.and_then(bearer) {
        Some(p) if tokens_match(p, &expected) => Ok(()),
        _ => Err(AuthError::Unauthorized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bearer() {
        assert_eq!(bearer("Bearer abc"), Some("abc"));
        assert_eq!(bearer("bearer abc"), Some("abc"));
        assert_eq!(bearer("Basic abc"), None);
        assert_eq!(bearer("Bearer "), None);
        assert_eq!(bearer("Bearer"), None);
    }

    #[test]
    fn checks_against_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("token");
        assert_eq!(check(Some("Bearer x"), &f), Err(AuthError::NotConfigured));
        std::fs::write(&f, "s3cret\n").unwrap();
        assert_eq!(check(Some("Bearer s3cret"), &f), Ok(()));
        assert_eq!(
            check(Some("Bearer s3cre"), &f),
            Err(AuthError::Unauthorized)
        );
        assert_eq!(check(None, &f), Err(AuthError::Unauthorized));
        std::fs::write(&f, "\n").unwrap();
        assert_eq!(check(Some("Bearer "), &f), Err(AuthError::NotConfigured));
    }
}
