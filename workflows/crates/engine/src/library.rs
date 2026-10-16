//! Definitions over the connector (ADR-0022 §4, §8): a [`flow::resolve::Loader`] reading files with
//! `resolve_ref` + `get_file`, restricted to an allow-list, cached by `(repo, sha, path)`, and the
//! three bindings layers (library `bindings.yaml`, Project `spec.workflows.bindings`, repo
//! `.enzarb/workflows.yaml`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use enzarb_connector::{Connector, RepoKey};
use enzarb_flow::format::BindingsFile;
use enzarb_flow::resolve::{self, Entry, Layer, Loaded, Loader, Options, RBinding, Resolved};
use enzarb_flow::yaml::SourceFile;

use crate::index::ProjectInfo;

/// Resolution budget per call (ADR-0022 Implementation notes: 5 s).
pub const BUDGET: Duration = Duration::from_secs(5);
/// How long a ref -> sha lookup is reused (definitions follow default-branch heads).
pub const REF_TTL: Duration = Duration::from_secs(30);
/// The repo layer file.
pub const REPO_BINDINGS: &str = ".enzarb/workflows.yaml";
/// The library layer file.
pub const LIBRARY_BINDINGS: &str = "bindings.yaml";

#[derive(Default)]
struct Cache {
    files: HashMap<(String, String, String), Option<Vec<u8>>>,
    refs: HashMap<(String, String), (String, Instant)>,
}

/// Shared definition source. Cheap to clone.
#[derive(Clone)]
pub struct Library {
    connector: Arc<dyn Connector>,
    /// `owner/repo@ref` of the instance default library (`ENZARB_DEFAULT_LIBRARY`).
    pub default_library: String,
    /// Extra allow-listed library repos (`ENZARB_LIBRARY_REPOS`).
    pub library_repos: Vec<String>,
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

impl Library {
    pub fn new(connector: Arc<dyn Connector>, default_library: &str, library_repos: Vec<String>) -> Self {
        Self {
            connector,
            default_library: default_library.to_string(),
            library_repos,
            cache: Arc::default(),
        }
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

    async fn sha_of(&self, repo: &str, rref: &str) -> Result<String, String> {
        if is_sha(rref) {
            return Ok(rref.to_string());
        }
        let k = (repo.to_string(), rref.to_string());
        if let Some((sha, at)) = self.cache.lock().expect("cache").refs.get(&k)
            && at.elapsed() < REF_TTL
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

    /// Reads a file at a ref, cached by sha. `None` when the file does not exist.
    pub async fn read(&self, repo: &str, rref: &str, path: &str) -> Result<(String, Option<Vec<u8>>), String> {
        let sha = self.sha_of(repo, rref).await?;
        let k = (repo.to_string(), sha.clone(), path.to_string());
        if let Some(v) = self.cache.lock().expect("cache").files.get(&k) {
            return Ok((sha, v.clone()));
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
    /// synchronous). Fails after [`BUDGET`].
    pub async fn with_loader<T: Send + 'static>(
        &self,
        allow: Vec<String>,
        f: impl FnOnce(&dyn Loader, &Options) -> T + Send + 'static,
    ) -> Result<T, String> {
        let me = self.clone();
        let handle = tokio::runtime::Handle::current();
        let job = tokio::task::spawn_blocking(move || {
            let loader = BlockingLoader {
                lib: me,
                handle,
                deadline: Instant::now() + BUDGET,
                allow: allow.clone(),
            };
            f(&loader, &Options { allow })
        });
        tokio::time::timeout(BUDGET + Duration::from_secs(1), job)
            .await
            .map_err(|_| "definition resolution exceeded its 5 s budget".to_string())?
            .map_err(|e| e.to_string())
    }

    /// Resolves a workflow entry for a Project.
    pub async fn resolve(&self, p: &ProjectInfo, entry: Entry) -> Result<Resolved, String> {
        self.with_loader(self.allow(p), move |l, o| {
            resolve::resolve(&entry, l, o)
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
        let lib = p.library.clone().unwrap_or_else(|| self.default_library.clone());
        let (lrepo, lref) = lib
            .split_once('@')
            .ok_or_else(|| format!("library `{lib}` must be owner/repo@ref"))?;
        let (_, bytes) = self.read(lrepo, lref, LIBRARY_BINDINGS).await?;
        if let Some(b) = bytes {
            layers.push(layer("library", lrepo, lref, LIBRARY_BINDINGS, &b)?);
        }
        if let Some(text) = &p.bindings {
            // Unaliased `use` in the Project layer resolves against the library.
            layers.push(layer("project", lrepo, lref, "spec.workflows.bindings", text.as_bytes())?);
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
}

impl Loader for BlockingLoader {
    fn load(&self, repo: &str, rref: &str, path: &str) -> Result<Loaded, String> {
        let repo = repo.to_ascii_lowercase();
        if !self.allow.contains(&repo) {
            return Err(format!("`{repo}` is not allow-listed for definitions"));
        }
        if Instant::now() > self.deadline {
            return Err("definition resolution exceeded its 5 s budget".into());
        }
        let (sha, bytes) = self.handle.block_on(self.lib.read(&repo, rref, path))?;
        let bytes = bytes.ok_or_else(|| format!("{repo}@{rref}:{path}: not found"))?;
        Ok(Loaded { sha, bytes })
    }
}
