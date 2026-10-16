//! Named secrets (ADR-0022 §8): user, Project and instance scope, AES-256-GCM in `engine.secrets`
//! with the key from `ENZARB_SECRETS_KEY` (32 bytes, base64). Ciphertext = nonce(12) || ct+tag.

use std::collections::BTreeMap;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use enzarb_flow::format::{Secret, SecretScope};
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
        out.extend(
            c.encrypt(&nonce, plaintext.as_bytes())
                .expect("aes-gcm encrypt"),
        );
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

/// The per-user Claude token store (`app.claude_credentials`, written by the UI's credential
/// capture flow) as the source of one user-scope secret name.
#[derive(Clone, Debug)]
pub struct ClaudeSource {
    /// The user-scope secret name it answers (`ENZARB_CLAUDE_SECRET_NAME`, default `claude_token`).
    pub name: String,
    pub keyring: crate::claude_credential::ClaudeKeyring,
}

/// Resolved secrets of one step: env vars and files (path relative to the primary workspace).
#[derive(Default)]
pub struct Resolved {
    pub env: BTreeMap<String, String>,
    pub files: BTreeMap<String, String>,
    /// `scope:name` of each secret that has no value.
    pub missing: Vec<String>,
}

/// Resolves declared secrets, each in its own scope: `user` (the task's user), `project` or
/// `instance`. At user scope the user's stored Claude credential answers `claude.name` when
/// `engine.secrets` has no row for it. Never another user's value.
pub async fn resolve(
    pool: &PgPool,
    key: Option<&SecretsKey>,
    claude: Option<&ClaudeSource>,
    user: Option<&str>,
    project: &str,
    decls: &[Secret],
) -> Result<Resolved, String> {
    let mut out = Resolved::default();
    for d in decls {
        let (scope, owner) = match d.scope {
            SecretScope::User => ("user", user),
            SecretScope::Project => ("project", Some(project)),
            SecretScope::Instance => ("instance", Some("")),
        };
        let value = match owner {
            None => None,
            Some(owner) => lookup(pool, key, claude, scope, owner, &d.name).await?,
        };
        let Some(v) = value else {
            out.missing.push(format!("{scope}:{}", d.name));
            continue;
        };
        if let Some(e) = &d.env {
            out.env.insert(e.clone(), v.clone());
        }
        if let Some(f) = &d.file {
            out.files.insert(f.clone(), v.clone());
        }
        if d.env.is_none() && d.file.is_none() {
            out.env.insert(d.name.clone(), v);
        }
    }
    Ok(out)
}

async fn lookup(
    pool: &PgPool,
    key: Option<&SecretsKey>,
    claude: Option<&ClaudeSource>,
    scope: &str,
    owner: &str,
    name: &str,
) -> Result<Option<String>, String> {
    let row: Option<(Option<Vec<u8>>, Option<String>)> = sqlx::query_as(
        "SELECT ciphertext, pending FROM engine.secrets WHERE scope = $1 AND owner = $2 AND name = $3",
    )
    .bind(scope)
    .bind(owner)
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    match row {
        Some((_, Some(p))) => return Ok(Some(p)),
        Some((Some(ct), None)) => {
            let k = key.ok_or("ENZARB_SECRETS_KEY is not set")?;
            return k.decrypt(&ct).map(Some);
        }
        _ => {}
    }
    if scope == "user"
        && let Some(c) = claude
        && c.name == name
    {
        match crate::claude_credential::fetch(pool, owner).await {
            Ok(Some((key_id, ct))) => {
                return c
                    .keyring
                    .decrypt(owner, &key_id, &ct)
                    .map(Some)
                    .map_err(|e| format!("stored Claude credential of {owner}: {e}"));
            }
            Ok(None) => {}
            // The UI's schema may be absent (tests, a fresh install).
            Err(e) => tracing::debug!(error = %e, "claude_credential_get unavailable"),
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_wrong_key() {
        let k =
            SecretsKey::from_base64(&base64::engine::general_purpose::STANDARD.encode([7u8; 32]))
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
