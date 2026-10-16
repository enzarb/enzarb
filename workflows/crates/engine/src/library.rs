//! Definitions over the connector (ADR-0022 §4, §8): a [`flow::resolve::Loader`] reading files with
//! `resolve_ref` + `get_file`, restricted to an allow-list, cached by `(repo, sha, path)`, and the
//! three bindings layers (library `bindings.yaml`, Project `spec.workflows.bindings`, repo
//! `.enzarb/workflows.yaml`).
//!
//! A library commit is read whole on first use: one `list_files` call, then the files fetched
//! concurrently ([`FETCH_CONCURRENCY`]) into the cache, so resolving a new library sha does not
//! pay one round trip per file in sequence.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use enzarb_connector::{Connector, RepoKey};
use enzarb_flow::format::BindingsFile;
use enzarb_flow::resolve::{self, Entry, Layer, Loaded, Loader, Options, RBinding, Resolved};
use enzarb_flow::yaml::SourceFile;
use futures::StreamExt;

use crate::index::ProjectInfo;

/// Default resolution budget per call (`ENZARB_RESOLVE_BUDGET_SECS`). A new library sha starts
/// with a cold cache, so the budget covers listing and fetching it.
pub const BUDGET: Duration = Duration::from_secs(30);
/// Concurrent file fetches while a library commit is read into the cache.
pub const FETCH_CONCURRENCY: usize = 16;
/// Files above this size are not prefetched (read on demand instead).
const PREFETCH_MAX_BYTES: u64 = 512 * 1024;
/// Commits with more files than this are not prefetched.
const PREFETCH_MAX_FILES: usize = 5000;
/// How long a ref -> sha lookup is reused (definitions follow default-branch heads).
pub const REF_TTL: Duration = Duration::from_secs(30);
/// The repo layer file.
pub const REPO_BINDINGS: &str = ".enzarb/workflows.yaml";
/// The library layer file.
pub const LIBRARY_BINDINGS: &str = "bindings.yaml";

type FileKey = (String, String, String);

#[derive(Default)]
struct Cache {
    files: HashMap<FileKey, Option<Vec<u8>>>,
    refs: HashMap<(String, String), (String, Instant)>,
    /// `(repo, sha)` whose complete file list is cached: a path missing from `files` is absent.
    complete: HashSet<(String, String)>,
    /// `(repo, sha)` already listed (successfully or not), so a commit is listed once.
    listed: HashSet<(String, String)>,
}

/// Shared definition source. Cheap to clone.
#[derive(Clone)]
pub struct Library {
    connector: Arc<dyn Connector>,
    /// `owner/repo@ref` of the instance default library (`ENZARB_DEFAULT_LIBRARY`).
    pub default_library: String,
    /// Extra allow-listed library repos (`ENZARB_LIBRARY_REPOS`).
    pub library_repos: Vec<String>,
    /// Resolution budget per call ([`BUDGET`] by default).
    pub budget: Duration,
    /// Reuse of ref lookups ([`REF_TTL`] by default).
    pub ref_ttl: Duration,
    cache: Arc<Mutex<Cache>>,
}

fn is_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn repo_key(repo: &str) -> Result<RepoKey, String> {
    let (o, r) = repo
        .split_once('/')
        .ok_or_else(|| format!("bad repo `{repo}`"))?;
    Ok(RepoKey::new("github", o, r))
}

fn over_budget(budget: Duration) -> String {
    format!(
        "definition resolution exceeded its {} s budget (ENZARB_RESOLVE_BUDGET_SECS)",
        budget.as_secs()
    )
}

impl Library {
    pub fn new(
        connector: Arc<dyn Connector>,
        default_library: &str,
        library_repos: Vec<String>,
    ) -> Self {
        Self {
            connector,
            default_library: default_library.to_string(),
            library_repos,
            budget: BUDGET,
            ref_ttl: REF_TTL,
            cache: Arc::default(),
        }
    }

    /// Overrides the resolution budget.
    pub fn with_budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    /// Overrides how long ref lookups are reused (tests).
    pub fn with_ref_ttl(mut self, ttl: Duration) -> Self {
        self.ref_ttl = ttl;
        self
    }

    pub fn connector(&self) -> &Arc<dyn Connector> {
        &self.connector
    }

    /// The allow-list for a Project: its repos, the library repos and the Project's library.
    pub fn allow(&self, p: &ProjectInfo) -> Vec<String> {
        let mut v: Vec<String> = p.repos.iter().map(|(r, _)| r.clone()).collect();
        v.extend(self.library_repos.iter().map(|r| r.to_ascii_lowercase()));
        for lib in [Some(self.default_library.as_str()), p.library.as_deref()]
            .into_iter()
            .flatten()
        {
            if let Some((r, _)) = lib.split_once('@') {
                v.push(r.to_ascii_lowercase());
            }
        }
        v.sort();
        v.dedup();
        v
    }

    /// Library repos are prefetched whole per commit: the instance library, the extra library
    /// repos and `libraries` (`owner/repo@ref`, the Project's). Project repos are read file by
    /// file.
    fn is_library_repo(&self, repo: &str, libraries: &[String]) -> bool {
        let named = |l: &str| {
            l.split_once('@')
                .is_some_and(|(r, _)| r.eq_ignore_ascii_case(repo))
        };
        named(&self.default_library)
            || self
                .library_repos
                .iter()
                .any(|r| r.eq_ignore_ascii_case(repo))
            || libraries.iter().any(|l| named(l))
    }

    async fn sha_of(&self, repo: &str, rref: &str) -> Result<String, String> {
        if is_sha(rref) {
            return Ok(rref.to_string());
        }
        let k = (repo.to_string(), rref.to_string());
        if let Some((sha, at)) = self.cache.lock().expect("cache").refs.get(&k)
            && at.elapsed() < self.ref_ttl
        {
            return Ok(sha.clone());
        }
        let sha = self
            .connector
            .resolve_ref(&repo_key(repo)?, rref)
            .await
            .map_err(|e| format!("{repo}@{rref}: {e}"))?
            .ok_or_else(|| format!("{repo}@{rref}: no such ref"))?;
        self.cache
            .lock()
            .expect("cache")
            .refs
            .insert(k, (sha.clone(), Instant::now()));
        Ok(sha)
    }

    /// The cached contents of `k`: `Some(None)` when the file is known to be absent.
    fn cached(&self, k: &FileKey) -> Option<Option<Vec<u8>>> {
        let c = self.cache.lock().expect("cache");
        if let Some(v) = c.files.get(k) {
            return Some(v.clone());
        }
        c.complete
            .contains(&(k.0.clone(), k.1.clone()))
            .then_some(None)
    }

    /// Reads every file of `repo@sha` into the cache: one listing plus concurrent fetches.
    /// Without a listing (unsupported, truncated, failed) files are read one by one instead.
    async fn prefetch(&self, repo: &str, sha: &str) {
        let k = (repo.to_string(), sha.to_string());
        if !self.cache.lock().expect("cache").listed.insert(k.clone()) {
            return;
        }
        let Ok(key) = repo_key(repo) else { return };
        let files = match self.connector.list_files(&key, sha).await {
            Ok(Some(f)) if f.len() <= PREFETCH_MAX_FILES => f,
            Ok(_) => return,
            Err(e) => {
                tracing::debug!("listing {repo}@{sha}: {e}");
                return;
            }
        };
        let (small, large): (Vec<_>, Vec<_>) = files
            .into_iter()
            .partition(|(_, size)| *size <= PREFETCH_MAX_BYTES);
        let fetched: Vec<(String, Result<Option<String>, String>)> =
            futures::stream::iter(small.into_iter().map(|(path, _)| {
                let (connector, key) = (self.connector.clone(), key.clone());
                async move {
                    let r = connector
                        .get_file(&key, sha, &path)
                        .await
                        .map_err(|e| e.to_string());
                    (path, r)
                }
            }))
            .buffer_unordered(FETCH_CONCURRENCY)
            .collect()
            .await;
        let mut c = self.cache.lock().expect("cache");
        let mut complete = large.is_empty();
        for (path, r) in fetched {
            match r {
                Ok(v) => {
                    c.files.insert(
                        (repo.to_string(), sha.to_string(), path),
                        v.map(String::into_bytes),
                    );
                }
                Err(e) => {
                    tracing::debug!("prefetching {repo}@{sha}:{path}: {e}");
                    complete = false;
                }
            }
        }
        if complete {
            c.complete.insert(k);
        }
    }

    /// Reads a file at a ref, cached by sha. `None` when the file does not exist.
    pub async fn read(
        &self,
        repo: &str,
        rref: &str,
        path: &str,
    ) -> Result<(String, Option<Vec<u8>>), String> {
        self.read_in(repo, rref, path, &[]).await
    }

    /// [`read`](Self::read), where `libraries` (`owner/repo@ref`) name further library repos.
    async fn read_in(
        &self,
        repo: &str,
        rref: &str,
        path: &str,
        libraries: &[String],
    ) -> Result<(String, Option<Vec<u8>>), String> {
        let sha = self.sha_of(repo, rref).await?;
        let k = (repo.to_string(), sha.clone(), path.to_string());
        if self.cached(&k).is_none() && self.is_library_repo(repo, libraries) {
            self.prefetch(repo, &sha).await;
        }
        if let Some(v) = self.cached(&k) {
            return Ok((sha, v));
        }
        let text = self
            .connector
            .get_file(&repo_key(repo)?, &sha, path)
            .await
            .map_err(|e| format!("{repo}@{sha}:{path}: {e}"))?;
        let v = text.map(String::into_bytes);
        self.cache.lock().expect("cache").files.insert(k, v.clone());
        Ok((sha, v))
    }

    /// Runs `f` with a blocking [`Loader`] over this library (the interpreter's resolver is
    /// synchronous). Fails after the budget. `libraries` (`owner/repo@ref`) are read whole per
    /// commit, like the instance library.
    pub async fn with_loader<T: Send + 'static>(
        &self,
        allow: Vec<String>,
        libraries: Vec<String>,
        f: impl FnOnce(&dyn Loader, &Options) -> T + Send + 'static,
    ) -> Result<T, String> {
        let me = self.clone();
        let budget = self.budget;
        let handle = tokio::runtime::Handle::current();
        let job = tokio::task::spawn_blocking(move || {
            let loader = BlockingLoader {
                lib: me,
                handle,
                deadline: Instant::now() + budget,
                allow: allow.clone(),
                libraries,
            };
            f(&loader, &Options { allow })
        });
        tokio::time::timeout(budget + Duration::from_secs(1), job)
            .await
            .map_err(|_| over_budget(budget))?
            .map_err(|e| e.to_string())
    }

    /// Resolves a workflow entry for a Project.
    pub async fn resolve(
        &self,
        p: &ProjectInfo,
        entry: Entry,
        params: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<Resolved, String> {
        let libraries = p.library.iter().cloned().collect();
        self.with_loader(self.allow(p), libraries, move |l, o| {
            resolve::resolve_with(&entry, l, o, params.as_ref())
                .map(|(r, _warnings)| r)
                .map_err(|d| {
                    d.0.iter()
                        .filter(|x| x.is_error())
                        .map(|x| format!("{}:{}:{}: {}", x.file, x.line, x.col, x.message))
                        .collect::<Vec<_>>()
                        .join("; ")
                })
        })
        .await?
    }

    /// The merged bindings for `repo` of Project `p` (highest layer first).
    pub async fn bindings(&self, p: &ProjectInfo, repo: &str) -> Result<Vec<RBinding>, String> {
        let mut layers = Vec::new();
        let lib = p
            .library
            .clone()
            .unwrap_or_else(|| self.default_library.clone());
        let (lrepo, lref) = lib
            .split_once('@')
            .ok_or_else(|| format!("library `{lib}` must be owner/repo@ref"))?;
        let (_, bytes) = self
            .read_in(lrepo, lref, LIBRARY_BINDINGS, std::slice::from_ref(&lib))
            .await?;
        if let Some(b) = bytes {
            layers.push(layer("library", lrepo, lref, LIBRARY_BINDINGS, &b)?);
        }
        if let Some(text) = &p.bindings {
            // Unaliased `use` in the Project layer resolves against the library.
            layers.push(layer(
                "project",
                lrepo,
                lref,
                "spec.workflows.bindings",
                text.as_bytes(),
            )?);
        }
        let branch = p.default_branch(repo).unwrap_or("main").to_string();
        let (_, bytes) = self.read(repo, &branch, REPO_BINDINGS).await?;
        if let Some(b) = bytes {
            layers.push(layer("repo", repo, &branch, REPO_BINDINGS, &b)?);
        }
        resolve::merge_bindings(&layers)
    }
}

fn layer(name: &str, repo: &str, rref: &str, file: &str, bytes: &[u8]) -> Result<Layer, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("{file}: {e}"))?;
    let src = SourceFile::parse(file, text).map_err(|d| format!("{file}: {}", d.message))?;
    let f: BindingsFile = src.typed().map_err(|d| format!("{file}: {}", d.message))?;
    Ok(Layer {
        name: name.to_string(),
        repo: repo.to_string(),
        rref: rref.to_string(),
        file: f,
    })
}

struct BlockingLoader {
    lib: Library,
    handle: tokio::runtime::Handle,
    deadline: Instant,
    allow: Vec<String>,
    libraries: Vec<String>,
}

impl Loader for BlockingLoader {
    fn load(&self, repo: &str, rref: &str, path: &str) -> Result<Loaded, String> {
        let repo = repo.to_ascii_lowercase();
        if !self.allow.contains(&repo) {
            return Err(format!("`{repo}` is not allow-listed for definitions"));
        }
        if Instant::now() > self.deadline {
            return Err(over_budget(self.lib.budget));
        }
        let (sha, bytes) =
            self.handle
                .block_on(self.lib.read_in(&repo, rref, path, &self.libraries))?;
        let bytes = bytes.ok_or_else(|| format!("{repo}@{rref}:{path}: not found"))?;
        Ok(Loaded { sha, bytes })
    }
}
