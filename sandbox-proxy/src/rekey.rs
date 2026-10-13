//! `enzarb-sandbox-proxy rekey`: re-encrypt rows still under an old key
//! (database.md "Key rotation"). Idempotent and resumable; logs owners, key
//! ids and counts only.

use std::collections::BTreeMap;

use crate::{credential::Keyring, store::CredentialStore};

/// A row whose compare-and-set lost to a concurrent change is retried at most
/// this many times per run.
const MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub migrated: u64,
    /// Rows changed concurrently (CAS lost) and not resolved within this run.
    pub skipped: u64,
    pub failed: u64,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.failed == 0
    }
}

pub async fn run(store: &dyn CredentialStore, keyring: &Keyring) -> anyhow::Result<Report> {
    let active = keyring.active_id().to_string();
    let mut report = Report::default();
    // owner -> attempts made in this run; a failed row is not retried.
    let mut attempts: BTreeMap<String, u32> = BTreeMap::new();
    let mut failed: std::collections::BTreeSet<String> = Default::default();
    loop {
        let stale = store.list_stale(&active).await?;
        let todo: Vec<_> = stale
            .into_iter()
            .filter(|(o, _)| {
                !failed.contains(o) && attempts.get(o).copied().unwrap_or(0) < MAX_ATTEMPTS
            })
            .collect();
        if todo.is_empty() {
            break;
        }
        for (owner, _) in todo {
            *attempts.entry(owner.clone()).or_default() += 1;
            let row = match store.get(&owner).await {
                Ok(Some(r)) if r.key_id != active => r,
                Ok(_) => continue, // deleted or already migrated meanwhile
                Err(e) => {
                    tracing::warn!(owner, error = %e, "rekey: read failed");
                    failed.insert(owner);
                    continue;
                }
            };
            let token = match keyring.open(&owner, &row.key_id, &row.ciphertext) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(owner, key_id = row.key_id, error = %e, "rekey: decrypt failed");
                    failed.insert(owner);
                    continue;
                }
            };
            let (new_id, blob) = match keyring.seal(&owner, &token) {
                Ok(x) => x,
                Err(e) => {
                    tracing::warn!(owner, error = %e, "rekey: encrypt failed");
                    failed.insert(owner);
                    continue;
                }
            };
            match store.rekey(&owner, &row.key_id, &new_id, &blob).await {
                Ok(true) => {
                    tracing::info!(owner, from = row.key_id, to = new_id, "rekeyed");
                    report.migrated += 1;
                }
                Ok(false) => tracing::info!(owner, "rekey: row changed meanwhile; retrying"),
                Err(e) => {
                    tracing::warn!(owner, error = %e, "rekey: update failed");
                    failed.insert(owner);
                }
            }
        }
    }
    report.failed = failed.len() as u64;
    report.skipped = store
        .list_stale(&active)
        .await?
        .iter()
        .filter(|(o, _)| !failed.contains(o))
        .count() as u64;
    tracing::info!(
        migrated = report.migrated,
        skipped = report.skipped,
        failed = report.failed,
        "rekey finished"
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use enzarb_proto::Secret;

    use super::*;
    use crate::store::{MemStore, Stored};

    fn ring(active: &str) -> Keyring {
        Keyring::from_keys(
            active,
            [("k1".to_string(), [1; 32]), ("k2".to_string(), [2; 32])],
        )
        .unwrap()
    }

    #[tokio::test]
    async fn migrates_then_is_a_no_op() {
        let old = ring("k1");
        let s = MemStore::default();
        for o in ["alice", "bob"] {
            let (k, b) = old.seal(o, &Secret::new(format!("tok-{o}"))).unwrap();
            s.set_row(
                o,
                Stored {
                    key_id: k,
                    ciphertext: b,
                },
            );
        }
        let new = ring("k2");
        let r = run(&s, &new).await.unwrap();
        assert_eq!(
            r,
            Report {
                migrated: 2,
                skipped: 0,
                failed: 0
            }
        );
        let row = s.row("alice").unwrap();
        assert_eq!(row.key_id, "k2");
        assert_eq!(
            new.open("alice", "k2", &row.ciphertext).unwrap().expose(),
            "tok-alice"
        );
        assert_eq!(run(&s, &new).await.unwrap(), Report::default());
    }

    #[tokio::test]
    async fn unknown_old_key_fails_but_others_continue() {
        let s = MemStore::default();
        s.set_row(
            "ghost",
            Stored {
                key_id: "k0".into(),
                ciphertext: vec![0; 40],
            },
        );
        let old = ring("k1");
        let (k, b) = old.seal("alice", &Secret::new("t")).unwrap();
        s.set_row(
            "alice",
            Stored {
                key_id: k,
                ciphertext: b,
            },
        );
        let r = run(&s, &ring("k2")).await.unwrap();
        assert_eq!(r.migrated, 1);
        assert_eq!(r.failed, 1);
        assert!(!r.ok());
        assert_eq!(s.row("ghost").unwrap().key_id, "k0");
    }

    #[tokio::test]
    async fn store_down_is_an_error() {
        let s = MemStore::default();
        s.fail.store(true, Ordering::SeqCst);
        assert!(run(&s, &ring("k2")).await.is_err());
    }
}
