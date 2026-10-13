//! Storage of encrypted Claude tokens (`app_api.claude_credential_*`,
//! database.md) and the [`Credentials`] service the relay uses.
//!
//! Errors carry only a class (SQLSTATE or error kind), never parameters, so a
//! ciphertext or owner list can never end up in a log line through them.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use enzarb_proto::Secret;
use sqlx::{
    PgPool, Row,
    postgres::{PgConnectOptions, PgPoolOptions},
};

use crate::credential::{CryptoError, Keyring};

#[derive(Debug, Clone, thiserror::Error)]
#[error("credential store: {0}")]
pub struct StoreError(pub String);

impl From<sqlx::Error> for StoreError {
    fn from(e: sqlx::Error) -> Self {
        let class = match &e {
            sqlx::Error::Database(d) => format!("database error {}", d.code().unwrap_or_default()),
            sqlx::Error::PoolTimedOut => "pool timed out".into(),
            sqlx::Error::Io(io) => format!("io {}", io.kind()),
            sqlx::Error::Tls(_) => "tls".into(),
            sqlx::Error::Protocol(_) => "protocol".into(),
            sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode".into(),
            _ => "other".into(),
        };
        StoreError(class)
    }
}

/// A stored row. `Debug` shows the key id and ciphertext length only.
#[derive(Clone, PartialEq, Eq)]
pub struct Stored {
    pub key_id: String,
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("key_id", &self.key_id)
            .field("ciphertext_len", &self.ciphertext.len())
            .finish()
    }
}

#[async_trait]
pub trait CredentialStore: Send + Sync + 'static {
    /// Upsert (or insert only when absent). Returns whether a row now exists.
    async fn put(
        &self,
        owner: &str,
        key_id: &str,
        ciphertext: &[u8],
        only_if_absent: bool,
    ) -> Result<bool, StoreError>;
    async fn delete(&self, owner: &str) -> Result<(), StoreError>;
    async fn get(&self, owner: &str) -> Result<Option<Stored>, StoreError>;
    /// `(owner, key_id)` of rows whose key is not `active`.
    async fn list_stale(&self, active: &str) -> Result<Vec<(String, String)>, StoreError>;
    /// Compare-and-set on `old_key_id`. Returns whether the row was updated.
    async fn rekey(
        &self,
        owner: &str,
        old_key_id: &str,
        new_key_id: &str,
        ciphertext: &[u8],
    ) -> Result<bool, StoreError>;
}

/// Postgres, as role `enzarb_sandbox_proxy`. Connection settings come from
/// `DATABASE_URL` and the `PG*` variables (pgop credentials Secret).
pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Lazy pool: the proxy starts (and serves terminals) while the database
    /// is unreachable; credential operations then fail.
    pub fn from_env() -> anyhow::Result<Self> {
        let opts: PgConnectOptions = match std::env::var("DATABASE_URL") {
            Ok(u) if !u.is_empty() => u.parse()?,
            _ => PgConnectOptions::new(),
        };
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(opts);
        Ok(Self { pool })
    }
}

#[async_trait]
impl CredentialStore for PgStore {
    async fn put(
        &self,
        owner: &str,
        key_id: &str,
        ciphertext: &[u8],
        only_if_absent: bool,
    ) -> Result<bool, StoreError> {
        Ok(
            sqlx::query_scalar("SELECT app_api.claude_credential_put($1, $2, $3, $4)")
                .bind(owner)
                .bind(key_id)
                .bind(ciphertext)
                .bind(only_if_absent)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    async fn delete(&self, owner: &str) -> Result<(), StoreError> {
        sqlx::query("SELECT app_api.claude_credential_delete($1)")
            .bind(owner)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get(&self, owner: &str) -> Result<Option<Stored>, StoreError> {
        let row = sqlx::query("SELECT key_id, ciphertext FROM app_api.claude_credential_get($1)")
            .bind(owner)
            .fetch_optional(&self.pool)
            .await?;
        Ok(match row {
            Some(r) => Some(Stored {
                key_id: r.try_get("key_id")?,
                ciphertext: r.try_get("ciphertext")?,
            }),
            None => None,
        })
    }

    async fn list_stale(&self, active: &str) -> Result<Vec<(String, String)>, StoreError> {
        let rows =
            sqlx::query("SELECT owner, key_id FROM app_api.claude_credential_list_stale($1)")
                .bind(active)
                .fetch_all(&self.pool)
                .await?;
        rows.into_iter()
            .map(|r| Ok((r.try_get("owner")?, r.try_get("key_id")?)))
            .collect()
    }

    async fn rekey(
        &self,
        owner: &str,
        old_key_id: &str,
        new_key_id: &str,
        ciphertext: &[u8],
    ) -> Result<bool, StoreError> {
        Ok(
            sqlx::query_scalar("SELECT app_api.claude_credential_rekey($1, $2, $3, $4)")
                .bind(owner)
                .bind(old_key_id)
                .bind(new_key_id)
                .bind(ciphertext)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}

/// In-memory store with the same semantics as the SQL functions (tests).
#[derive(Default)]
pub struct MemStore {
    rows: Mutex<BTreeMap<String, Stored>>,
    /// While set, every call fails.
    pub fail: AtomicBool,
    /// Every `put` call as `(owner, only_if_absent)`.
    pub puts: Mutex<Vec<(String, bool)>>,
}

impl MemStore {
    fn check(&self) -> Result<(), StoreError> {
        if self.fail.load(Ordering::SeqCst) {
            Err(StoreError("unavailable".into()))
        } else {
            Ok(())
        }
    }

    pub fn row(&self, owner: &str) -> Option<Stored> {
        self.rows.lock().unwrap().get(owner).cloned()
    }

    pub fn delete_row(&self, owner: &str) {
        self.rows.lock().unwrap().remove(owner);
    }

    pub fn set_row(&self, owner: &str, row: Stored) {
        self.rows.lock().unwrap().insert(owner.into(), row);
    }
}

#[async_trait]
impl CredentialStore for MemStore {
    async fn put(
        &self,
        owner: &str,
        key_id: &str,
        ciphertext: &[u8],
        only_if_absent: bool,
    ) -> Result<bool, StoreError> {
        self.check()?;
        self.puts
            .lock()
            .unwrap()
            .push((owner.into(), only_if_absent));
        let mut rows = self.rows.lock().unwrap();
        if !(only_if_absent && rows.contains_key(owner)) {
            rows.insert(
                owner.into(),
                Stored {
                    key_id: key_id.into(),
                    ciphertext: ciphertext.to_vec(),
                },
            );
        }
        Ok(true)
    }

    async fn delete(&self, owner: &str) -> Result<(), StoreError> {
        self.check()?;
        self.rows.lock().unwrap().remove(owner);
        Ok(())
    }

    async fn get(&self, owner: &str) -> Result<Option<Stored>, StoreError> {
        self.check()?;
        Ok(self.row(owner))
    }

    async fn list_stale(&self, active: &str) -> Result<Vec<(String, String)>, StoreError> {
        self.check()?;
        Ok(self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, r)| r.key_id != active)
            .map(|(o, r)| (o.clone(), r.key_id.clone()))
            .collect())
    }

    async fn rekey(
        &self,
        owner: &str,
        old_key_id: &str,
        new_key_id: &str,
        ciphertext: &[u8],
    ) -> Result<bool, StoreError> {
        self.check()?;
        let mut rows = self.rows.lock().unwrap();
        match rows.get_mut(owner) {
            Some(r) if r.key_id == old_key_id => {
                r.key_id = new_key_id.into();
                r.ciphertext = ciphertext.to_vec();
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("credential store timed out")]
    Timeout,
}

/// Encrypt/decrypt plus storage, keyed by the authenticated owner only.
pub struct Credentials {
    pub store: Arc<dyn CredentialStore>,
    pub keyring: Arc<Keyring>,
    pub timeout: Duration,
}

impl Credentials {
    pub fn new(store: Arc<dyn CredentialStore>, keyring: Arc<Keyring>) -> Self {
        Self {
            store,
            keyring,
            timeout: Duration::from_secs(5),
        }
    }

    async fn bounded<T>(
        &self,
        f: impl Future<Output = Result<T, StoreError>>,
    ) -> Result<T, CredError> {
        Ok(tokio::time::timeout(self.timeout, f)
            .await
            .map_err(|_| CredError::Timeout)??)
    }

    /// Store a captured token for `owner`. `legacy` inserts only when the
    /// owner has none. Returns whether a row now exists.
    pub async fn store(
        &self,
        owner: &str,
        token: &Secret,
        legacy: bool,
    ) -> Result<bool, CredError> {
        let (key_id, blob) = self.keyring.seal(owner, token)?;
        self.bounded(self.store.put(owner, &key_id, &blob, legacy))
            .await
    }

    pub async fn clear(&self, owner: &str) -> Result<(), CredError> {
        self.bounded(self.store.delete(owner)).await
    }

    /// The owner's decrypted token, if any. Fails closed on an unknown key.
    pub async fn token_for(&self, owner: &str) -> Result<Option<Secret>, CredError> {
        match self.bounded(self.store.get(owner)).await? {
            None => Ok(None),
            Some(row) => Ok(Some(self.keyring.open(
                owner,
                &row.key_id,
                &row.ciphertext,
            )?)),
        }
    }
}
