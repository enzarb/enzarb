//! Resolution (ADR-0022 §2, §4, §8): load a workflow and everything it
//! references through an injected [`Loader`], bind params, wire exits and
//! validate. Pure apart from the loader; errors carry file + line/column.

use crate::diag::{Diagnostic, Diagnostics};
use crate::document::{FACT_KINDS, ROOTS};
use crate::events::CATALOG;
use crate::format::*;
use crate::template::{self, Scope};
use crate::yaml::{SourceFile, join, typed_at};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// Default `max_entries` of a phase.
pub const DEFAULT_MAX_ENTRIES: u32 = 5;
/// Default `max_visits` of a node.
pub const DEFAULT_MAX_VISITS: u32 = 10;
/// The implicit exit that flows to the next phase.
pub const EXIT_DONE: &str = "done";
/// The implicit exit taken when an entry or visit cap is exceeded.
pub const EXIT_LIMIT: &str = "limit_reached";
/// Event types the engine itself produces for waits.
pub const ENGINE_EVENTS: &[&str] = &["timer", "fact"];

/// Reads definition files. `rref` is a branch, tag or sha; the returned
/// sha is the commit the bytes came from.
pub trait Loader {
    fn load(&self, repo: &str, rref: &str, path: &str) -> Result<Loaded, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub sha: String,
    pub bytes: Vec<u8>,
}

/// Where a file came from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Origin {
    pub repo: String,
    #[serde(rename = "ref")]
    pub rref: String,
    pub path: String,
    pub sha: String,
}

/// A file to start resolution from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub repo: String,
    #[serde(rename = "ref")]
    pub rref: String,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Repositories definitions may be imported from (the Project's repos
    /// plus `workflows.libraryRepos`). The entry repo is always allowed.
    pub allow: Vec<String>,
}

// ---------------------------------------------------------------- resolved model

/// A fully resolved workflow: every reference loaded, params bound, exits wired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolved {
    pub entry: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub phases: Vec<RPhase>,
    #[serde(default)]
    pub events: BTreeMap<String, CustomEventDecl>,
    #[serde(default)]
    pub board: BoardFile,
    /// Every file used, for pinning and the viewer's origin badges.
    pub origins: Vec<Origin>,
    /// The workflow's bound params (`params` in handler conditions).
    #[serde(default)]
    pub params: Map<String, Value>,
    /// Task-wide handlers (`on:`), `set` params substituted.
    #[serde(default)]
    pub handlers: Vec<Handler>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RPhase {
    pub id: String,
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub params: Map<String, Value>,
    /// Declared exits, including `done` and `limit_reached`.
    pub exits: Vec<String>,
    /// Top-level phases only: where each exit goes.
    #[serde(default)]
    pub routes: BTreeMap<String, Route>,
    pub max_entries: u32,
    pub on_limit: String,
    pub start: String,
    pub nodes: BTreeMap<String, RNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    Phase(String),
    End(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RNode {
    pub kind: RKind,
    pub next: Vec<REdge>,
    pub max_visits: u32,
    #[serde(default)]
    pub on_limit: Option<String>,
    /// Assigned on entry (strings are expressions, other values literals).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub set: Map<String, Value>,
    /// Assigned when leaving, after the edge was chosen.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub set_on_exit: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RKind {
    Step(Box<RStep>),
    /// Params substituted: text fields are `TextField::Value`; a timer whose
    /// template reads only params is already `NumField::Int`.
    Wait(Box<Wait>),
    /// Params substituted in `args`.
    Action(Box<Action>),
    End(String),
    Phase(Box<RPhase>),
    /// Pure branching (a `decide` node, or a node with only `set`).
    Decide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct REdge {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cond: Option<String>,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RStep {
    /// The `use` reference, or `inline`.
    pub name: String,
    pub origin: Origin,
    pub params: Map<String, Value>,
    pub exec: Exec,
    pub inputs: BTreeMap<String, Input>,
    pub outputs: BTreeMap<String, Output>,
    pub result: Option<ResultSpec>,
    pub workspaces: Vec<Workspace>,
    pub github: BTreeMap<String, Access>,
    /// Normalized (`SecretDecl::normalized`).
    pub secrets: Vec<Secret>,
    pub priority: Option<i32>,
    pub concurrency: Option<Concurrency>,
    pub retry: Option<Retry>,
    pub steer: Vec<Matcher>,
    pub stop: Vec<Matcher>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Exec {
    Agent {
        runtime: Box<RRuntime>,
        model: Option<String>,
        prompt: String,
    },
    Command {
        run: String,
        tools: Tools,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RRuntime {
    pub name: String,
    pub origin: Origin,
    pub file: RuntimeFile,
}

impl Resolved {
    /// The phase at `path` (top-level id, then nested phase node ids).
    pub fn phase(&self, path: &[String]) -> Option<&RPhase> {
        let (first, rest) = path.split_first()?;
        let mut p = self.phases.iter().find(|p| &p.id == first)?;
        for id in rest {
            match &p.nodes.get(id)?.kind {
                RKind::Phase(inner) => p = inner,
                _ => return None,
            }
        }
        Some(p)
    }

    pub fn phase_index(&self, id: &str) -> Option<usize> {
        self.phases.iter().position(|p| p.id == id)
    }
}

/// The default stop matcher of a step: a `stop` command from a writer.
pub fn default_stop() -> Vec<Matcher> {
    vec![Matcher::Full(MatcherSpec {
        event: "comment".into(),
        from: Some(Sender::Writer),
        cond: Some("event.command == 'stop'".into()),
    })]
}

// ---------------------------------------------------------------- entry points

/// Resolves a workflow file without params (a workflow with required params
/// is an error; see [`resolve_with`]).
pub fn resolve(
    entry: &Entry,
    loader: &dyn Loader,
    opts: &Options,
) -> Result<(Resolved, Vec<Diagnostic>), Diagnostics> {
    resolve_with(entry, loader, opts, Some(&Map::new()))
}

/// Resolves a workflow file with the workflow params a binding's `with`
/// produced. `None` uses placeholders for missing required params (lint).
pub fn resolve_with(
    entry: &Entry,
    loader: &dyn Loader,
    opts: &Options,
    params: Option<&Map<String, Value>>,
) -> Result<(Resolved, Vec<Diagnostic>), Diagnostics> {
    let mut r = Resolver::new(loader, opts, &entry.repo);
    let out = r.workflow(entry, params);
    r.finish(out)
}

/// Resolves a single phase file with placeholder params, for linting
/// library phases no workflow uses.
pub fn resolve_phase_file(
    entry: &Entry,
    loader: &dyn Loader,
    opts: &Options,
) -> Result<(RPhase, Vec<Diagnostic>), Diagnostics> {
    let mut r = Resolver::new(loader, opts, &entry.repo);
    r.placeholders = true;
    let ctx = Ctx {
        repo: entry.repo.clone(),
        rref: entry.rref.clone(),
        imports: Imports::new(),
    };
    let id = entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or("phase")
        .trim_end_matches(".yaml")
        .to_string();
    let out = r.phase(&ctx, &entry.path, &id, None, &Map::new(), &Map::new(), None);
    r.finish(out)
}

/// Resolves a single step file with placeholder params, for linting.
pub fn resolve_step_file(
    entry: &Entry,
    loader: &dyn Loader,
    opts: &Options,
) -> Result<(RStep, Vec<Diagnostic>), Diagnostics> {
    let mut r = Resolver::new(loader, opts, &entry.repo);
    r.placeholders = true;
    let ctx = Ctx {
        repo: entry.repo.clone(),
        rref: entry.rref.clone(),
        imports: Imports::new(),
    };
    let path = with_ext(&entry.path);
    let out = (|| {
        let k = r.load(&entry.repo, &entry.rref, &path, true, None)?;
        let sf: StepFile = r.typed(&k)?;
        let mut sctx = ctx.clone();
        sctx.imports.extend(sf.imports.clone());
        r.step(
            &sctx,
            &k,
            "",
            &entry.path,
            &sf,
            &Map::new(),
            &sctx,
            None,
            &Map::new(),
        )
    })();
    r.finish(out)
}

// ---------------------------------------------------------------- resolver

#[derive(Debug, Clone)]
struct Ctx {
    repo: String,
    rref: String,
    imports: Imports,
}

/// Where a reference is written: file key + value path inside it.
#[derive(Clone)]
struct At<'a> {
    file: &'a str,
    path: String,
}

struct Resolver<'l> {
    loader: &'l dyn Loader,
    allow: BTreeSet<String>,
    entry_repo: String,
    files: BTreeMap<String, (Origin, Option<SourceFile>, String)>,
    diags: Vec<Diagnostic>,
    stack: Vec<String>,
    placeholders: bool,
    custom_events: Option<BTreeSet<String>>,
    board: BoardFile,
    /// Text loaded from a file through a prompt param → that file's key, so
    /// template errors point into the file.
    text_origin: BTreeMap<String, String>,
    /// The workflow's `commands:` defaults (names rendered), inherited by every wait.
    default_commands: Vec<CommandDecl>,
    /// Phase ids of the workflow being resolved (targets of `goto:`).
    phase_ids: Vec<String>,
    /// A whole workflow is being resolved: waits are held to the common command set.
    command_lint: bool,
    /// The workflow declares commands somewhere: missing common commands are errors.
    commands_declared: bool,
}

fn key(repo: &str, rref: &str, path: &str) -> String {
    format!("{repo}@{rref}:{path}")
}

fn with_ext(path: &str) -> String {
    let p = path.trim_start_matches("./");
    let last = p.rsplit('/').next().unwrap_or(p);
    if last.contains('.') {
        p.to_string()
    } else {
        format!("{p}.yaml")
    }
}

impl<'l> Resolver<'l> {
    fn new(loader: &'l dyn Loader, opts: &Options, entry_repo: &str) -> Self {
        let mut allow: BTreeSet<String> = opts.allow.iter().cloned().collect();
        allow.insert(entry_repo.to_string());
        Self {
            loader,
            allow,
            entry_repo: entry_repo.into(),
            files: BTreeMap::new(),
            diags: Vec::new(),
            stack: Vec::new(),
            placeholders: false,
            custom_events: None,
            board: BoardFile::default(),
            text_origin: BTreeMap::new(),
            default_commands: Vec::new(),
            phase_ids: Vec::new(),
            command_lint: false,
            commands_declared: false,
        }
    }

    fn finish<T>(self, out: Option<T>) -> Result<(T, Vec<Diagnostic>), Diagnostics> {
        let has_errors = self.diags.iter().any(Diagnostic::is_error);
        match out {
            Some(v) if !has_errors => Ok((v, self.diags)),
            _ => Err(Diagnostics(self.diags)),
        }
    }

    fn display(&self, repo: &str, rref: &str, path: &str) -> String {
        if repo == self.entry_repo {
            path.to_string()
        } else {
            format!("{repo}@{rref}:{path}")
        }
    }

    fn err(&mut self, at: &At, msg: impl Into<String>) {
        let d = match self.files.get(at.file) {
            Some((_, Some(f), _)) => f.error(&at.path, msg),
            Some((o, None, _)) => {
                Diagnostic::error(self.display(&o.repo, &o.rref, &o.path), (0, 0), msg)
            }
            None => Diagnostic::error(at.file, (0, 0), msg),
        };
        self.diags.push(d);
    }

    fn warn(&mut self, at: &At, msg: impl Into<String>) {
        if let Some((_, Some(f), _)) = self.files.get(at.file) {
            let d = f.warning(&at.path, msg);
            self.diags.push(d);
        }
    }

    fn origin(&self, file: &str) -> Origin {
        self.files
            .get(file)
            .map(|f| f.0.clone())
            .expect("file loaded")
    }

    /// Maps a reference to `(repo, ref, path)`: an import alias prefix, or a
    /// path in the referencing file's repository.
    fn locate(
        &mut self,
        ctx: &Ctx,
        r: &str,
        at: &At,
        yaml: bool,
    ) -> Option<(String, String, String)> {
        let r = r.trim();
        if r.is_empty() {
            self.err(at, "empty reference");
            return None;
        }
        let (repo, rref, path) = match r.split_once('/') {
            Some((alias, rest)) if ctx.imports.contains_key(alias) => {
                let spec = &ctx.imports[alias];
                let Some((repo, rref)) = spec.split_once('@') else {
                    self.err(
                        at,
                        format!("import `{alias}: {spec}` must be `owner/repo@ref`"),
                    );
                    return None;
                };
                (repo.to_string(), rref.to_string(), rest.to_string())
            }
            _ => (ctx.repo.clone(), ctx.rref.clone(), r.to_string()),
        };
        if !self.allow.contains(&repo) {
            self.err(
                at,
                format!(
                    "`{repo}` is not an allowed definition repository (allowed: {})",
                    self.allow.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            );
            return None;
        }
        let path = if yaml {
            with_ext(&path)
        } else {
            path.trim_start_matches("./").to_string()
        };
        Some((repo, rref, path))
    }

    /// Loads a file (cached); returns its key.
    fn load(
        &mut self,
        repo: &str,
        rref: &str,
        path: &str,
        yaml: bool,
        at: Option<&At>,
    ) -> Option<String> {
        let k = key(repo, rref, path);
        if self.files.contains_key(&k) {
            return Some(k);
        }
        let loaded = match self.loader.load(repo, rref, path) {
            Ok(l) => l,
            Err(e) => {
                let msg = format!("cannot load `{}`: {e}", self.display(repo, rref, path));
                match at {
                    Some(at) => self.err(at, msg),
                    None => self.diags.push(Diagnostic::error(
                        self.display(repo, rref, path),
                        (0, 0),
                        msg,
                    )),
                }
                return None;
            }
        };
        let origin = Origin {
            repo: repo.into(),
            rref: rref.into(),
            path: path.into(),
            sha: loaded.sha,
        };
        let name = self.display(repo, rref, path);
        let Ok(text) = String::from_utf8(loaded.bytes) else {
            self.diags
                .push(Diagnostic::error(name, (0, 0), "file is not UTF-8"));
            return None;
        };
        let parsed = if yaml {
            match SourceFile::parse(&name, &text) {
                Ok(f) => Some(f),
                Err(d) => {
                    self.diags.push(d);
                    return None;
                }
            }
        } else {
            None
        };
        self.files.insert(k.clone(), (origin, parsed, text));
        Some(k)
    }

    fn typed<T: serde::de::DeserializeOwned>(&mut self, file: &str) -> Option<T> {
        let f = self.files.get(file)?.1.as_ref()?;
        match f.typed::<T>() {
            Ok(v) => Some(v),
            Err(d) => {
                self.diags.push(d);
                None
            }
        }
    }

    fn source_value(&self, file: &str, path: &str) -> Value {
        let Some((_, Some(f), _)) = self.files.get(file) else {
            return Value::Null;
        };
        let mut cur = &f.value;
        for seg in split_path(path) {
            cur = match (cur, seg) {
                (Value::Object(m), Seg::Key(k)) => m.get(k).unwrap_or(&Value::Null),
                (Value::Array(a), Seg::Index(i)) => a.get(i).unwrap_or(&Value::Null),
                _ => &Value::Null,
            };
        }
        cur.clone()
    }

    fn check_version(&mut self, file: &str, v: Option<u32>) {
        if let Some(v) = v
            && v != 1
        {
            self.err(
                &At {
                    file,
                    path: "version".into(),
                },
                format!("unsupported version {v} (expected 1)"),
            );
        }
    }

    // ------------------------------------------------------------ workflow

    fn workflow(&mut self, entry: &Entry, given: Option<&Map<String, Value>>) -> Option<Resolved> {
        let path = with_ext(&entry.path);
        let file = self.load(&entry.repo, &entry.rref, &path, true, None)?;
        let wf: WorkflowFile = self.typed(&file)?;
        self.check_version(&file, Some(wf.version));
        let ctx = Ctx {
            repo: entry.repo.clone(),
            rref: entry.rref.clone(),
            imports: wf.imports.clone(),
        };
        self.check_imports(&file, &wf.imports);
        let was = self.placeholders;
        self.placeholders = given.is_none();
        let empty = Map::new();
        let wparams = self.bind(
            &wf.params,
            &ctx,
            &file,
            given.unwrap_or(&empty),
            &ctx,
            None,
            &Map::new(),
        );
        self.placeholders = was;
        self.custom_events = Some(wf.events.keys().cloned().collect());
        for (name, decl) in &wf.events {
            let at = At {
                file: &file,
                path: format!("events.{name}"),
            };
            if CATALOG.contains(&name.as_str()) || ENGINE_EVENTS.contains(&name.as_str()) {
                self.err(
                    &at,
                    format!("custom event `{name}` shadows a built-in event type"),
                );
            }
            if !decl.schema.is_object() && !decl.schema.is_boolean() {
                self.err(
                    &At {
                        file: &file,
                        path: format!("events.{name}.schema"),
                    },
                    "schema must be a JSON Schema object",
                );
            }
        }
        if let Some(b) = &wf.board {
            let at = At {
                file: &file,
                path: "board".into(),
            };
            if let Some((repo, rref, p)) = self.locate(&ctx, b, &at, true)
                && let Some(bf) = self.load(&repo, &rref, &p, true, Some(&at))
                && let Some(board) = self.typed::<BoardFile>(&bf)
            {
                self.check_version(&bf, board.version);
                self.check_board(&bf, &board);
                self.board = board;
            }
        }

        // Phase ids first, so exits can target later phases.
        let mut ids: Vec<String> = Vec::new();
        for (i, p) in wf.phases.iter().enumerate() {
            let id = p.id.clone().unwrap_or_else(|| {
                p.use_
                    .rsplit('/')
                    .next()
                    .unwrap_or(&p.use_)
                    .trim_end_matches(".yaml")
                    .to_string()
            });
            if ids.contains(&id) {
                let at = At {
                    file: &file,
                    path: format!("phases[{i}]"),
                };
                self.err(
                    &at,
                    format!("duplicate phase id `{id}`; give one of them an `id`"),
                );
            }
            ids.push(id);
        }
        if wf.phases.is_empty() {
            self.err(
                &At {
                    file: &file,
                    path: "phases".into(),
                },
                "a workflow needs at least one phase",
            );
        }

        self.phase_ids = ids.clone();
        self.command_lint = true;
        self.commands_declared = !wf.commands.is_empty();
        self.default_commands = self.default_commands(&file, &wf.commands, &wparams);

        let mut phases = Vec::new();
        for (i, p) in wf.phases.iter().enumerate() {
            let at = At {
                file: &file,
                path: format!("phases[{i}].use"),
            };
            let with_at = format!("phases[{i}].with");
            let Some(mut phase) = self.phase(
                &ctx,
                &p.use_,
                &ids[i],
                Some(&at),
                &p.with,
                &wparams,
                Some((&file, &with_at)),
            ) else {
                continue;
            };
            phase.max_entries = match &p.max_entries {
                None => DEFAULT_MAX_ENTRIES,
                Some(n) => {
                    let at = At {
                        file: &file,
                        path: format!("phases[{i}].max_entries"),
                    };
                    match self.num(n, &wparams, &at, false, None) {
                        Some(NumField::Int(n)) => u32::try_from(n).unwrap_or(u32::MAX),
                        _ => DEFAULT_MAX_ENTRIES,
                    }
                }
            };
            if phase.max_entries == 0 {
                self.err(
                    &At {
                        file: &file,
                        path: format!("phases[{i}].max_entries"),
                    },
                    "max_entries must be at least 1",
                );
            }
            phase.on_limit = p.on_limit.clone().unwrap_or_else(|| EXIT_LIMIT.into());
            if !phase.exits.contains(&phase.on_limit) {
                let msg = format!(
                    "on_limit `{}` is not an exit of phase `{}` (exits: {})",
                    phase.on_limit,
                    phase.id,
                    phase.exits.join(", ")
                );
                self.err(
                    &At {
                        file: &file,
                        path: format!("phases[{i}].on_limit"),
                    },
                    msg,
                );
            }
            for (exit, to) in &p.exits {
                let eat = At {
                    file: &file,
                    path: format!("phases[{i}].exits.{exit}"),
                };
                if !phase.exits.contains(exit) {
                    let msg = format!(
                        "phase `{}` has no exit `{exit}` (exits: {})",
                        phase.id,
                        phase.exits.join(", ")
                    );
                    self.err(&eat, msg);
                }
                if let ExitTo::Phase(t) = to
                    && t != "next"
                    && !ids.contains(t)
                {
                    self.err(
                        &eat,
                        format!(
                            "exit target `{t}` is not a phase id (phases: {})",
                            ids.join(", ")
                        ),
                    );
                }
            }
            for exit in phase.exits.clone() {
                let route = match p.exits.get(&exit) {
                    Some(ExitTo::End { end }) => Route::End(end.clone()),
                    Some(ExitTo::Phase(t)) if t != "next" => Route::Phase(t.clone()),
                    Some(ExitTo::Phase(_)) => next_route(&ids, i, &exit),
                    None if exit == EXIT_DONE => next_route(&ids, i, &exit),
                    None => Route::End(exit.clone()),
                };
                phase.routes.insert(exit, route);
            }
            phases.push(phase);
        }

        let mut handlers = Vec::new();
        let hscope = Scope {
            names: ["event".to_string()].into(),
            params: wf.params.keys().cloned().collect(),
        };
        for (i, h) in wf.on.iter().enumerate() {
            let base = format!("on[{i}]");
            let at = At {
                file: &file,
                path: base.clone(),
            };
            if h.events.is_empty() {
                self.err(&at, "a handler needs at least one event");
            }
            self.check_matchers(&file, &format!("{base}.events"), &h.events, &hscope);
            if let Some(c) = &h.cond {
                self.check_expr(&file, &format!("{base}.if"), c, &hscope);
            }
            let set = self.sets(&file, &format!("{base}.set"), &h.set, &wparams, &hscope);
            match &h.do_ {
                None if h.set.is_empty() => self.err(&at, "a handler needs `do` or `set`"),
                Some(HandlerDo::Goto(p)) if !ids.contains(p) => self.err(
                    &At {
                        file: &file,
                        path: format!("{base}.do"),
                    },
                    format!("`goto:{p}`: not a phase id (phases: {})", ids.join(", ")),
                ),
                _ => {}
            }
            let mut h = h.clone();
            h.set = set;
            handlers.push(h);
        }

        let origins: Vec<Origin> = self.files.values().map(|f| f.0.clone()).collect();
        Some(Resolved {
            entry: self.origin(&file),
            description: wf.description,
            phases,
            events: wf.events,
            board: self.board.clone(),
            origins,
            params: wparams,
            handlers,
        })
    }

    // ------------------------------------------------------------ templated literals

    /// Resolves an integer field against `params`. A template that reads
    /// only params is rendered now; one that reads more is kept for run time
    /// when `runtime` gives its scope, else it is an error.
    fn num(
        &mut self,
        f: &NumField,
        params: &Map<String, Value>,
        at: &At,
        durations: bool,
        runtime: Option<&Scope>,
    ) -> Option<NumField> {
        let (val, derived) = match f {
            NumField::Int(n) => return Some(NumField::Int(*n)),
            NumField::Param { param } => match params.get(param) {
                Some(v) => (v.clone(), true),
                None => {
                    self.err(at, format!("`{{param: {param}}}`: no param `{param}`"));
                    return None;
                }
            },
            NumField::Value(s) if template::is_template(s) => {
                let vars = match template::template_vars(s) {
                    Ok(v) => v,
                    Err(e) => {
                        self.err(at, format!("invalid template: {}", e.message));
                        return None;
                    }
                };
                if vars.iter().any(|v| v.split('.').next() != Some("params")) {
                    match runtime {
                        Some(scope) => {
                            for m in template::check_vars(&vars, scope) {
                                self.err(at, m);
                            }
                            return Some(f.clone());
                        }
                        None => {
                            self.err(at, "this template may only read `params`");
                            return None;
                        }
                    }
                }
                let scope = Scope {
                    names: BTreeSet::new(),
                    params: params.keys().cloned().collect(),
                };
                let msgs = template::check_vars(&vars, &scope);
                if !msgs.is_empty() {
                    for m in msgs {
                        self.err(at, m);
                    }
                    return None;
                }
                match template::render(s, &json!({ "params": params })) {
                    Ok(r) => (Value::String(r), true),
                    Err(e) => {
                        self.err(at, format!("template error: {}", e.message));
                        return None;
                    }
                }
            }
            NumField::Value(s) => (Value::String(s.clone()), false),
        };
        let n = match &val {
            Value::Number(n) => n.as_u64(),
            Value::String(s) if durations => parse_duration(s),
            Value::String(s) => s.trim().parse::<u64>().ok(),
            _ => None,
        };
        match n {
            Some(n) => Some(NumField::Int(n)),
            None if derived && self.placeholders => Some(NumField::Int(1)),
            None => {
                let want = if durations {
                    "a non-negative integer or a duration (90s, 15m, 2h, 14d, 1w)"
                } else {
                    "a non-negative integer"
                };
                self.err(at, format!("must be {want}, got {val}"));
                None
            }
        }
    }

    /// Resolves a text field: `{param}` substituted, templates checked.
    fn text(
        &mut self,
        f: &TextField,
        params: &Map<String, Value>,
        file: &str,
        path: &str,
        scope: &Scope,
    ) -> TextField {
        let s = match f {
            TextField::Param { param } => match params.get(param) {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => {
                    self.err(
                        &At {
                            file,
                            path: path.into(),
                        },
                        format!("`{{param: {param}}}`: no param `{param}`"),
                    );
                    return f.clone();
                }
            },
            TextField::Value(s) => s.clone(),
        };
        if template::is_template(&s) {
            self.check_template(file, path, &s, scope, None);
        }
        TextField::Value(s)
    }

    /// Checks a `set` map: identifier keys, expressions; substitutes `{param}`.
    fn sets(
        &mut self,
        file: &str,
        base: &str,
        m: &Map<String, Value>,
        params: &Map<String, Value>,
        scope: &Scope,
    ) -> Map<String, Value> {
        let mut out = Map::new();
        for (k, v) in m {
            let path = format!("{base}.{k}");
            if !is_ident(k) {
                self.err(
                    &At {
                        file,
                        path: path.clone(),
                    },
                    format!("`{k}` is not a variable name (letters, digits, `_`)"),
                );
            }
            let v = match subst_params(v, params) {
                Ok(v) => v,
                Err(p) => {
                    self.err(
                        &At {
                            file,
                            path: path.clone(),
                        },
                        format!("`{{param: {p}}}`: no param `{p}`"),
                    );
                    continue;
                }
            };
            // Strings are expressions; a string param becomes a string literal.
            let v = match (v, is_param_ref(&m[k])) {
                (Value::String(s), true) => {
                    Value::String(serde_json::to_string(&s).unwrap_or_default())
                }
                (Value::String(e), false) => {
                    self.check_expr(file, &path, &e, scope);
                    Value::String(e)
                }
                (v, _) => v,
            };
            out.insert(k.clone(), v);
        }
        out
    }

    fn check_secrets(&mut self, file: &str, base: &str, decls: &[SecretDecl]) -> Vec<Secret> {
        let mut out: Vec<Secret> = Vec::new();
        for (i, d) in decls.iter().enumerate() {
            let at = At {
                file,
                path: format!("{base}[{i}]"),
            };
            let s = d.normalized();
            if s.name.trim().is_empty() {
                self.err(&at, "a secret needs a name");
            }
            if let SecretDecl::Full(f) = d
                && f.env.is_some()
                && f.file.is_some()
            {
                self.err(&at, "a secret has `env` or `file`, not both");
            }
            if let Some(e) = &s.env
                && !is_env_name(e)
            {
                self.err(
                    &at,
                    format!("`{e}` is not an environment variable name ([A-Za-z_][A-Za-z0-9_]*)"),
                );
            }
            if let Some(f) = &s.file
                && (f.is_empty() || f.starts_with('/') || f.split('/').any(|x| x == ".."))
            {
                self.err(
                    &at,
                    format!("secret file `{f}` must be a relative path without `..`"),
                );
            }
            if out.iter().any(|o| o.name == s.name && o.scope == s.scope) {
                self.err(&at, format!("duplicate secret `{}`", s.name));
            }
            out.push(s);
        }
        out
    }

    fn check_imports(&mut self, file: &str, imports: &Imports) {
        for (alias, spec) in imports {
            let at = At {
                file,
                path: format!("imports.{alias}"),
            };
            match spec.split_once('@') {
                Some((repo, rref)) if repo.contains('/') && !rref.is_empty() => {
                    if !self.allow.contains(repo) {
                        let msg = format!(
                            "`{repo}` is not an allowed definition repository (allowed: {})",
                            self.allow.iter().cloned().collect::<Vec<_>>().join(", ")
                        );
                        self.err(&at, msg);
                    }
                }
                _ => self.err(&at, format!("import `{spec}` must be `owner/repo@ref`")),
            }
        }
    }

    fn check_board(&mut self, file: &str, b: &BoardFile) {
        let mut seen = BTreeSet::new();
        for (i, c) in b.columns.iter().enumerate() {
            if !seen.insert(c.id.clone()) {
                self.err(
                    &At {
                        file,
                        path: format!("columns[{i}].id"),
                    },
                    format!("duplicate column `{}`", c.id),
                );
            }
        }
        if b.columns.is_empty() {
            return;
        }
        let d = &b.defaults;
        for (k, v) in [
            ("running", &d.running),
            ("waiting_human", &d.waiting_human),
            ("waiting_agent", &d.waiting_agent),
            ("waiting_external", &d.waiting_external),
            ("paused", &d.paused),
            ("ended", &d.ended),
        ] {
            if let Some(v) = v
                && !seen.contains(v)
            {
                self.err(
                    &At {
                        file,
                        path: format!("defaults.{k}"),
                    },
                    format!("unknown column `{v}`"),
                );
            }
        }
    }

    // ------------------------------------------------------------ params

    /// Binds `with` (written at `with_at`) against `decls` (declared in `decl_file`).
    #[allow(clippy::too_many_arguments)]
    fn bind(
        &mut self,
        decls: &BTreeMap<String, Param>,
        decl_ctx: &Ctx,
        decl_file: &str,
        with: &Map<String, Value>,
        with_ctx: &Ctx,
        with_at: Option<(&str, &str)>,
        enclosing: &Map<String, Value>,
    ) -> Map<String, Value> {
        let mut out = Map::new();
        let (wfile, wbase) = with_at.unwrap_or((decl_file, ""));
        for k in with.keys() {
            if !decls.contains_key(k) {
                let mut p = wbase.to_string();
                join(&mut p, k);
                let msg = format!(
                    "unknown param `{k}` (declared: {})",
                    if decls.is_empty() {
                        "none".to_string()
                    } else {
                        decls.keys().cloned().collect::<Vec<_>>().join(", ")
                    }
                );
                self.err(
                    &At {
                        file: wfile,
                        path: p,
                    },
                    msg,
                );
            }
        }
        for (name, decl) in decls {
            let mut wpath = wbase.to_string();
            join(&mut wpath, name);
            let (value, ctx, at) = match with.get(name) {
                Some(v) => (
                    v.clone(),
                    with_ctx,
                    At {
                        file: wfile,
                        path: wpath,
                    },
                ),
                None => match &decl.default {
                    Some(d) => (
                        d.clone(),
                        decl_ctx,
                        At {
                            file: decl_file,
                            path: format!("params.{name}.default"),
                        },
                    ),
                    None if self.placeholders => {
                        out.insert(name.clone(), placeholder(decl));
                        continue;
                    }
                    None => {
                        self.err(
                            &At {
                                file: wfile,
                                path: wbase.to_string(),
                            },
                            format!("missing required param `{name}`"),
                        );
                        continue;
                    }
                },
            };
            let value = match value
                .as_object()
                .and_then(|o| o.get("param"))
                .filter(|_| value.as_object().is_some_and(|o| o.len() == 1))
            {
                Some(Value::String(p)) => match enclosing.get(p) {
                    Some(v) => v.clone(),
                    None => {
                        self.err(
                            &at,
                            format!("`{{param: {p}}}`: the enclosing file has no param `{p}`"),
                        );
                        continue;
                    }
                },
                _ => value,
            };
            if let Some(v) = self.check_param(decl, value, ctx, &at, name) {
                out.insert(name.clone(), v);
            }
        }
        out
    }

    fn check_param(
        &mut self,
        decl: &Param,
        v: Value,
        ctx: &Ctx,
        at: &At,
        name: &str,
    ) -> Option<Value> {
        let bad = |want: &str| format!("param `{name}` must be {want}, got {v}");
        let ok = match decl.ty {
            ParamType::String | ParamType::Command => v.is_string(),
            ParamType::Integer => v.is_i64() || v.is_u64(),
            ParamType::Boolean => v.is_boolean(),
            ParamType::Enum => {
                if decl.values.is_empty() {
                    self.err(at, format!("enum param `{name}` declares no `values`"));
                    return None;
                }
                if !v
                    .as_str()
                    .is_some_and(|s| decl.values.iter().any(|x| x == s))
                {
                    self.err(
                        at,
                        format!(
                            "param `{name}` must be one of {}, got {v}",
                            decl.values.join(", ")
                        ),
                    );
                    return None;
                }
                true
            }
            ParamType::List => match v.as_array() {
                Some(items) => {
                    let item = Param {
                        ty: decl.items.unwrap_or(ParamType::String),
                        description: None,
                        default: None,
                        values: Vec::new(),
                        items: None,
                    };
                    let mut out = Vec::new();
                    for (i, it) in items.iter().enumerate() {
                        out.push(self.check_param(
                            &item,
                            it.clone(),
                            ctx,
                            at,
                            &format!("{name}[{i}]"),
                        )?);
                    }
                    return Some(Value::Array(out));
                }
                None => false,
            },
            ParamType::Prompt => {
                return match &v {
                    Value::String(_) => Some(v),
                    Value::Object(o)
                        if o.len() == 1 && o.get("file").is_some_and(Value::is_string) =>
                    {
                        let f = o["file"].as_str().unwrap_or_default().to_string();
                        self.load_text(ctx, &f, at)
                    }
                    _ => {
                        self.err(at, format!("param `{name}` must be a prompt (text or `{{file: path}}`), got {v}"));
                        None
                    }
                };
            }
        };
        if ok {
            Some(v)
        } else {
            let want = match decl.ty {
                ParamType::String => "a string",
                ParamType::Command => "a command string",
                ParamType::Integer => "an integer",
                ParamType::Boolean => "a boolean",
                _ => "a list",
            };
            self.err(at, bad(want));
            None
        }
    }

    fn load_text(&mut self, ctx: &Ctx, f: &str, at: &At) -> Option<Value> {
        let (repo, rref, p) = self.locate(ctx, f, at, false)?;
        let k = self.load(&repo, &rref, &p, false, Some(at))?;
        let text = self.files[&k].2.clone();
        self.text_origin.entry(text.clone()).or_insert(k);
        Some(Value::String(text))
    }

    fn source(
        &mut self,
        ctx: &Ctx,
        src: &Source,
        params: &Map<String, Value>,
        at: &At,
    ) -> Option<(String, Option<String>)> {
        match src {
            Source::Inline(s) => Some((s.clone(), None)),
            Source::File { file } => {
                let (repo, rref, p) = self.locate(ctx, file, at, false)?;
                let k = self.load(&repo, &rref, &p, false, Some(at))?;
                Some((self.files[&k].2.clone(), Some(k)))
            }
            Source::Param { param } => match params.get(param) {
                Some(Value::String(s)) => Some((s.clone(), self.text_origin.get(s).cloned())),
                Some(other) => {
                    self.err(at, format!("param `{param}` is not text: {other}"));
                    None
                }
                None => {
                    self.err(at, format!("no param `{param}`"));
                    None
                }
            },
        }
    }

    // ------------------------------------------------------------ phase

    #[allow(clippy::too_many_arguments)]
    fn phase(
        &mut self,
        ctx: &Ctx,
        r: &str,
        id: &str,
        at: Option<&At>,
        with: &Map<String, Value>,
        enclosing: &Map<String, Value>,
        with_at: Option<(&str, &str)>,
    ) -> Option<RPhase> {
        let fallback = At {
            file: "",
            path: String::new(),
        };
        let at_ref = at.unwrap_or(&fallback);
        let (repo, rref, path) = if at.is_some() {
            self.locate(ctx, r, at_ref, true)?
        } else {
            (ctx.repo.clone(), ctx.rref.clone(), with_ext(r))
        };
        let k = self.load(&repo, &rref, &path, true, at)?;
        if self.stack.contains(&k) {
            self.err(at_ref, format!("phase `{r}` includes itself"));
            return None;
        }
        let pf: PhaseFile = self.typed(&k)?;
        self.check_version(&k, Some(pf.version));
        self.check_imports(&k, &pf.imports);
        let mut imports = ctx.imports.clone();
        if repo != ctx.repo {
            imports.clear();
        }
        imports.extend(pf.imports.clone());
        let pctx = Ctx {
            repo,
            rref,
            imports,
        };
        let params = self.bind(&pf.params, &pctx, &k, with, ctx, with_at, enclosing);

        let mut exits = vec![EXIT_DONE.to_string(), EXIT_LIMIT.to_string()];
        for (i, e) in pf.exits.iter().enumerate() {
            if exits.contains(e) {
                self.err(
                    &At {
                        file: &k,
                        path: format!("exits[{i}]"),
                    },
                    format!("duplicate or implicit exit `{e}`"),
                );
            } else {
                exits.push(e.clone());
            }
        }
        self.stack.push(k.clone());
        let mut nodes = BTreeMap::new();
        let param_names: BTreeSet<String> = pf.params.keys().cloned().collect();
        for (nid, node) in &pf.nodes {
            if let Some(n) = self.node(
                &pctx,
                &k,
                nid,
                node,
                &params,
                &param_names,
                &exits,
                &pf.nodes,
            ) {
                nodes.insert(nid.clone(), n);
            }
        }
        self.stack.pop();
        if !pf.nodes.contains_key(&pf.start) {
            self.err(
                &At {
                    file: &k,
                    path: "start".into(),
                },
                format!("start node `{}` does not exist", pf.start),
            );
        } else {
            self.reachability(&k, &pf);
        }
        Some(RPhase {
            id: id.to_string(),
            origin: self.origin(&k),
            description: pf.description,
            params,
            exits,
            routes: BTreeMap::new(),
            max_entries: DEFAULT_MAX_ENTRIES,
            on_limit: EXIT_LIMIT.into(),
            start: pf.start,
            nodes,
        })
    }

    fn reachability(&mut self, file: &str, pf: &PhaseFile) {
        let mut seen = BTreeSet::new();
        let mut todo = vec![pf.start.clone()];
        while let Some(n) = todo.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            let Some(node) = pf.nodes.get(&n) else {
                continue;
            };
            todo.extend(targets(node));
        }
        for n in pf.nodes.keys() {
            if !seen.contains(n) {
                self.warn(
                    &At {
                        file,
                        path: format!("nodes.{n}"),
                    },
                    format!("node `{n}` is unreachable from `{}`", pf.start),
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        ctx: &Ctx,
        file: &str,
        id: &str,
        node: &Node,
        params: &Map<String, Value>,
        param_names: &BTreeSet<String>,
        exits: &[String],
        all: &BTreeMap<String, Node>,
    ) -> Option<RNode> {
        let base = format!("nodes.{id}");
        let at = At {
            file,
            path: base.clone(),
        };
        let kinds = [
            node.step.is_some(),
            node.wait.is_some(),
            node.action.is_some(),
            node.end.is_some(),
            node.phase.is_some(),
            node.decide.is_some(),
        ];
        let count = kinds.iter().filter(|k| **k).count();
        let set_only = count == 0 && !node.set.is_empty();
        if count != 1 && !set_only {
            self.err(
                &at,
                format!(
                    "node `{id}` must have exactly one of step, wait, action, end, phase, decide (or only `set`)"
                ),
            );
            return None;
        }
        let mut scope = Scope {
            names: ["last".to_string(), "exit".into()].into(),
            params: param_names.clone(),
        };
        let set = self.sets(file, &format!("{base}.set"), &node.set, params, &scope);
        let kind = if node.decide.is_some() || set_only {
            RKind::Decide
        } else if let Some(s) = &node.step {
            RKind::Step(Box::new(self.step_use(
                ctx,
                file,
                &format!("{base}.step"),
                s,
                params,
            )?))
        } else if let Some(w) = &node.wait {
            let w = self.check_wait(file, &format!("{base}.wait"), w, &scope, params);
            RKind::Wait(Box::new(w))
        } else if let Some(a) = &node.action {
            let aat = format!("{base}.action");
            let mut a = a.clone();
            match subst_params(&Value::Object(a.args.clone()), params) {
                Ok(Value::Object(o)) => a.args = o,
                Ok(_) => {}
                Err(p) => self.err(
                    &At {
                        file,
                        path: format!("{aat}.args"),
                    },
                    format!("`{{param: {p}}}`: no param `{p}`"),
                ),
            }
            self.check_value_templates(
                file,
                &format!("{aat}.args"),
                &Value::Object(a.args.clone()),
                &scope,
            );
            let rscope = scope.clone().with_names(["result"]);
            if let Some(rel) = &a.relate {
                match (&rel.targets, &rel.from) {
                    (Some(t), None) => {
                        self.check_expr(file, &format!("{aat}.relate.targets"), t, &scope)
                    }
                    (None, Some(f)) => {
                        self.check_expr(file, &format!("{aat}.relate.from"), f, &rscope)
                    }
                    _ => self.err(
                        &At {
                            file,
                            path: format!("{aat}.relate"),
                        },
                        "relate has exactly one of `targets` and `from`",
                    ),
                }
            }
            if let Some(n) = &a.as_
                && !is_ident(n)
            {
                self.err(
                    &At {
                        file,
                        path: format!("{aat}.as"),
                    },
                    format!("`{n}` is not an output name (letters, digits, `_`)"),
                );
            }
            match &a.subject {
                Some(SubjectLink::Flag(false)) | None => {}
                Some(SubjectLink::Flag(true)) => {}
                Some(SubjectLink::Spec(s)) => {
                    for (k, e) in [("number", &s.number), ("repo", &s.repo)] {
                        if let Some(e) = e {
                            self.check_expr(file, &format!("{aat}.subject.{k}"), e, &rscope);
                        }
                    }
                }
            }
            RKind::Action(Box::new(a))
        } else if let Some(e) = &node.end {
            if !exits.contains(&e.status) {
                self.err(
                    &At {
                        file,
                        path: format!("{base}.end.status"),
                    },
                    format!(
                        "`{}` is not an exit of this phase (exits: {})",
                        e.status,
                        exits.join(", ")
                    ),
                );
            }
            if node.next.is_some() {
                self.err(
                    &At {
                        file,
                        path: format!("{base}.next"),
                    },
                    "an end node has no `next`",
                );
            }
            RKind::End(e.status.clone())
        } else {
            let p = node.phase.as_ref()?;
            let pat = At {
                file,
                path: format!("{base}.phase.use"),
            };
            let with_at = format!("{base}.phase.with");
            let inner = self.phase(
                ctx,
                &p.use_,
                id,
                Some(&pat),
                &p.with,
                params,
                Some((file, &with_at)),
            )?;
            RKind::Phase(Box::new(inner))
        };
        if matches!(kind, RKind::Wait(_)) {
            scope.names.insert("event".into());
        }
        let set_on_exit = self.sets(
            file,
            &format!("{base}.set_on_exit"),
            &node.set_on_exit,
            params,
            &scope,
        );
        if matches!(kind, RKind::End(_)) && !node.set_on_exit.is_empty() {
            self.err(
                &At {
                    file,
                    path: format!("{base}.set_on_exit"),
                },
                "an end node has no `set_on_exit` (use `set`)",
            );
        }

        let mut next = Vec::new();
        match &node.next {
            None if !matches!(kind, RKind::End(_)) => {
                self.err(&at, format!("node `{id}` needs `next`"))
            }
            None => {}
            Some(Next::To(t)) => next.push(REdge {
                cond: None,
                to: t.clone(),
            }),
            Some(Next::Branches(bs)) => {
                for (i, b) in bs.iter().enumerate() {
                    let bat = At {
                        file,
                        path: format!("{base}.next[{i}]"),
                    };
                    match (&b.cond, &b.to, &b.else_) {
                        (Some(c), Some(t), None) => {
                            self.check_expr(file, &format!("{base}.next[{i}].if"), c, &scope);
                            next.push(REdge {
                                cond: Some(c.clone()),
                                to: t.clone(),
                            });
                        }
                        (None, None, Some(t)) => {
                            if i + 1 != bs.len() {
                                self.err(&bat, "`else` must be the last branch");
                            }
                            next.push(REdge {
                                cond: None,
                                to: t.clone(),
                            });
                        }
                        _ => self.err(
                            &bat,
                            "a branch is `{if: <expr>, to: <node>}` or `{else: <node>}`",
                        ),
                    }
                }
                if !matches!(kind, RKind::Wait(_)) && next.last().is_some_and(|e| e.cond.is_some())
                {
                    self.err(&At { file, path: format!("{base}.next") }, "branches must end with `else` (only a wait may keep waiting when no branch matches)");
                }
            }
        }
        for (i, e) in next.iter().enumerate() {
            if !all.contains_key(&e.to) {
                let p = if matches!(node.next, Some(Next::To(_))) {
                    format!("{base}.next")
                } else {
                    format!("{base}.next[{i}]")
                };
                self.err(&At { file, path: p }, format!("unknown node `{}`", e.to));
            }
        }
        if let Some(l) = &node.on_limit
            && !all.contains_key(l)
        {
            self.err(
                &At {
                    file,
                    path: format!("{base}.on_limit"),
                },
                format!("unknown node `{l}`"),
            );
        }
        let max_visits = match &node.max_visits {
            None => DEFAULT_MAX_VISITS,
            Some(n) => {
                let at = At {
                    file,
                    path: format!("{base}.max_visits"),
                };
                match self.num(n, params, &at, false, None) {
                    Some(NumField::Int(n)) => u32::try_from(n).unwrap_or(u32::MAX),
                    _ => DEFAULT_MAX_VISITS,
                }
            }
        };
        if max_visits == 0 {
            self.err(
                &At {
                    file,
                    path: format!("{base}.max_visits"),
                },
                "max_visits must be at least 1",
            );
        }
        let mut kind = kind;
        if let RKind::Wait(w) = &mut kind {
            self.wait_commands(
                file,
                &format!("{base}.wait"),
                w,
                &next,
                params,
                &scope,
                exits,
                all,
            );
        }
        Some(RNode {
            kind,
            next,
            max_visits,
            on_limit: node.on_limit.clone(),
            set,
            set_on_exit,
        })
    }

    /// Renders the workflow's `commands:` defaults.
    fn default_commands(
        &mut self,
        file: &str,
        decls: &[CommandDecl],
        params: &Map<String, Value>,
    ) -> Vec<CommandDecl> {
        let scope = Scope {
            names: BTreeSet::new(),
            params: params.keys().cloned().collect(),
        };
        let mut out: Vec<CommandDecl> = Vec::new();
        for (i, d) in decls.iter().enumerate() {
            let base = format!("commands[{i}]");
            let Some(d) = self.render_command(file, &base, d, params, &scope) else {
                continue;
            };
            if out.iter().any(|o| o.name == d.name) {
                self.err(
                    &At {
                        file,
                        path: format!("{base}.name"),
                    },
                    format!("command `{}` is declared twice", d.name.as_str()),
                );
                continue;
            }
            match d.to.as_deref() {
                None => self.err(
                    &At {
                        file,
                        path: format!("{base}.to"),
                    },
                    "a default command needs a route (`to`)",
                ),
                Some(t) => {
                    if let Some(p) = t.strip_prefix("goto:")
                        && !self.phase_ids.iter().any(|i| i == p.trim())
                    {
                        self.err(
                            &At {
                                file,
                                path: format!("{base}.to"),
                            },
                            format!(
                                "`goto:{p}`: not a phase id (phases: {})",
                                self.phase_ids.join(", ")
                            ),
                        );
                    }
                }
            }
            out.push(d);
        }
        out
    }

    /// Substitutes params in a command's text fields and checks its name.
    fn render_command(
        &mut self,
        file: &str,
        base: &str,
        d: &CommandDecl,
        params: &Map<String, Value>,
        scope: &Scope,
    ) -> Option<CommandDecl> {
        let text = |this: &mut Self, f: &TextField, what: &str| {
            this.text(f, params, file, &format!("{base}.{what}"), scope)
        };
        let name = text(self, &d.name, "name");
        let word = name.as_str().trim_start_matches('/').to_ascii_lowercase();
        if !crate::commands::is_command_word(&word) && !(self.placeholders && word.is_empty()) {
            self.err(
                &At {
                    file,
                    path: format!("{base}.name"),
                },
                "a command is a single word without the leading `/`",
            );
            return None;
        }
        Some(CommandDecl {
            name: TextField::Value(word),
            label: d.label.as_ref().map(|l| text(self, l, "label")),
            description: d.description.as_ref().map(|l| text(self, l, "description")),
            from: d.from,
            to: d.to.clone(),
        })
    }

    /// A wait's final command list: its own declarations, the workflow defaults it does not
    /// override (those that can route in this phase), then the commands its `events:` and
    /// `next:` conditions already match (listed, routed by the wait itself, `to` unset).
    /// Lint: every wait must end up accepting `retry` (`cancel` is engine-native).
    #[allow(clippy::too_many_arguments)]
    fn wait_commands(
        &mut self,
        file: &str,
        base: &str,
        w: &mut Wait,
        next: &[REdge],
        params: &Map<String, Value>,
        scope: &Scope,
        exits: &[String],
        all: &BTreeMap<String, Node>,
    ) {
        use crate::commands::{CommandRoute, literal_commands};
        // What the wait's own conditions handle.
        let mut handled: Vec<(String, bool)> = Vec::new();
        for m in &w.events {
            if let Matcher::Full(MatcherSpec {
                event,
                from,
                cond: Some(c),
            }) = m
                && event == "comment"
            {
                let writer = *from == Some(Sender::Writer) || w.from == Some(Sender::Writer);
                for (name, w2) in literal_commands(c, params) {
                    handled.push((name, writer || w2));
                }
            }
        }
        for e in next {
            if let Some(c) = &e.cond {
                handled.extend(literal_commands(c, params));
            }
        }
        let explicit = std::mem::take(&mut w.commands);
        let had_explicit = !explicit.is_empty();
        let mut out: Vec<CommandDecl> = Vec::new();
        for (i, d) in explicit.iter().enumerate() {
            let cbase = format!("{base}.commands[{i}]");
            let Some(mut d) = self.render_command(file, &cbase, d, params, scope) else {
                continue;
            };
            if out.iter().any(|o| o.name == d.name) {
                self.err(
                    &At {
                        file,
                        path: format!("{cbase}.name"),
                    },
                    format!("command `{}` is declared twice", d.name.as_str()),
                );
                continue;
            }
            match d.to.clone() {
                Some(to) => match CommandRoute::parse(&to, |n| all.contains_key(n)) {
                    Ok(CommandRoute::Exit(e)) if !exits.contains(&e) => self.err(
                        &At {
                            file,
                            path: format!("{cbase}.to"),
                        },
                        format!(
                            "`exit:{e}`: the phase has no such exit (exits: {})",
                            exits.join(", ")
                        ),
                    ),
                    Ok(CommandRoute::Goto(p)) if !self.phase_ids.contains(&p) => self.err(
                        &At {
                            file,
                            path: format!("{cbase}.to"),
                        },
                        format!(
                            "`goto:{p}`: not a phase id (phases: {})",
                            self.phase_ids.join(", ")
                        ),
                    ),
                    Ok(_) => {}
                    Err(m) => self.err(
                        &At {
                            file,
                            path: format!("{cbase}.to"),
                        },
                        m,
                    ),
                },
                None => {
                    if !handled.iter().any(|(n, _)| n == d.name.as_str()) {
                        self.err(
                            &At {
                                file,
                                path: format!("{cbase}.to"),
                            },
                            format!(
                                "command `{}` has no route: set `to`, or match it in `next:`",
                                d.name.as_str()
                            ),
                        );
                    }
                }
            }
            d.from.get_or_insert(Sender::Writer);
            out.push(d);
        }
        for d in &self.default_commands {
            if out.iter().any(|o| o.name == d.name) {
                continue;
            }
            let routable = match CommandRoute::parse(d.to.as_deref().unwrap_or_default(), |n| {
                all.contains_key(n)
            }) {
                Ok(CommandRoute::Exit(e)) => exits.contains(&e),
                Ok(_) => true,
                Err(_) => false,
            };
            if routable {
                let mut d = d.clone();
                d.from.get_or_insert(Sender::Writer);
                out.push(d);
            }
        }
        for (name, writer) in handled {
            match out.iter().position(|o| o.name.as_str() == name) {
                Some(_) => {}
                None => out.push(CommandDecl {
                    name: TextField::Value(name),
                    label: None,
                    description: None,
                    from: Some(if writer {
                        Sender::Writer
                    } else {
                        Sender::Anyone
                    }),
                    to: None,
                }),
            }
        }
        for a in &w.actions {
            let name = a
                .command
                .as_str()
                .trim_start_matches('/')
                .to_ascii_lowercase();
            if !name.is_empty() && !out.iter().any(|o| o.name.as_str() == name) {
                out.push(CommandDecl {
                    name: TextField::Value(name),
                    label: Some(a.label.clone()),
                    description: None,
                    from: Some(Sender::Writer),
                    to: None,
                });
            }
        }
        if self.command_lint && !self.placeholders {
            for common in COMMON_COMMANDS {
                if NATIVE_COMMANDS.contains(&common)
                    || out.iter().any(|o| o.name.as_str() == common)
                {
                    continue;
                }
                let msg = format!(
                    "wait does not accept `/{common}`: declare it in `commands:` or add a default to the workflow"
                );
                let at = At {
                    file,
                    path: base.to_string(),
                };
                if self.commands_declared || had_explicit {
                    self.err(&at, msg);
                } else {
                    self.warn(&at, msg);
                }
            }
        }
        w.commands = out;
    }

    fn check_matchers(&mut self, file: &str, base: &str, ms: &[Matcher], scope: &Scope) {
        for (i, m) in ms.iter().enumerate() {
            let p = format!("{base}[{i}]");
            let t = m.event();
            let known = CATALOG.contains(&t) && t != "custom" || ENGINE_EVENTS.contains(&t);
            match &self.custom_events {
                _ if known => {}
                Some(c) if c.contains(t) => {}
                Some(_) => self.err(
                    &At {
                        file,
                        path: p.clone(),
                    },
                    format!(
                        "unknown event type `{t}` (catalog: {}; or declare it under `events`)",
                        CATALOG.join(", ")
                    ),
                ),
                None => self.warn(
                    &At {
                        file,
                        path: p.clone(),
                    },
                    format!("event type `{t}` is not in the catalog; the workflow must declare it"),
                ),
            }
            if let Matcher::Full(MatcherSpec { cond: Some(c), .. }) = m {
                let scope = scope.clone().with_names(["event"]);
                self.check_expr(file, &format!("{p}.if"), c, &scope);
            }
        }
    }

    fn check_wait(
        &mut self,
        file: &str,
        base: &str,
        w: &Wait,
        scope: &Scope,
        params: &Map<String, Value>,
    ) -> Wait {
        let mut out = w.clone();
        out.reason = w
            .reason
            .as_ref()
            .map(|t| self.text(t, params, file, &format!("{base}.reason"), scope));
        out.text = w
            .text
            .as_ref()
            .map(|t| self.text(t, params, file, &format!("{base}.text"), scope));
        out.column = w
            .column
            .as_ref()
            .map(|t| self.text(t, params, file, &format!("{base}.column"), scope));
        for (i, a) in w.actions.iter().enumerate() {
            out.actions[i].label = self.text(
                &a.label,
                params,
                file,
                &format!("{base}.actions[{i}].label"),
                scope,
            );
            out.actions[i].command = self.text(
                &a.command,
                params,
                file,
                &format!("{base}.actions[{i}].command"),
                scope,
            );
        }
        if let Some(t) = &w.timer {
            let at = At {
                file,
                path: format!("{base}.timer.seconds"),
            };
            if let Some(n) = self.num(&t.seconds, params, &at, true, Some(scope)) {
                out.timer = Some(Timer { seconds: n });
            }
        }
        self.check_wait_rest(file, base, &out, scope);
        out
    }

    fn check_wait_rest(&mut self, file: &str, base: &str, w: &Wait, scope: &Scope) {
        if w.events.is_empty() {
            self.err(
                &At {
                    file,
                    path: format!("{base}.events"),
                },
                "a wait needs at least one event",
            );
        }
        self.check_matchers(file, &format!("{base}.events"), &w.events, scope);
        self.check_matchers(file, &format!("{base}.steer"), &w.steer, scope);
        if let Some(t) = &w.text
            && !template::is_template(t.as_str())
        {
            // Plain text is still a template (e.g. `{{` escapes); check it.
            self.check_template(file, &format!("{base}.text"), t.as_str(), scope, None);
        }
        if let Some(c) = w.column.as_ref().map(|c| c.as_str().to_string())
            && !template::is_template(&c)
            && !(self.placeholders && c.is_empty())
            && !self.board.columns.is_empty()
            && !self.board.columns.iter().any(|x| x.id == c)
        {
            let ids: Vec<_> = self.board.columns.iter().map(|c| c.id.clone()).collect();
            self.err(
                &At {
                    file,
                    path: format!("{base}.column"),
                },
                format!("unknown column `{c}` (board: {})", ids.join(", ")),
            );
        }
        for (i, f) in w.refresh.iter().enumerate() {
            if !FACT_KINDS.contains(&f.as_str()) {
                self.err(
                    &At {
                        file,
                        path: format!("{base}.refresh[{i}]"),
                    },
                    format!("unknown fact `{f}` (facts: {})", FACT_KINDS.join(", ")),
                );
            }
        }
        for (i, a) in w.actions.iter().enumerate() {
            let c = a.command.as_str();
            if !template::is_template(c)
                && !(self.placeholders && c.is_empty())
                && (c.is_empty() || c.starts_with('/') || c.contains(char::is_whitespace))
            {
                self.err(
                    &At {
                        file,
                        path: format!("{base}.actions[{i}].command"),
                    },
                    "command is a single word without the leading `/`",
                );
            }
        }
    }

    fn check_expr(&mut self, file: &str, path: &str, src: &str, scope: &Scope) {
        match template::expr_vars(src) {
            Err(e) => self.err(
                &At {
                    file,
                    path: path.into(),
                },
                format!("invalid expression: {}", e.message),
            ),
            Ok(vars) => {
                for m in template::check_vars(&vars, scope) {
                    self.err(
                        &At {
                            file,
                            path: path.into(),
                        },
                        m,
                    );
                }
            }
        }
    }

    /// Checks a template; `text_file` is set when the template is a separate file.
    fn check_template(
        &mut self,
        file: &str,
        path: &str,
        src: &str,
        scope: &Scope,
        text_file: Option<&str>,
    ) {
        let report = |s: &mut Self, line: Option<usize>, msg: String| match text_file
            .and_then(|k| s.files.get(k))
        {
            Some((o, _, _)) => {
                let name = s.display(&o.repo, &o.rref, &o.path);
                s.diags
                    .push(Diagnostic::error(name, (line.unwrap_or(1), 1), msg));
            }
            None => s.err(
                &At {
                    file,
                    path: path.into(),
                },
                msg,
            ),
        };
        match template::template_vars(src) {
            Err(e) => report(self, e.line, format!("invalid template: {}", e.message)),
            Ok(vars) => {
                let msgs = template::check_vars(&vars, scope);
                for (v, m) in vars
                    .iter()
                    .filter(|v| !template::check_vars(&[(*v).clone()].into(), scope).is_empty())
                    .zip(msgs)
                {
                    report(self, template::find_line(src, v), m);
                }
            }
        }
    }

    fn check_value_templates(&mut self, file: &str, path: &str, v: &Value, scope: &Scope) {
        match v {
            Value::String(s) => self.check_template(file, path, s, scope, None),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    self.check_value_templates(file, &format!("{path}[{i}]"), x, scope);
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    self.check_value_templates(file, &format!("{path}.{k}"), x, scope);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ step

    fn step_use(
        &mut self,
        ctx: &Ctx,
        file: &str,
        base: &str,
        s: &StepUse,
        enclosing: &Map<String, Value>,
    ) -> Option<RStep> {
        let at = At {
            file,
            path: base.to_string(),
        };
        let with_at = format!("{base}.with");
        match (&s.use_, &s.inline) {
            (Some(r), None) => {
                let uat = At {
                    file,
                    path: format!("{base}.use"),
                };
                let (repo, rref, path) = self.locate(ctx, r, &uat, true)?;
                let k = self.load(&repo, &rref, &path, true, Some(&uat))?;
                let sf: StepFile = self.typed(&k)?;
                let mut imports = if repo == ctx.repo {
                    ctx.imports.clone()
                } else {
                    Imports::new()
                };
                imports.extend(sf.imports.clone());
                let sctx = Ctx {
                    repo,
                    rref,
                    imports,
                };
                self.step(
                    &sctx,
                    &k,
                    "",
                    r,
                    &sf,
                    &s.with,
                    ctx,
                    Some((file, &with_at)),
                    enclosing,
                )
            }
            (None, Some(inline)) => {
                let v = self.source_value(file, &format!("{base}.inline"));
                let f = self.files.get(file)?.1.clone()?;
                let sf: StepFile = match typed_at(&f, &format!("{base}.inline"), v) {
                    Ok(sf) => sf,
                    Err(d) => {
                        self.diags.push(d);
                        return None;
                    }
                };
                debug_assert_eq!(&sf, inline.as_ref());
                self.step(
                    ctx,
                    file,
                    &format!("{base}.inline"),
                    "inline",
                    &sf,
                    &s.with,
                    ctx,
                    Some((file, &with_at)),
                    enclosing,
                )
            }
            _ => {
                self.err(
                    &at,
                    "a step is `{use: <ref>, with: {…}}` or `{inline: {…}}`",
                );
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn step(
        &mut self,
        sctx: &Ctx,
        k: &str,
        base: &str,
        name: &str,
        sf: &StepFile,
        with: &Map<String, Value>,
        with_ctx: &Ctx,
        with_at: Option<(&str, &str)>,
        enclosing: &Map<String, Value>,
    ) -> Option<RStep> {
        let p = |s: &str| {
            let mut b = base.to_string();
            join(&mut b, s);
            b
        };
        self.check_version(k, sf.version);
        self.check_imports(k, &sf.imports);
        let params = self.bind(&sf.params, sctx, k, with, with_ctx, with_at, enclosing);
        let mut scope = Scope {
            names: sf.inputs.keys().cloned().collect(),
            params: sf.params.keys().cloned().collect(),
        };
        for (n, i) in &sf.inputs {
            let root = i.from.split('.').next().unwrap_or_default();
            if !ROOTS.contains(&root) && root != "params" {
                self.err(
                    &At {
                        file: k,
                        path: p(&format!("inputs.{n}.from")),
                    },
                    format!(
                        "`{}` is not a task document path (roots: {})",
                        i.from,
                        ROOTS.join(", ")
                    ),
                );
            }
            if ROOTS.contains(&n.as_str()) || n == "params" {
                self.err(
                    &At {
                        file: k,
                        path: p(&format!("inputs.{n}")),
                    },
                    format!("input `{n}` shadows a task document root"),
                );
            }
        }
        let exec = match (&sf.agent, &sf.command) {
            (Some(a), None) => {
                let at = At {
                    file: k,
                    path: p("agent.prompt"),
                };
                let (prompt, pfile) = self.source(sctx, &a.prompt, &params, &at)?;
                self.check_template(k, &p("agent.prompt"), &prompt, &scope, pfile.as_deref());
                let rat = At {
                    file: k,
                    path: p("agent.runtime"),
                };
                let (rrepo, rref, rpath) = self.locate(sctx, &a.runtime, &rat, true)?;
                let rk = self.load(&rrepo, &rref, &rpath, true, Some(&rat))?;
                let rf: RuntimeFile = self.typed(&rk)?;
                self.check_version(&rk, Some(rf.version));
                self.check_secrets(&rk, "secrets", &rf.secrets);
                let model = a
                    .model
                    .as_ref()
                    .map(|m| self.text(m, &params, k, &p("agent.model"), &scope))
                    .map(|m| m.as_str().to_string());
                if rf.launch.command.is_empty() {
                    self.err(
                        &At {
                            file: &rk,
                            path: "launch.command".into(),
                        },
                        "launch.command needs at least the program",
                    );
                }
                if sf.result.is_some() && !rf.capabilities.structured_output {
                    self.warn(&At { file: k, path: p("result") }, format!("runtime `{}` does not declare structured_output; `submit_result` may be unavailable", a.runtime));
                }
                let runtime = RRuntime {
                    name: a.runtime.clone(),
                    origin: self.origin(&rk),
                    file: rf,
                };
                Exec::Agent {
                    runtime: Box::new(runtime),
                    model,
                    prompt,
                }
            }
            (None, Some(c)) => {
                let at = At {
                    file: k,
                    path: p("command.run"),
                };
                let (run, rfile) = self.source(sctx, &c.run, &params, &at)?;
                self.check_template(k, &p("command.run"), &run, &scope, rfile.as_deref());
                Exec::Command {
                    run,
                    tools: c.tools.clone(),
                }
            }
            _ => {
                self.err(
                    &At {
                        file: k,
                        path: base.to_string(),
                    },
                    "a step has exactly one of `agent` and `command`",
                );
                return None;
            }
        };
        if let Some(r) = &sf.result
            && !r.schema.is_object()
        {
            self.err(
                &At {
                    file: k,
                    path: p("result.schema"),
                },
                "result.schema must be a JSON Schema object",
            );
        }
        for (i, w) in sf.workspaces.iter().enumerate() {
            self.check_template(
                k,
                &p(&format!("workspaces[{i}].repo")),
                &w.repo,
                &scope,
                None,
            );
            for (f, v) in [("branch", &w.branch), ("base", &w.base)] {
                if let Some(v) = v {
                    self.check_template(k, &p(&format!("workspaces[{i}].{f}")), v, &scope, None);
                }
            }
            if w.branch.is_some() && w.mode == Access::Read {
                self.err(
                    &At {
                        file: k,
                        path: p(&format!("workspaces[{i}].branch")),
                    },
                    "`branch` is only for write workspaces",
                );
            }
        }
        let secrets = self.check_secrets(k, &p("secrets"), &sf.secrets);
        if let Some(c) = &sf.concurrency {
            self.check_template(k, &p("concurrency.group"), &c.group, &scope, None);
            if c.max == 0 {
                self.err(
                    &At {
                        file: k,
                        path: p("concurrency.max"),
                    },
                    "concurrency.max must be at least 1",
                );
            }
        }
        scope.names.insert("event".into());
        self.check_matchers(k, &p("steer"), &sf.steer, &scope);
        if let Some(stop) = &sf.stop {
            self.check_matchers(k, &p("stop"), stop, &scope);
        }
        Some(RStep {
            name: name.to_string(),
            origin: self.origin(k),
            params,
            exec,
            inputs: sf.inputs.clone(),
            outputs: sf.outputs.clone(),
            result: sf.result.clone(),
            workspaces: sf.workspaces.clone(),
            github: sf.github.clone(),
            secrets,
            priority: sf.priority,
            concurrency: sf.concurrency.clone(),
            retry: sf.retry.clone(),
            steer: sf.steer.clone(),
            stop: sf.stop.clone().unwrap_or_else(default_stop),
        })
    }
}

fn next_route(ids: &[String], i: usize, exit: &str) -> Route {
    match ids.get(i + 1) {
        Some(n) => Route::Phase(n.clone()),
        None => Route::End(exit.to_string()),
    }
}

fn targets(n: &Node) -> Vec<String> {
    let mut t = Vec::new();
    match &n.next {
        Some(Next::To(x)) => t.push(x.clone()),
        Some(Next::Branches(bs)) => t.extend(
            bs.iter()
                .filter_map(|b| b.to.clone().or_else(|| b.else_.clone())),
        ),
        None => {}
    }
    t.extend(n.on_limit.clone());
    t
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

fn is_env_name(s: &str) -> bool {
    is_ident(s)
}

/// `{param: <name>}` (an object with exactly that key).
fn is_param_ref(v: &Value) -> bool {
    v.as_object()
        .is_some_and(|o| o.len() == 1 && o.get("param").is_some_and(Value::is_string))
}

/// Replaces every `{param: x}` in a value with the param's value; `Err`
/// names an unknown param.
fn subst_params(v: &Value, params: &Map<String, Value>) -> Result<Value, String> {
    if is_param_ref(v) {
        let p = v["param"].as_str().unwrap_or_default();
        return params.get(p).cloned().ok_or_else(|| p.to_string());
    }
    Ok(match v {
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|x| subst_params(x, params))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| Ok((k.clone(), subst_params(x, params)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

fn placeholder(p: &Param) -> Value {
    match p.ty {
        // 1, not 0: integer params often feed caps that must be at least 1.
        ParamType::Integer => json!(1),
        ParamType::Boolean => json!(false),
        ParamType::Enum => json!(p.values.first().cloned().unwrap_or_default()),
        ParamType::List => json!([]),
        _ => json!(""),
    }
}

enum Seg<'a> {
    Key(&'a str),
    Index(usize),
}

fn split_path(p: &str) -> Vec<Seg<'_>> {
    let mut out = Vec::new();
    for part in p.split('.').filter(|s| !s.is_empty()) {
        let (name, rest) = part.split_once('[').map_or((part, ""), |(a, b)| (a, b));
        if !name.is_empty() {
            out.push(Seg::Key(name));
        }
        for idx in rest.split('[') {
            if let Ok(i) = idx.trim_end_matches(']').parse() {
                out.push(Seg::Index(i));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- bindings

/// One bindings layer, lowest first: library, Project, repo.
#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    /// Repository and ref that unaliased `use` references resolve against.
    pub repo: String,
    pub rref: String,
    pub file: BindingsFile,
}

/// A binding after layering, with its workflow reference located.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RBinding {
    pub id: Option<String>,
    pub layer: String,
    pub on: Trigger,
    pub cond: Option<String>,
    /// Workflow params; string leaves are templates over the routing context.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub with: Map<String, Value>,
    pub workflow: Entry,
}

/// The binding chosen for an event, with its `with` rendered.
#[derive(Debug, Clone, PartialEq)]
pub struct Selected<'a> {
    pub binding: &'a RBinding,
    /// Params for [`resolve_with`].
    pub params: Map<String, Value>,
}

/// Pure merge of bindings layers. A binding with the id of a lower-layer
/// binding replaces it (`disabled: true` removes it). The result lists the
/// highest layer first; [`select_binding`] takes the first match.
pub fn merge_bindings(layers: &[Layer]) -> Result<Vec<RBinding>, String> {
    let mut winner: BTreeMap<String, usize> = BTreeMap::new();
    for (li, l) in layers.iter().enumerate() {
        for b in &l.file.bindings {
            if let Some(id) = &b.id {
                winner.insert(id.clone(), li);
            }
        }
    }
    let mut out = Vec::new();
    for (li, l) in layers.iter().enumerate().rev() {
        for b in &l.file.bindings {
            if b.id.as_ref().is_some_and(|id| winner[id] != li) || b.disabled {
                continue;
            }
            let r = b
                .use_
                .as_deref()
                .ok_or_else(|| format!("{}: binding {:?} has no `use`", l.name, b.id))?;
            let on =
                b.on.clone()
                    .ok_or_else(|| format!("{}: binding {:?} has no `on`", l.name, b.id))?;
            let (repo, rref, path) = match r.split_once('/') {
                Some((alias, rest)) if l.file.imports.contains_key(alias) => {
                    let (repo, rref) = l.file.imports[alias].split_once('@').ok_or_else(|| {
                        format!("{}: import `{alias}` must be owner/repo@ref", l.name)
                    })?;
                    (repo.to_string(), rref.to_string(), rest.to_string())
                }
                _ => (l.repo.clone(), l.rref.clone(), r.to_string()),
            };
            out.push(RBinding {
                id: b.id.clone(),
                layer: l.name.clone(),
                on,
                cond: b.cond.clone(),
                with: b.with.clone(),
                workflow: Entry {
                    repo,
                    rref,
                    path: with_ext(&path),
                },
            });
        }
    }
    Ok(out)
}

/// The binding that starts a task for `event`, if any (routing context
/// without relations; see [`select_binding_in`]).
pub fn select_binding<'a>(
    bindings: &'a [RBinding],
    event: &crate::events::Event,
) -> Result<Option<&'a RBinding>, String> {
    let ctx = crate::document::RoutingContext {
        event: event.clone(),
        relations: Vec::new(),
    };
    Ok(select_binding_in(bindings, &ctx)?.map(|s| s.binding))
}

/// The binding that starts a task for the routing context, with its
/// `with` rendered over `{event, relations}`.
pub fn select_binding_in<'a>(
    bindings: &'a [RBinding],
    routing: &crate::document::RoutingContext,
) -> Result<Option<Selected<'a>>, String> {
    let event = &routing.event;
    let ctx = serde_json::to_value(routing).map_err(|e| e.to_string())?;
    for b in bindings {
        let on = match (&b.on, &event.payload) {
            (Trigger::Event(t), _) => t == event.type_name(),
            (Trigger::Schedule { schedule }, crate::events::Payload::Schedule(s)) => {
                *schedule == s.cron
            }
            _ => false,
        };
        if !on {
            continue;
        }
        if let Some(c) = &b.cond
            && !template::truthy(c, &ctx)
                .map_err(|e| format!("binding {:?}: {}", b.id, e.message))?
        {
            continue;
        }
        let params = match template::render_value(&Value::Object(b.with.clone()), &ctx)
            .map_err(|e| format!("binding {:?}: with: {}", b.id, e.message))?
        {
            Value::Object(o) => o,
            _ => Map::new(),
        };
        return Ok(Some(Selected { binding: b, params }));
    }
    Ok(None)
}
