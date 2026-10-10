use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Verify a GitHub `X-Hub-Signature-256` header (`sha256=<hex>`) over `body`.
/// The comparison is constant-time (`Mac::verify_slice`).
pub fn verify(secret: &[u8], body: &[u8], header: Option<&str>) -> bool {
    let Some(hexsig) = header.and_then(|h| h.strip_prefix("sha256=")) else {
        return false;
    };
    let Ok(sig) = hex::decode(hexsig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&sig).is_ok()
}

#[cfg(test)]
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_signature() {
        let h = sign(b"s3cret", b"{}");
        assert!(verify(b"s3cret", b"{}", Some(&h)));
    }

    #[test]
    fn invalid_signatures() {
        let h = sign(b"s3cret", b"{}");
        assert!(!verify(b"other", b"{}", Some(&h)));
        assert!(!verify(b"s3cret", b"{ }", Some(&h)));
        assert!(!verify(b"s3cret", b"{}", None));
        assert!(!verify(b"s3cret", b"{}", Some("sha256=zz")));
        assert!(!verify(
            b"s3cret",
            b"{}",
            Some(h.trim_start_matches("sha256="))
        ));
        assert!(!verify(b"s3cret", b"{}", Some("sha256=")));
    }
}
