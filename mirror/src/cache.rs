use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Metadata for one cached object (a blob, a manifest-by-digest, or a
/// manifest-by-tag). Persisted in index.json so access history survives
/// restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub size: u64,
    pub last_access: u64,
    pub access_count: u64,
    /// Upstream Content-Type (meaningful for manifests; blobs are octet-stream).
    pub content_type: String,
    /// Canonical digest of the content (sha256:...).
    pub digest: String,
    /// When the entry was fetched from upstream; used for tag TTL only.
    pub fetched_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    entries: HashMap<String, Entry>,
}

/// Disk-backed cache with a size cap and least-recently-used eviction.
///
/// Keys are opaque strings (`blob:<digest>`, `manifest:<digest>`,
/// `tag:<host>/<repo>:<tag>`); each maps to one file under `<root>/data/`.
/// Eviction simply unlinks files: on Linux an already-open file descriptor
/// keeps streaming to its client after the unlink, so no in-flight refcounting
/// is needed.
pub struct Cache {
    root: PathBuf,
    max_bytes: u64,
    low_water: u64,
    state: Mutex<Index>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// File name for a key: keys contain '/' and ':' so hex-encode them flat.
fn key_file(key: &str) -> String {
    hex::encode(key.as_bytes())
}

impl Cache {
    pub fn load(root: impl Into<PathBuf>, max_bytes: u64) -> Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("data")).context("create cache data dir")?;
        std::fs::create_dir_all(root.join("tmp")).context("create cache tmp dir")?;

        let mut index: Index = match std::fs::read(root.join("index.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Index::default(),
        };

        // Reconcile the index against what is actually on disk: files without a
        // row get their mtime as last_access; rows without a file are dropped.
        let mut on_disk: HashMap<String, (u64, u64)> = HashMap::new();
        for dirent in std::fs::read_dir(root.join("data")).context("scan cache data dir")? {
            let dirent = dirent?;
            let meta = dirent.metadata()?;
            let Some(name) = dirent.file_name().to_str().map(String::from) else {
                continue;
            };
            let Ok(raw) = hex::decode(&name) else {
                continue;
            };
            let Ok(key) = String::from_utf8(raw) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or_else(now);
            on_disk.insert(key, (meta.len(), mtime));
        }
        index.entries.retain(|k, _| on_disk.contains_key(k));
        for (key, (size, mtime)) in on_disk {
            index.entries.entry(key).or_insert(Entry {
                size,
                last_access: mtime,
                access_count: 0,
                content_type: "application/octet-stream".into(),
                digest: String::new(),
                fetched_at: mtime,
            });
        }
        // Leftover temp files from a crashed download are garbage.
        if let Ok(tmps) = std::fs::read_dir(root.join("tmp")) {
            for t in tmps.flatten() {
                let _ = std::fs::remove_file(t.path());
            }
        }

        let low_water = max_bytes - max_bytes / 10;
        Ok(Self {
            root,
            max_bytes,
            low_water,
            state: Mutex::new(index),
        })
    }

    pub fn data_path(&self, key: &str) -> PathBuf {
        self.root.join("data").join(key_file(key))
    }

    pub fn temp_path(&self) -> PathBuf {
        self.root.join("tmp").join(format!(
            "dl-{}-{}",
            std::process::id(),
            now() ^ rand_suffix()
        ))
    }

    /// Look up a key, recording the access (LRU touch + hit count).
    pub fn lookup(&self, key: &str) -> Option<Entry> {
        let mut idx = self.state.lock().unwrap();
        let e = idx.entries.get_mut(key)?;
        e.last_access = now();
        e.access_count += 1;
        Some(e.clone())
    }

    /// Refresh a tag entry's fetched_at after a successful upstream
    /// revalidation (digest unchanged).
    pub fn refresh(&self, key: &str) {
        let mut idx = self.state.lock().unwrap();
        if let Some(e) = idx.entries.get_mut(key) {
            e.fetched_at = now();
        }
    }

    /// Move a fully-downloaded temp file into place and record it, evicting
    /// least-recently-used entries if the cap is exceeded.
    pub fn commit(&self, key: &str, temp: &Path, content_type: &str, digest: &str) -> Result<()> {
        let size = std::fs::metadata(temp).context("stat temp file")?.len();
        let dest = self.data_path(key);
        std::fs::rename(temp, &dest).context("move into cache")?;

        let mut idx = self.state.lock().unwrap();
        let t = now();
        idx.entries.insert(
            key.to_string(),
            Entry {
                size,
                last_access: t,
                access_count: 1,
                content_type: content_type.to_string(),
                digest: digest.to_string(),
                fetched_at: t,
            },
        );
        self.evict_locked(&mut idx);
        Ok(())
    }

    pub fn remove(&self, key: &str) {
        let mut idx = self.state.lock().unwrap();
        if idx.entries.remove(key).is_some() {
            let _ = std::fs::remove_file(self.data_path(key));
        }
    }

    fn evict_locked(&self, idx: &mut Index) {
        let mut total: u64 = idx.entries.values().map(|e| e.size).sum();
        if total <= self.max_bytes {
            return;
        }
        let mut keys: Vec<(String, u64)> = idx
            .entries
            .iter()
            .map(|(k, e)| (k.clone(), e.last_access))
            .collect();
        keys.sort_by_key(|(_, la)| *la);
        for (key, _) in keys {
            if total <= self.low_water {
                break;
            }
            if let Some(e) = idx.entries.remove(&key) {
                total -= e.size;
                let _ = std::fs::remove_file(self.data_path(&key));
                tracing::info!(key, size = e.size, "evicted LRU cache entry");
            }
        }
    }

    /// Atomically persist the index (temp file + rename).
    pub fn flush(&self) -> Result<()> {
        let bytes = {
            let idx = self.state.lock().unwrap();
            serde_json::to_vec(&*idx)?
        };
        let tmp = self.root.join("index.json.tmp");
        std::fs::write(&tmp, bytes).context("write index tmp")?;
        std::fs::rename(&tmp, self.root.join("index.json")).context("rename index")?;
        Ok(())
    }

    /// Snapshot for the /stats endpoint.
    pub fn stats(&self) -> serde_json::Value {
        let idx = self.state.lock().unwrap();
        let total: u64 = idx.entries.values().map(|e| e.size).sum();
        let mut top: Vec<(&String, &Entry)> = idx.entries.iter().collect();
        top.sort_by_key(|(_, e)| std::cmp::Reverse(e.size));
        serde_json::json!({
            "entries": idx.entries.len(),
            "total_bytes": total,
            "max_bytes": self.max_bytes,
            "top_by_size": top.iter().take(25).map(|(k, e)| serde_json::json!({
                "key": k, "size": e.size, "hits": e.access_count,
                "last_access": e.last_access,
            })).collect::<Vec<_>>(),
        })
    }
}

fn rand_suffix() -> u64 {
    // Cheap uniqueness for temp names; no crypto needed.
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_entry(c: &Cache, key: &str, bytes: &[u8]) {
        let tmp = c.temp_path();
        std::fs::write(&tmp, bytes).unwrap();
        c.commit(key, &tmp, "application/octet-stream", "sha256:x")
            .unwrap();
    }

    #[test]
    fn evicts_lru_and_keeps_recently_read() {
        let dir = tempfile::tempdir().unwrap();
        let c = Cache::load(dir.path(), 250).unwrap();
        write_entry(&c, "a", &[0u8; 100]);
        write_entry(&c, "b", &[0u8; 100]);
        // Order last_access decisively: a older than b.
        {
            let mut idx = c.state.lock().unwrap();
            idx.entries.get_mut("a").unwrap().last_access = 10;
            idx.entries.get_mut("b").unwrap().last_access = 20;
        }
        // Touch a so b becomes the LRU entry.
        c.lookup("a").unwrap();
        write_entry(&c, "c", &[0u8; 100]); // 300 > 250 → evict b
        assert!(c.lookup("b").is_none());
        assert!(c.lookup("a").is_some());
        assert!(c.lookup("c").is_some());
        assert!(!c.data_path("b").exists());
        let total: u64 = c
            .state
            .lock()
            .unwrap()
            .entries
            .values()
            .map(|e| e.size)
            .sum();
        assert!(total <= 250);
    }

    #[test]
    fn index_persists_access_counts_across_reload() {
        let dir = tempfile::tempdir().unwrap();
        {
            let c = Cache::load(dir.path(), 1000).unwrap();
            write_entry(&c, "a", b"hello");
            c.lookup("a").unwrap();
            c.lookup("a").unwrap();
            c.flush().unwrap();
        }
        let c = Cache::load(dir.path(), 1000).unwrap();
        let e = c.lookup("a").unwrap();
        // 1 from commit + 2 lookups persisted + this lookup.
        assert_eq!(e.access_count, 4);
    }

    #[test]
    fn reconciles_untracked_files_and_stale_rows() {
        let dir = tempfile::tempdir().unwrap();
        {
            let c = Cache::load(dir.path(), 1000).unwrap();
            write_entry(&c, "gone", b"bye");
            write_entry(&c, "kept", b"hi");
            c.flush().unwrap();
        }
        // Delete one file behind the index's back; add an untracked one.
        std::fs::remove_file(dir.path().join("data").join(key_file("gone"))).unwrap();
        std::fs::write(dir.path().join("data").join(key_file("new")), b"found").unwrap();

        let c = Cache::load(dir.path(), 1000).unwrap();
        assert!(c.lookup("gone").is_none());
        assert!(c.lookup("kept").is_some());
        let e = c.lookup("new").unwrap();
        assert_eq!(e.size, 5);
    }
}
