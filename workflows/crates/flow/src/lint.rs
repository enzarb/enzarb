//! What `enzarb-lint` runs over a directory: static checks of every
//! definition file found, and scenario tests. Filesystem access lives in
//! [`FsLoader`] and the directory walk; validation itself is the pure
//! resolver.

use crate::diag::Diagnostic;
use crate::format::{BindingsFile, BoardFile, CheckFile, RuntimeFile, Trigger};
use crate::resolve::{self, ENGINE_EVENTS, Entry, Loaded, Loader, Options};
use crate::scenario::{self, Scenario};
use crate::template;
use crate::yaml::SourceFile;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Ref used for local files.
pub const LOCAL_REF: &str = "local";

/// Serves repositories from local directories (sha `local`).
#[derive(Debug, Clone, Default)]
pub struct FsLoader {
    pub roots: BTreeMap<String, PathBuf>,
}

impl Loader for FsLoader {
    fn load(&self, repo: &str, _rref: &str, path: &str) -> Result<Loaded, String> {
        let root = self.roots.get(repo).ok_or_else(|| format!("repository `{repo}` is not available locally (pass --library <alias>={repo}-checkout)"))?;
        if path.split('/').any(|s| s == "..") {
            return Err("`..` is not allowed in references".into());
        }
        let bytes = std::fs::read(root.join(path)).map_err(|e| e.to_string())?;
        Ok(Loaded {
            sha: LOCAL_REF.into(),
            bytes,
        })
    }
}

/// Lint configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub dir: PathBuf,
    /// Repository name of `dir` (`owner/repo`).
    pub repo: String,
    /// `alias or owner/repo` → local checkout.
    pub libraries: Vec<(String, PathBuf)>,
}

pub struct Workspace {
    cfg: Config,
    loader: FsLoader,
    files: Vec<String>,
    /// Every repository some file imports (allowed; it may still be unavailable locally).
    imported: BTreeSet<String>,
}

impl Workspace {
    pub fn new(cfg: Config) -> Result<Self, String> {
        let mut files = Vec::new();
        walk(&cfg.dir, &cfg.dir, &mut files).map_err(|e| format!("{}: {e}", cfg.dir.display()))?;
        files.sort();
        // Map aliases to repositories from every `imports:` found.
        let mut aliases: BTreeMap<String, String> = BTreeMap::new();
        for f in &files {
            if let Ok(text) = std::fs::read_to_string(cfg.dir.join(f))
                && let Ok(sf) = SourceFile::parse(f, &text)
                && let Some(imports) = sf.value.get("imports").and_then(|v| v.as_object())
            {
                for (alias, spec) in imports {
                    if let Some((repo, _)) = spec.as_str().and_then(|s| s.split_once('@')) {
                        aliases.insert(alias.clone(), repo.to_string());
                    }
                }
            }
        }
        let mut loader = FsLoader::default();
        loader.roots.insert(cfg.repo.clone(), cfg.dir.clone());
        for (k, p) in &cfg.libraries {
            let repo = if k.contains('/') {
                k.clone()
            } else {
                aliases
                    .get(k)
                    .cloned()
                    .ok_or_else(|| format!("--library {k}: no file imports an alias `{k}`"))?
            };
            loader.roots.insert(repo, p.clone());
        }
        Ok(Self {
            cfg,
            loader,
            files,
            imported: aliases.into_values().collect(),
        })
    }

    fn opts(&self) -> Options {
        Options {
            allow: self
                .loader
                .roots
                .keys()
                .chain(&self.imported)
                .cloned()
                .collect(),
        }
    }

    fn entry(&self, path: &str) -> Entry {
        Entry {
            repo: self.cfg.repo.clone(),
            rref: LOCAL_REF.into(),
            path: path.into(),
        }
    }

    /// Rewrites `owner/repo@ref:path` to the library's local path.
    fn localize(&self, mut d: Diagnostic) -> Diagnostic {
        for (repo, root) in &self.loader.roots {
            let prefix = format!("{repo}@{LOCAL_REF}:");
            if let Some(rest) = d.file.strip_prefix(&prefix).or_else(|| {
                d.file
                    .strip_prefix(&format!("{repo}@"))
                    .and_then(|r| r.split_once(':').map(|x| x.1))
            }) {
                d.file = root.join(rest).display().to_string();
                return d;
            }
        }
        if !d.file.starts_with('/') && !d.file.is_empty() && self.cfg.dir != Path::new(".") {
            d.file = self.cfg.dir.join(&d.file).display().to_string();
        }
        d
    }

    fn under(f: &str, dirs: &[&str]) -> bool {
        dirs.iter().any(|d| f.starts_with(&format!("{d}/")))
    }

    /// Static validation of everything in the directory.
    pub fn check(&self) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        let opts = self.opts();
        let mut push = |r: Vec<Diagnostic>| out.extend(r);
        let mut workflows: BTreeSet<String> = BTreeSet::new();
        for f in &self.files {
            if Self::under(f, &["workflows", ".enzarb/workflows"]) {
                workflows.insert(f.clone());
            }
        }
        for f in &self.files {
            let e = self.entry(f);
            if Self::under(f, &["phases", ".enzarb/phases"]) {
                push(diags(resolve::resolve_phase_file(&e, &self.loader, &opts)));
            } else if Self::under(f, &["steps", ".enzarb/steps"]) {
                push(diags(resolve::resolve_step_file(&e, &self.loader, &opts)));
            } else if Self::under(f, &["runtimes", ".enzarb/runtimes"]) {
                push(self.typed::<RuntimeFile>(f).err().into_iter().collect());
            } else if Self::under(f, &["checks", ".enzarb/checks"]) {
                push(self.check_check_file(f));
            } else if f == "board.yaml" || f == ".enzarb/board.yaml" || Self::under(f, &["boards"])
            {
                push(self.typed::<BoardFile>(f).err().into_iter().collect());
            } else if f == "bindings.yaml" || f == ".enzarb/workflows.yaml" {
                push(self.check_bindings(f, &mut workflows));
            }
        }
        for w in &workflows {
            push(diags(resolve::resolve_with(
                &self.entry(w),
                &self.loader,
                &opts,
                None,
            )));
        }
        let mut seen = BTreeSet::new();
        out.into_iter()
            .map(|d| self.localize(d))
            .filter(|d| seen.insert(d.to_string()))
            .collect()
    }

    fn typed<T: serde::de::DeserializeOwned>(
        &self,
        f: &str,
    ) -> Result<(T, SourceFile), Diagnostic> {
        let text = std::fs::read_to_string(self.cfg.dir.join(f))
            .map_err(|e| Diagnostic::error(f, (0, 0), e.to_string()))?;
        let sf = SourceFile::parse(f, &text)?;
        Ok((sf.typed::<T>()?, sf))
    }

    fn check_check_file(&self, f: &str) -> Vec<Diagnostic> {
        match self.typed::<CheckFile>(f) {
            Ok((c, _)) => c
                .problems()
                .into_iter()
                .map(|m| Diagnostic::error(f, (0, 0), m))
                .collect(),
            Err(d) => vec![d],
        }
    }

    fn check_bindings(&self, f: &str, workflows: &mut BTreeSet<String>) -> Vec<Diagnostic> {
        let (b, sf) = match self.typed::<BindingsFile>(f) {
            Ok(x) => x,
            Err(d) => return vec![d],
        };
        let mut out = Vec::new();
        let layer = resolve::Layer {
            name: f.into(),
            repo: self.cfg.repo.clone(),
            rref: LOCAL_REF.into(),
            file: b.clone(),
        };
        for (i, bind) in b.bindings.iter().enumerate() {
            let at = format!("bindings[{i}]");
            if let Some(Trigger::Event(t)) = &bind.on
                && !crate::events::CATALOG.contains(&t.as_str())
                && !ENGINE_EVENTS.contains(&t.as_str())
            {
                out.push(sf.warning(
                    &format!("{at}.on"),
                    format!(
                        "`{t}` is not a catalog event type; it must be a declared custom event"
                    ),
                ));
            }
            if let Some(c) = &bind.cond {
                match template::expr_vars(c) {
                    Err(e) => out.push(sf.error(
                        &format!("{at}.if"),
                        format!("invalid expression: {}", e.message),
                    )),
                    Ok(vars) => {
                        for v in routing_violations(&vars) {
                            out.push(sf.error(
                                &format!("{at}.if"),
                                format!(
                                    "bindings can only read `event` and `relations`, not `{v}`"
                                ),
                            ));
                        }
                    }
                }
            }
            for (k, v) in &bind.with {
                check_with_templates(&sf, &format!("{at}.with.{k}"), v, &mut out);
            }
            if (bind.use_.is_none() || bind.on.is_none()) && !bind.disabled {
                out.push(sf.error(&at, "a binding needs `on` and `use` (or `disabled: true`)"));
            }
        }
        match resolve::merge_bindings(&[layer]) {
            Err(e) => out.push(sf.error("bindings", e)),
            Ok(rb) => {
                for r in &rb {
                    if r.workflow.repo == self.cfg.repo {
                        workflows.insert(r.workflow.path.clone());
                    }
                    match resolve::resolve_with(&r.workflow, &self.loader, &self.opts(), None) {
                        Ok((res, _)) => {
                            let i = b
                                .bindings
                                .iter()
                                .position(|x| x.id == r.id && x.with == r.with)
                                .unwrap_or(0);
                            for k in r.with.keys() {
                                if !res.params.contains_key(k) {
                                    let declared: Vec<_> = res.params.keys().cloned().collect();
                                    out.push(sf.error(
                                        &format!("bindings[{i}].with.{k}"),
                                        format!(
                                            "workflow `{}` has no param `{k}` (declared: {})",
                                            r.workflow.path,
                                            if declared.is_empty() {
                                                "none".into()
                                            } else {
                                                declared.join(", ")
                                            }
                                        ),
                                    ));
                                }
                            }
                        }
                        Err(d) if r.workflow.repo != self.cfg.repo => out.extend(d.0),
                        Err(_) => {} // reported when the workflow itself is checked
                    }
                }
            }
        }
        out
    }

    /// Runs every scenario under `tests/` and `.enzarb/tests/`.
    pub fn test(&self) -> (usize, Vec<Diagnostic>) {
        let mut out = Vec::new();
        let mut n = 0;
        for f in self
            .files
            .iter()
            .filter(|f| Self::under(f, &["tests", ".enzarb/tests"]))
        {
            n += 1;
            let (sc, sf) = match self.typed::<Scenario>(f) {
                Ok(x) => x,
                Err(d) => {
                    out.push(d);
                    continue;
                }
            };
            let res = match resolve::resolve_with(
                &self.entry(&sc.workflow),
                &self.loader,
                &self.opts(),
                Some(&sc.params),
            ) {
                Ok((r, _)) => r,
                Err(d) => {
                    out.push(sf.error(
                        "workflow",
                        format!(
                            "workflow does not resolve ({} errors; run `enzarb-lint check`)",
                            d.0.len()
                        ),
                    ));
                    continue;
                }
            };
            for fl in scenario::run(&res, &sc) {
                out.push(sf.error(&fl.path, fl.message));
            }
        }
        (n, out.into_iter().map(|d| self.localize(d)).collect())
    }
}

/// Variables a binding may not read (anything but `event` and `relations`).
fn routing_violations(vars: &BTreeSet<String>) -> Vec<String> {
    vars.iter()
        .filter(|v| !matches!(v.split('.').next(), Some("event" | "relations")))
        .cloned()
        .collect()
}

fn check_with_templates(
    sf: &SourceFile,
    path: &str,
    v: &serde_json::Value,
    out: &mut Vec<Diagnostic>,
) {
    match v {
        serde_json::Value::String(s) => match template::template_vars(s) {
            Err(e) => out.push(sf.error(path, format!("invalid template: {}", e.message))),
            Ok(vars) => {
                for v in routing_violations(&vars) {
                    out.push(sf.error(
                        path,
                        format!("binding `with` can only read `event` and `relations`, not `{v}`"),
                    ));
                }
            }
        },
        serde_json::Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                check_with_templates(sf, &format!("{path}[{i}]"), x, out);
            }
        }
        serde_json::Value::Object(o) => {
            for (k, x) in o {
                check_with_templates(sf, &format!("{path}.{k}"), x, out);
            }
        }
        _ => {}
    }
}

fn diags<T>(r: Result<(T, Vec<Diagnostic>), crate::diag::Diagnostics>) -> Vec<Diagnostic> {
    match r {
        Ok((_, w)) => w,
        Err(d) => d.0,
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if e.file_type()?.is_dir() {
            if name == ".git" || name == "target" || name == "node_modules" {
                continue;
            }
            walk(root, &p, out)?;
        } else if name.ends_with(".yaml") || name.ends_with(".yml") {
            out.push(
                p.strip_prefix(root)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}
