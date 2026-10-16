//! Named secrets (ADR-0022 §8): user, Project and instance scope, AES-256-GCM in `engine.secrets`
//! with the key from `ENZARB_SECRETS_KEY` (32 bytes, base64). Ciphertext = nonce(12) || ct+tag.

use std::collections::BTreeMap;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use sqlx::PgPool;

#[derive(Clone)]
pub struct SecretsKey(Key<Aes256Gcm>);

impl std::fmt::Debug for SecretsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretsKey(<redacted>)")
    }
}

impl SecretsKey {
    pub fn from_base64(s: &str) -> Result<Self, String> {
        let b = base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map_err(|e| format!("ENZARB_SECRETS_KEY: {e}"))?;
        if b.len() != 32 {
            return Err("ENZARB_SECRETS_KEY must be 32 bytes".into());
        }
        Ok(Self(*Key::<Aes256Gcm>::from_slice(&b)))
    }

    pub fn encrypt(&self, plaintext: &str) -> Vec<u8> {
        let c = Aes256Gcm::new(&self.0);
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut out = nonce.to_vec();
        out.extend(c.encrypt(&nonce, plaintext.as_bytes()).expect("aes-gcm encrypt"));
        out
    }

    pub fn decrypt(&self, data: &[u8]) -> Result<String, String> {
        if data.len() < 12 + 16 {
            return Err("ciphertext too short".into());
        }
        let c = Aes256Gcm::new(&self.0);
        let pt = c
            .decrypt(Nonce::from_slice(&data[..12]), &data[12..])
            .map_err(|_| "cannot decrypt (wrong ENZARB_SECRETS_KEY?)".to_string())?;
        String::from_utf8(pt).map_err(|e| e.to_string())
    }
}

/// Encrypts secrets written by `engine_api.secret_set` (stored as `pending`). Returns how many.
pub async fn encrypt_pending(pool: &PgPool, key: &SecretsKey) -> Result<u64, sqlx::Error> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT scope, owner, name, pending FROM engine.secrets WHERE pending IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut n = 0;
    for (scope, owner, name, pending) in rows {
        let ct = key.encrypt(&pending);
        n += sqlx::query(
            "UPDATE engine.secrets SET ciphertext = $4, pending = NULL
             WHERE scope = $1 AND owner = $2 AND name = $3 AND pending = $5",
        )
        .bind(&scope)
        .bind(&owner)
        .bind(&name)
        .bind(ct)
        .bind(&pending)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(n)
}

/// Resolves `names`, most specific scope first: user (`user`), Project, instance. Returns the
/// values found and the names that are missing.
pub async fn resolve(
    pool: &PgPool,
    key: Option<&SecretsKey>,
    user: Option<&str>,
    project: &str,
    names: &[String],
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let mut found = BTreeMap::new();
    let mut missing = Vec::new();
    for name in names {
        let row: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
            "SELECT ciphertext, pending FROM engine.secrets
             WHERE name = $1 AND ((scope = 'user' AND owner = $2) OR (scope = 'project' AND owner = $3)
                                  OR (scope = 'instance'))
             ORDER BY CASE scope WHEN 'user' THEN 0 WHEN 'project' THEN 1 ELSE 2 END LIMIT 1",
        )
        .bind(name)
        .bind(user.unwrap_or("\u{0}"))
        .bind(project)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?;
        match row {
            Some((_, Some(p))) => {
                found.insert(name.clone(), p);
            }
            Some((Some(ct), None)) => {
                let k = key.ok_or("ENZARB_SECRETS_KEY is not set")?;
                found.insert(name.clone(), k.decrypt(&ct)?);
            }
            _ => missing.push(name.clone()),
        }
    }
    Ok((found, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_wrong_key() {
        let k = SecretsKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([7u8; 32]))
            .unwrap();
        let ct = k.encrypt("hunter2");
        assert_eq!(k.decrypt(&ct).unwrap(), "hunter2");
        let other =
            SecretsKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([8u8; 32]))
                .unwrap();
        assert!(other.decrypt(&ct).is_err());
        assert!(SecretsKey::from_base64("c2hvcnQ=").is_err());
    }
}
