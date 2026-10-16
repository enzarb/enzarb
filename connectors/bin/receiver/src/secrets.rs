//! Project secrets as stored by the engine: `ciphertext` = 12-byte nonce ||
//! AES-256-GCM ciphertext+tag, keyed by `ENZARB_SECRETS_KEY` (32 bytes, base64).
//! A secret not yet encrypted carries its plaintext in `pending`.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;

const NONCE: usize = 12;

#[derive(Clone)]
pub struct SecretsKey([u8; 32]);

impl std::fmt::Debug for SecretsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretsKey(..)")
    }
}

impl SecretsKey {
    /// Parses standard base64 of exactly 32 bytes.
    pub fn from_base64(s: &str) -> Result<Self, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|e| format!("not base64: {e}"))?;
        let key: [u8; 32] = bytes
            .try_into()
            .map_err(|b: Vec<u8>| format!("{} bytes, want 32", b.len()))?;
        Ok(Self(key))
    }

    pub fn encrypt(&self, nonce: [u8; NONCE], plaintext: &[u8]) -> Vec<u8> {
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(self.0));
        let mut out = nonce.to_vec();
        out.extend(
            cipher
                .encrypt(&Nonce::from(nonce), plaintext)
                .expect("encryption cannot fail"),
        );
        out
    }

    pub fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        if ciphertext.len() < NONCE + 16 {
            return Err("ciphertext too short".into());
        }
        let (nonce, ct) = ciphertext.split_at(NONCE);
        Aes256Gcm::new(&Key::<Aes256Gcm>::from(self.0))
            .decrypt(
                &Nonce::from(<[u8; NONCE]>::try_from(nonce).expect("split at NONCE")),
                ct,
            )
            .map_err(|_| "decryption failed (wrong key?)".to_string())
    }
}

/// The secret's plaintext: `pending` when set, else `ciphertext` decrypted with `key`.
pub fn plaintext(
    pending: Option<String>,
    ciphertext: Option<Vec<u8>>,
    key: Option<&SecretsKey>,
) -> Result<Vec<u8>, String> {
    if let Some(p) = pending {
        return Ok(p.into_bytes());
    }
    let ct = ciphertext.ok_or("secret has neither pending nor ciphertext")?;
    key.ok_or("ENZARB_SECRETS_KEY is not set")?.decrypt(&ct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_pending() {
        let key =
            SecretsKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([7u8; 32]))
                .unwrap();
        let ct = key.encrypt([1; 12], b"s3cret");
        assert_eq!(
            plaintext(None, Some(ct.clone()), Some(&key)).unwrap(),
            b"s3cret"
        );
        assert_eq!(plaintext(Some("p".into()), None, None).unwrap(), b"p");
        assert!(plaintext(None, Some(ct), None).is_err());
        let other = SecretsKey([8; 32]);
        assert!(other.decrypt(&key.encrypt([1; 12], b"x")).is_err());
        assert!(SecretsKey::from_base64("AAAA").is_err());
    }
}
