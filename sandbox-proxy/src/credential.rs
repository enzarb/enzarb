//! Per-user Claude token encryption (ADR-0004 amendment 2026-10-13,
//! database.md "Byte layout").
//!
//! * A key file holds 32 raw key bytes; the keyring directory (the mounted
//!   `enzarb-claude-key` Secret) holds one file per `<key_id>` plus `active`,
//!   whose content is the id used for new writes.
//! * `ciphertext` = 12-byte nonce ‖ AES-256-GCM output (ciphertext ‖ tag).
//! * Associated data = `"enzarb-claude-v1"` ‖ `0x00` ‖ owner ‖ `0x00` ‖ key_id.
//!
//! Decrypting a row whose `key_id` is not in the keyring fails closed. Key
//! bytes and tokens never appear in `Debug` output or errors.

use std::{collections::BTreeMap, path::Path};

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use enzarb_proto::Secret;
use zeroize::Zeroizing;

pub const AAD_PREFIX: &[u8] = b"enzarb-claude-v1";
pub const NONCE_LEN: usize = 12;
pub const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// The row's key is not in the keyring (fail closed).
    #[error("unknown key_id {0:?}")]
    UnknownKey(String),
    #[error("ciphertext is malformed or does not authenticate")]
    Invalid,
    #[error("encryption failed")]
    Encrypt,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("keyring: {0}")]
    Io(String),
    #[error("keyring: no `active` file")]
    NoActive,
    #[error("keyring: active key {0:?} has no key file")]
    ActiveMissing(String),
    #[error("keyring: key {0:?} is not {KEY_LEN} bytes")]
    BadLength(String),
    #[error("keyring: invalid key id {0:?}")]
    BadId(String),
}

/// The loaded keyring. `Debug` lists key ids only.
pub struct Keyring {
    active: String,
    keys: BTreeMap<String, Zeroizing<[u8; KEY_LEN]>>,
}

impl std::fmt::Debug for Keyring {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyring")
            .field("active", &self.active)
            .field("key_ids", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 253
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && !id.starts_with('.')
}

/// Associated data for `owner` and `key_id`.
pub fn aad(owner: &str, key_id: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(AAD_PREFIX.len() + owner.len() + key_id.len() + 2);
    v.extend_from_slice(AAD_PREFIX);
    v.push(0);
    v.extend_from_slice(owner.as_bytes());
    v.push(0);
    v.extend_from_slice(key_id.as_bytes());
    v
}

impl Keyring {
    /// Build from in-memory keys (tests, tools).
    pub fn from_keys(
        active: &str,
        keys: impl IntoIterator<Item = (String, [u8; KEY_LEN])>,
    ) -> Result<Self, KeyringError> {
        let mut map = BTreeMap::new();
        for (id, k) in keys {
            if !valid_id(&id) {
                return Err(KeyringError::BadId(id));
            }
            map.insert(id, Zeroizing::new(k));
        }
        if !map.contains_key(active) {
            return Err(KeyringError::ActiveMissing(active.into()));
        }
        Ok(Self {
            active: active.into(),
            keys: map,
        })
    }

    /// Load `<dir>/active` and every `<dir>/<key_id>` (Kubernetes' `..data`
    /// bookkeeping entries, which start with a dot, are skipped).
    pub fn load(dir: &Path) -> Result<Self, KeyringError> {
        let io = |e: std::io::Error| KeyringError::Io(e.kind().to_string());
        let active = match std::fs::read_to_string(dir.join("active")) {
            Ok(s) => s.trim().to_string(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(KeyringError::NoActive);
            }
            Err(e) => return Err(io(e)),
        };
        let mut keys = Vec::new();
        for entry in std::fs::read_dir(dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name == "active" || name.starts_with('.') {
                continue;
            }
            if !valid_id(&name) {
                return Err(KeyringError::BadId(name));
            }
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let bytes = Zeroizing::new(std::fs::read(&path).map_err(io)?);
            let k: [u8; KEY_LEN] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| KeyringError::BadLength(name.clone()))?;
            keys.push((name, k));
        }
        Self::from_keys(&active, keys)
    }

    pub fn active_id(&self) -> &str {
        &self.active
    }

    pub fn has(&self, key_id: &str) -> bool {
        self.keys.contains_key(key_id)
    }

    /// Encrypt `token` for `owner` under the active key. Returns
    /// `(key_id, ciphertext)`.
    pub fn seal(&self, owner: &str, token: &Secret) -> Result<(String, Vec<u8>), CryptoError> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let blob = self.seal_with_nonce(&self.active, owner, token, nonce.into())?;
        Ok((self.active.clone(), blob))
    }

    fn seal_with_nonce(
        &self,
        key_id: &str,
        owner: &str,
        token: &Secret,
        nonce: [u8; NONCE_LEN],
    ) -> Result<Vec<u8>, CryptoError> {
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| CryptoError::UnknownKey(key_id.into()))?;
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(**key));
        let aad = aad(owner, key_id);
        let ct = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: token.expose().as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Encrypt)?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt a stored row. Fails closed on an unknown `key_id`.
    pub fn open(&self, owner: &str, key_id: &str, blob: &[u8]) -> Result<Secret, CryptoError> {
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| CryptoError::UnknownKey(key_id.into()))?;
        if blob.len() < NONCE_LEN + TAG_LEN {
            return Err(CryptoError::Invalid);
        }
        let (nonce, ct) = blob.split_at(NONCE_LEN);
        let nonce: [u8; NONCE_LEN] = nonce.try_into().map_err(|_| CryptoError::Invalid)?;
        let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(**key));
        let aad = aad(owner, key_id);
        let pt = Zeroizing::new(
            cipher
                .decrypt(&Nonce::from(nonce), Payload { msg: ct, aad: &aad })
                .map_err(|_| CryptoError::Invalid)?,
        );
        let s = std::str::from_utf8(&pt).map_err(|_| CryptoError::Invalid)?;
        Ok(Secret::new(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq<const N: usize>() -> [u8; N] {
        std::array::from_fn(|i| i as u8)
    }

    fn ring() -> Keyring {
        Keyring::from_keys("k1", [("k1".to_string(), seq::<32>())]).unwrap()
    }

    /// The shared vector from database.md "Byte layout" (also used by workflows).
    #[test]
    fn shared_test_vector() {
        let r = ring();
        let blob = r
            .seal_with_nonce("k1", "octocat", &Secret::new("sk-ant-oat01-test"), seq())
            .unwrap();
        assert_eq!(
            hex::encode(&blob),
            "000102030405060708090a0b3469fb7aab91ef74ec35a7ba9c9d1d1ef7f96b381458aac61771c67de398262ffd"
        );
        assert_eq!(aad("octocat", "k1"), b"enzarb-claude-v1\0octocat\0k1");
        assert_eq!(
            r.open("octocat", "k1", &blob).unwrap().expose(),
            "sk-ant-oat01-test"
        );
    }

    #[test]
    fn round_trip_and_binding() {
        let r = Keyring::from_keys(
            "k2",
            [("k1".to_string(), seq::<32>()), ("k2".to_string(), [7; 32])],
        )
        .unwrap();
        let tok = Secret::new("tok");
        let (kid, blob) = r.seal("alice", &tok).unwrap();
        assert_eq!(kid, "k2");
        assert_eq!(r.open("alice", "k2", &blob).unwrap(), tok);
        // Another owner, another key id, tampering: all fail.
        assert!(matches!(
            r.open("bob", "k2", &blob),
            Err(CryptoError::Invalid)
        ));
        assert!(matches!(
            r.open("alice", "k1", &blob),
            Err(CryptoError::Invalid)
        ));
        let mut bad = blob.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(r.open("alice", "k2", &bad).is_err());
        assert!(r.open("alice", "k2", &blob[..10]).is_err());
        // Nonces are random.
        assert_ne!(r.seal("alice", &tok).unwrap().1, blob);
    }

    #[test]
    fn unknown_key_fails_closed() {
        let r = ring();
        let blob = r.seal("a", &Secret::new("t")).unwrap().1;
        let r2 = Keyring::from_keys("k9", [("k9".to_string(), [1; 32])]).unwrap();
        assert!(matches!(
            r2.open("a", "k1", &blob),
            Err(CryptoError::UnknownKey(k)) if k == "k1"
        ));
    }

    #[test]
    fn load_from_dir() {
        let dir = std::env::temp_dir().join(format!("enzarb-keyring-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("..2026_x")).unwrap();
        assert!(matches!(Keyring::load(&dir), Err(KeyringError::NoActive)));
        std::fs::write(dir.join("active"), "k1\n").unwrap();
        assert!(matches!(
            Keyring::load(&dir),
            Err(KeyringError::ActiveMissing(_))
        ));
        std::fs::write(dir.join("k1"), [1u8; 31]).unwrap();
        assert!(matches!(
            Keyring::load(&dir),
            Err(KeyringError::BadLength(_))
        ));
        std::fs::write(dir.join("k1"), [1u8; 32]).unwrap();
        std::fs::write(dir.join("k0"), [2u8; 32]).unwrap();
        let r = Keyring::load(&dir).unwrap();
        assert_eq!(r.active_id(), "k1");
        assert!(r.has("k0"));
        let dbg = format!("{r:?}");
        assert!(!dbg.contains("[1, 1"), "{dbg}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn debug_is_redacted() {
        let s = Secret::new("sk-ant-oat01-SECRET");
        assert!(!format!("{s:?}").contains("SECRET"));
        assert!(!format!("{:?}", ring()).contains("[0, 1, 2"));
    }
}
