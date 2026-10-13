//! Per-user Claude token (ADR-0004 amendment 2026-10-13, `database.md`).
//!
//! The executor reads the Project owner's row with `app_api.claude_credential_get(owner)` and
//! decrypts it with the keyring mounted from Secret `enzarb-claude-key`
//! (`ENZARB_CLAUDE_KEYRING_DIR`, one file `<key_id>` of 32 raw bytes per key, plus `active`).
//!
//! Byte layout (shared with sandbox-proxy): `ciphertext` = 12-byte nonce ‖ AES-256-GCM output
//! (ciphertext ‖ 16-byte tag); associated data = `"enzarb-claude-v1"` ‖ `0x00` ‖ owner ‖ `0x00`
//! ‖ key_id. Key material and plaintext never appear in `Debug`, errors or logs.

use std::collections::HashMap;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use sqlx::PgPool;

use super::Secret;

const AAD_PREFIX: &[u8] = b"enzarb-claude-v1";
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;

/// Key id to key bytes. Read-only: the executor only decrypts.
#[derive(Clone, Default)]
pub struct ClaudeKeyring {
    keys: HashMap<String, [u8; KEY_LEN]>,
}

impl std::fmt::Debug for ClaudeKeyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut ids: Vec<&str> = self.keys.keys().map(String::as_str).collect();
        ids.sort_unstable();
        f.debug_struct("ClaudeKeyring")
            .field("key_ids", &ids)
            .finish_non_exhaustive()
    }
}

/// Why a stored credential could not be decrypted. Carries no key or ciphertext bytes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecryptError {
    #[error("unknown key id")]
    UnknownKey,
    #[error("ciphertext too short")]
    Truncated,
    #[error("authentication failed")]
    Auth,
    #[error("plaintext is not UTF-8")]
    NotUtf8,
}

/// Why the keyring directory could not be loaded. Never includes key bytes.
#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("cannot read `{path}`: {kind}")]
    Io {
        path: String,
        kind: std::io::ErrorKind,
    },
    #[error("key `{0}` is not {KEY_LEN} bytes")]
    BadLength(String),
    #[error("no keys in `{0}`")]
    Empty(String),
}

fn aad(owner: &str, key_id: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(AAD_PREFIX.len() + owner.len() + key_id.len() + 2);
    v.extend_from_slice(AAD_PREFIX);
    v.push(0);
    v.extend_from_slice(owner.as_bytes());
    v.push(0);
    v.extend_from_slice(key_id.as_bytes());
    v
}

impl ClaudeKeyring {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) one key.
    pub fn with_key(mut self, key_id: impl Into<String>, key: [u8; KEY_LEN]) -> Self {
        self.keys.insert(key_id.into(), key);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Load a mounted keyring Secret: every regular file is a key named by its file name,
    /// except `active` (the id of the active key, used only by writers) and Kubernetes'
    /// hidden `..data` entries. Every key must be exactly 32 bytes.
    pub fn load_dir(dir: &Path) -> Result<Self, KeyringError> {
        let io = |p: &Path, e: std::io::Error| KeyringError::Io {
            path: p.display().to_string(),
            kind: e.kind(),
        };
        let mut ring = Self::new();
        for entry in std::fs::read_dir(dir).map_err(|e| io(dir, e))? {
            let entry = entry.map_err(|e| io(dir, e))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "active" {
                continue;
            }
            let path = entry.path();
            // Follows the Secret volume's symlinks.
            let meta = std::fs::metadata(&path).map_err(|e| io(&path, e))?;
            if !meta.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| io(&path, e))?;
            let key: [u8; KEY_LEN] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| KeyringError::BadLength(name.clone()))?;
            ring.keys.insert(name, key);
        }
        if ring.is_empty() {
            return Err(KeyringError::Empty(dir.display().to_string()));
        }
        Ok(ring)
    }

    /// Decrypt one stored row. An unknown `key_id` fails closed.
    pub fn decrypt(
        &self,
        owner: &str,
        key_id: &str,
        ciphertext: &[u8],
    ) -> Result<Secret, DecryptError> {
        let key = self.keys.get(key_id).ok_or(DecryptError::UnknownKey)?;
        if ciphertext.len() < NONCE_LEN + TAG_LEN {
            return Err(DecryptError::Truncated);
        }
        let (nonce, body) = ciphertext.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new(key.into());
        let aad = aad(owner, key_id);
        let plain = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: &aad,
                },
            )
            .map_err(|_| DecryptError::Auth)?;
        String::from_utf8(plain)
            .map(Secret::new)
            .map_err(|_| DecryptError::NotUtf8)
    }

    /// Encrypt with an explicit nonce, in the stored layout. The executor never writes rows;
    /// this exists for tests and fixtures.
    #[doc(hidden)]
    pub fn encrypt_with_nonce(
        &self,
        owner: &str,
        key_id: &str,
        nonce: [u8; NONCE_LEN],
        plaintext: &str,
    ) -> Option<Vec<u8>> {
        let key = self.keys.get(key_id)?;
        let cipher = Aes256Gcm::new(key.into());
        let aad = aad(owner, key_id);
        let body = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_bytes(),
                    aad: &aad,
                },
            )
            .ok()?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&body);
        Some(out)
    }
}

/// `app_api.claude_credential_get(owner)`: `(key_id, ciphertext)` or `None` when the owner has
/// no row.
pub(crate) async fn fetch(
    pool: &PgPool,
    owner: &str,
) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    sqlx::query_as("SELECT key_id, ciphertext FROM app_api.claude_credential_get($1)")
        .bind(owner)
        .fetch_optional(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector_ring() -> ClaudeKeyring {
        let mut key = [0u8; 32];
        for (i, b) in key.iter_mut().enumerate() {
            *b = i as u8;
        }
        ClaudeKeyring::new().with_key("k1", key)
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Shared test vector from `docs/database.md` ("Byte layout"); sandbox-proxy passes it too.
    const VECTOR: &str = "000102030405060708090a0b3469fb7aab91ef74ec35a7ba9c9d1d1ef7f96b381458aac61771c67de398262ffd";

    #[test]
    fn decrypts_the_shared_test_vector() {
        let got = vector_ring()
            .decrypt("octocat", "k1", &hex(VECTOR))
            .unwrap();
        assert_eq!(got.expose(), "sk-ant-oat01-test");
    }

    #[test]
    fn encrypts_the_shared_test_vector() {
        let mut nonce = [0u8; 12];
        for (i, b) in nonce.iter_mut().enumerate() {
            *b = i as u8;
        }
        let ct = vector_ring()
            .encrypt_with_nonce("octocat", "k1", nonce, "sk-ant-oat01-test")
            .unwrap();
        assert_eq!(ct, hex(VECTOR));
    }

    #[test]
    fn owner_and_key_id_are_bound() {
        let ring = vector_ring();
        let ct = hex(VECTOR);
        assert_eq!(ring.decrypt("mallory", "k1", &ct), Err(DecryptError::Auth));
        let ring2 = ring.clone().with_key("k2", [0u8; 32]);
        assert_eq!(ring2.decrypt("octocat", "k2", &ct), Err(DecryptError::Auth));
    }

    #[test]
    fn unknown_key_wrong_key_truncated_and_tampered_fail_closed() {
        let ct = hex(VECTOR);
        assert_eq!(
            vector_ring().decrypt("octocat", "k9", &ct),
            Err(DecryptError::UnknownKey)
        );
        let wrong = ClaudeKeyring::new().with_key("k1", [7u8; 32]);
        assert_eq!(wrong.decrypt("octocat", "k1", &ct), Err(DecryptError::Auth));
        assert_eq!(
            vector_ring().decrypt("octocat", "k1", &ct[..27]),
            Err(DecryptError::Truncated)
        );
        let mut bad = ct.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(
            vector_ring().decrypt("octocat", "k1", &bad),
            Err(DecryptError::Auth)
        );
    }

    #[test]
    fn debug_shows_no_key_material() {
        let s = format!("{:?}", vector_ring());
        assert!(s.contains("k1"));
        assert!(!s.contains("31"), "{s}");
        assert!(!s.contains("[0"), "{s}");
    }

    #[test]
    fn load_dir_skips_active_and_hidden_and_checks_length() {
        let dir = std::env::temp_dir().join(format!("enzarb-keyring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("..data")).unwrap();
        std::fs::write(dir.join("k1"), [1u8; 32]).unwrap();
        std::fs::write(dir.join("active"), "k1").unwrap();
        let ring = ClaudeKeyring::load_dir(&dir).unwrap();
        assert_eq!(
            format!("{ring:?}"),
            "ClaudeKeyring { key_ids: [\"k1\"], .. }"
        );
        std::fs::write(dir.join("k2"), [1u8; 31]).unwrap();
        assert!(matches!(
            ClaudeKeyring::load_dir(&dir),
            Err(KeyringError::BadLength(k)) if k == "k2"
        ));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(matches!(
            ClaudeKeyring::load_dir(&dir),
            Err(KeyringError::Io { .. })
        ));
    }
}
