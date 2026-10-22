//! Webhook signature verification (`X-Hub-Signature-256`).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// Checks `X-Hub-Signature-256` (`sha256=<hex>`) against the raw request body.
///
/// The MAC is recomputed and compared in constant time. Returns false for a
/// missing prefix, bad hex or a wrong signature.
pub fn verify_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    let Some(hex_sig) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    let Ok(sig) = hex::decode(hex_sig) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    let expected = mac.finalize().into_bytes();
    expected.as_slice().ct_eq(&sig).into()
}

/// Computes the `X-Hub-Signature-256` header value for `body`. Useful for
/// tests and local relays.
pub fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Example from GitHub's "Validating webhook deliveries" docs.
    const SECRET: &[u8] = b"It's a Secret to Everybody";
    const BODY: &[u8] = b"Hello, World!";
    const SIG: &str = "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";

    #[test]
    fn github_documented_vector() {
        assert!(verify_signature(SECRET, BODY, SIG));
        assert_eq!(sign(SECRET, BODY), SIG);
    }

    #[test]
    fn rejects_bad_signatures() {
        assert!(!verify_signature(SECRET, b"Hello, World?", SIG));
        assert!(!verify_signature(b"other", BODY, SIG));
        assert!(!verify_signature(SECRET, BODY, ""));
        assert!(!verify_signature(SECRET, BODY, "sha256="));
        assert!(!verify_signature(SECRET, BODY, "sha256=zz"));
        assert!(!verify_signature(SECRET, BODY, &SIG["sha256=".len()..]));
        assert!(!verify_signature(
            SECRET,
            BODY,
            &SIG.replace("sha256=", "sha1=")
        ));
        assert!(!verify_signature(SECRET, BODY, &SIG[..SIG.len() - 2]));
    }
}
