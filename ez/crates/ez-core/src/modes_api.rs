//! Machine-readable face of the mode/hook editor, shared by `ez modes ... --json` (used inside
//! WSL by the desktop app) and the desktop app's local Tauri commands, so both produce exactly the
//! same JSON (camelCase).
//!
//! * [`get`] reads the project's `.ez/modes.toml` (plus a catalog of tools, mise tasks,
//!   profiles and the lifecycle templates for the editor's pickers).
//! * [`validate`] checks an edited document (unknown tools, bad globs and regexes, dangling
//!   `next` / `auto_advance_to`, duplicate ids, ...).
//! * [`preview`] / [`apply`] write the document back with `toml_edit`, keeping the comments,
//!   ordering and formatting of everything that was not edited (see `modes_edit`).
//!
//! The project's `[hooks]` table (from the merged `config.toml`) is returned as `globalHooks`
//! for display only; the editor does not change it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ez_hooks::{HookEvent, HookSpec, HooksConfig, OnFailure};
use globset::Glob;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Config;
use crate::init;
use crate::modes::ModeDef;
use crate::permissions::Permission;

pub const HOOK_EVENTS: [HookEvent; 7] = [
    HookEvent::SessionStart,
    HookEvent::UserPromptSubmit,
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::PostEdit,
    HookEvent::ModeChange,
    HookEvent::Stop,
];

pub const DEFAULT_MAX_FIX_TURNS: u32 = 2;
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

pub fn modes_path(project: &Path) -> PathBuf {
    project.join(".ez").join("modes.toml")
}

// ----- documents -----

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookDto {
    pub run: String,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default)]
    pub matcher: Option<String>,
    /// `ignore` (default), `block` or `fix_turn`.
    #[serde(default = "default_on_failure")]
    pub on_failure: String,
    #[serde(default = "default_fix_turns")]
    pub max_fix_turns: u32,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_on_failure() -> String {
    "ignore".into()
}
fn default_fix_turns() -> u32 {
    DEFAULT_MAX_FIX_TURNS
}
fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

impl HookDto {
    fn from_spec(s: &HookSpec) -> Self {
        Self {
            run: s.run.clone(),
            glob: s.glob.clone(),
            matcher: s.matcher.clone(),
            on_failure: on_failure_str(s.on_failure).into(),
            max_fix_turns: s.max_fix_turns,
            timeout_secs: s.timeout_secs,
        }
    }
}

fn on_failure_str(o: OnFailure) -> &'static str {
    match o {
        OnFailure::Ignore => "ignore",
        OnFailure::Block => "block",
        OnFailure::FixTurn => "fix_turn",
    }
}

fn perm_str(p: Permission) -> &'static str {
    match p {
        Permission::Allow => "allow",
        Permission::Ask => "ask",
        Permission::Deny => "deny",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeDto {
    pub id: String,
    /// The id this mode has in the file on disk. Set by `get` for every existing mode; when it
    /// differs from `id` the mode is renamed (its section headers are rewritten). Absent for new
    /// modes. It is not stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub prompt: String,
    /// `null` means every built-in tool.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub mise_tasks: Vec<String>,
    #[serde(default)]
    pub mcp_tools: Vec<String>,
    #[serde(default)]
    pub next: Vec<String>,
    #[serde(default)]
    pub auto_advance_to: Vec<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Tool name or glob to `allow` / `ask` / `deny`.
    #[serde(default)]
    pub permissions: BTreeMap<String, String>,
    /// Hooks per event (`session_start`, `post_edit`, ...); events without hooks are absent.
    #[serde(default)]
    pub hooks: BTreeMap<String, Vec<HookDto>>,
}

impl ModeDto {
    pub fn from_def(id: &str, d: &ModeDef) -> Self {
        let mut hooks = BTreeMap::new();
        for ev in HOOK_EVENTS {
            let list = d.hooks.get(ev);
            if !list.is_empty() {
                hooks.insert(
                    ev.as_str().to_string(),
                    list.iter().map(HookDto::from_spec).collect(),
                );
            }
        }
        Self {
            id: id.into(),
            original_id: None,
            name: d.name.clone(),
            description: d.description.clone(),
            prompt: d.prompt.clone(),
            tools: d.tools.clone(),
            mise_tasks: d.mise_tasks.clone(),
            mcp_tools: d.mcp_tools.clone(),
            next: d.next.clone(),
            auto_advance_to: d.auto_advance_to.clone(),
            profile: d.profile.clone(),
            model: d.model.clone(),
            permissions: d
                .permissions
                .iter()
                .map(|(k, v)| (k.clone(), perm_str(*v).to_string()))
                .collect(),
            hooks,
        }
    }

    /// Canonical form: empty strings become `None`, empty hook lists vanish.
    pub fn normalized(mut self) -> Self {
        let blank = |o: &mut Option<String>| {
            if o.as_deref().is_some_and(|s| s.trim().is_empty()) {
                *o = None;
            }
        };
        blank(&mut self.name);
        blank(&mut self.profile);
        blank(&mut self.model);
        self.hooks.retain(|_, v| !v.is_empty());
        for list in self.hooks.values_mut() {
            for h in list {
                blank(&mut h.glob);
                blank(&mut h.matcher);
            }
        }
        self
    }

    /// Equality ignoring `original_id`.
    pub(crate) fn same_content(&self, other: &ModeDto) -> bool {
        let strip = |m: &ModeDto| ModeDto {
            original_id: None,
            ..m.clone()
        };
        strip(self) == strip(other)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModesDoc {
    #[serde(default)]
    pub default_mode: String,
    #[serde(default)]
    pub modes: Vec<ModeDto>,
}

impl ModesDoc {
    pub fn normalized(mut self) -> Self {
        self.modes = self.modes.into_iter().map(ModeDto::normalized).collect();
        self
    }

    /// Rename a mode and update every reference to it (`default_mode`, `next`,
    /// `auto_advance_to`). `original_id` keeps the on-disk id (set when missing).
    pub fn rename_mode(&mut self, old: &str, new: &str) {
        if self.default_mode == old {
            self.default_mode = new.into();
        }
        for m in &mut self.modes {
            if m.id == old {
                m.original_id.get_or_insert_with(|| old.to_string());
                m.id = new.into();
            }
            for list in [&mut m.next, &mut m.auto_advance_to] {
                for r in list.iter_mut().filter(|r| *r == old) {
                    *r = new.into();
                }
            }
        }
    }
}

/// Parse `modes.toml` text leniently: cross references are not checked here (see [`validate`]),
/// so the editor can open and repair a file the agent would refuse. Modes keep file order.
pub fn parse_doc(text: &str) -> Result<ModesDoc> {
    let table: toml::Table = text.parse().context("parsing modes.toml")?;
    let mut modes = Vec::new();
    if let Some(t) = table.get("modes") {
        let t = t.as_table().context("`modes` must be a table")?;
        for (id, v) in t {
            let def: ModeDef = v
                .clone()
                .try_into()
                .with_context(|| format!("invalid mode `{id}`"))?;
            modes.push(ModeDto::from_def(id, &def));
        }
    }
    let default_mode = match table.get("default_mode") {
        Some(v) => v
            .as_str()
            .context("`default_mode` must be a string")?
            .to_string(),
        None => modes.first().map(|m| m.id.clone()).unwrap_or_default(),
    };
    Ok(ModesDoc {
        default_mode,
        modes,
    })
}

// ----- validation -----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// `defaultMode`, `modes`, `modes[2].id`, `modes[2].tools[1]`,
    /// `modes[2].permissions.Bash`, `modes[2].hooks.post_edit[0].matcher`, ...
    pub path: String,
    pub message: String,
    /// `error` (blocks saving), `warning` or `info`.
    pub severity: String,
}

fn issue(out: &mut Vec<Issue>, sev: &str, path: String, message: impl Into<String>) {
    out.push(Issue {
        path,
        message: message.into(),
        severity: sev.into(),
    });
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Built-in tool names plus the agent-level `SwitchMode`.
fn known_tool_names() -> Vec<String> {
    let mut v: Vec<String> = ez_tools::builtin_tools()
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    v.push("SwitchMode".into());
    v
}

/// Check a document. `profiles` (configured profile names) enables the unknown-profile warning.
pub fn validate_doc(doc: &ModesDoc, profiles: Option<&[String]>) -> Vec<Issue> {
    let mut out = Vec::new();
    let doc = doc.clone().normalized();
    let tools = known_tool_names();
    if doc.modes.is_empty() {
        issue(
            &mut out,
            "error",
            "modes".into(),
            "Define at least one mode.",
        );
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (i, m) in doc.modes.iter().enumerate() {
        let p = format!("modes[{i}]");
        if !valid_id(&m.id) {
            issue(
                &mut out,
                "error",
                format!("{p}.id"),
                if m.id.is_empty() {
                    "The id is required.".to_string()
                } else {
                    format!(
                        "`{}` is not a valid id: use letters, digits, `-` and `_` (max 64).",
                        m.id
                    )
                },
            );
        } else if !seen.insert(m.id.as_str()) {
            issue(
                &mut out,
                "error",
                format!("{p}.id"),
                format!("Duplicate mode id `{}`.", m.id),
            );
        }
    }
    let ids: BTreeSet<&str> = doc.modes.iter().map(|m| m.id.as_str()).collect();
    if !doc.modes.is_empty() && !ids.contains(doc.default_mode.as_str()) {
        issue(
            &mut out,
            "error",
            "defaultMode".into(),
            if doc.default_mode.is_empty() {
                "Choose the default mode.".to_string()
            } else {
                format!("The default mode `{}` does not exist.", doc.default_mode)
            },
        );
    }
    for (i, m) in doc.modes.iter().enumerate() {
        let p = format!("modes[{i}]");
        if let Some(t) = &m.tools {
            let mut dup = BTreeSet::new();
            for (j, t) in t.iter().enumerate() {
                let tp = format!("{p}.tools[{j}]");
                if !dup.insert(t.as_str()) {
                    issue(&mut out, "warning", tp, format!("`{t}` is listed twice."));
                } else if t.trim().is_empty() {
                    issue(&mut out, "error", tp, "Empty tool name.");
                } else if !(tools.iter().any(|k| k == t)
                    || t.starts_with("mise__")
                    || t.starts_with("mcp__"))
                {
                    issue(
                        &mut out,
                        "warning",
                        tp,
                        format!("Unknown tool `{t}`; the agent ignores it."),
                    );
                }
            }
        }
        for (j, g) in m.mise_tasks.iter().enumerate() {
            if let Err(e) = Glob::new(g) {
                issue(
                    &mut out,
                    "error",
                    format!("{p}.miseTasks[{j}]"),
                    format!("Invalid glob `{g}`: {}", e.kind()),
                );
            }
        }
        for (j, g) in m.mcp_tools.iter().enumerate() {
            let gp = format!("{p}.mcpTools[{j}]");
            match Glob::new(g) {
                Err(e) => issue(
                    &mut out,
                    "error",
                    gp,
                    format!("Invalid glob `{g}`: {}", e.kind()),
                ),
                Ok(_) if !g.starts_with("mcp__") && !g.starts_with('*') => issue(
                    &mut out,
                    "warning",
                    gp,
                    format!(
                        "`{g}` matches full MCP tool names, which look like `mcp__<server>__<tool>`."
                    ),
                ),
                Ok(_) => {}
            }
        }
        for (key, list) in [("next", &m.next), ("autoAdvanceTo", &m.auto_advance_to)] {
            let mut dup = BTreeSet::new();
            for (j, r) in list.iter().enumerate() {
                let rp = format!("{p}.{key}[{j}]");
                if !dup.insert(r.as_str()) {
                    issue(&mut out, "warning", rp, format!("`{r}` is listed twice."));
                } else if !ids.contains(r.as_str()) {
                    issue(
                        &mut out,
                        "error",
                        rp,
                        format!("`{r}` is not a defined mode."),
                    );
                } else if *r == m.id {
                    issue(
                        &mut out,
                        "warning",
                        rp,
                        "A mode cannot switch to itself; this entry is ignored.",
                    );
                } else if key == "autoAdvanceTo" && !m.next.is_empty() && !m.next.contains(r) {
                    issue(
                        &mut out,
                        "warning",
                        rp,
                        format!("`{r}` is not in `next`, so SwitchMode cannot reach it."),
                    );
                }
            }
        }
        if let (Some(prof), Some(known)) = (&m.profile, profiles)
            && !known.iter().any(|k| k == prof)
        {
            issue(
                &mut out,
                "warning",
                format!("{p}.profile"),
                format!("Profile `{prof}` is not configured; the session profile is used instead."),
            );
        }
        for (pat, val) in &m.permissions {
            let pp = format!("{p}.permissions.{pat}");
            if pat.trim().is_empty() {
                issue(&mut out, "error", pp.clone(), "Empty tool name or glob.");
            } else if let Err(e) = Glob::new(pat) {
                issue(
                    &mut out,
                    "error",
                    pp.clone(),
                    format!("Invalid glob `{pat}`: {}", e.kind()),
                );
            }
            if !["allow", "ask", "deny"].contains(&val.as_str()) {
                issue(
                    &mut out,
                    "error",
                    pp,
                    format!("`{val}` is not allow, ask or deny."),
                );
            }
        }
        for (ev, list) in &m.hooks {
            let Some(event) = HOOK_EVENTS.iter().find(|e| e.as_str() == ev) else {
                issue(
                    &mut out,
                    "error",
                    format!("{p}.hooks.{ev}"),
                    format!("Unknown hook event `{ev}`."),
                );
                continue;
            };
            for (j, h) in list.iter().enumerate() {
                let hp = format!("{p}.hooks.{ev}[{j}]");
                if h.run.trim().is_empty() {
                    issue(
                        &mut out,
                        "error",
                        format!("{hp}.run"),
                        "The command is required.",
                    );
                }
                if let Some(g) = &h.glob {
                    if let Err(e) = Glob::new(g) {
                        issue(
                            &mut out,
                            "error",
                            format!("{hp}.glob"),
                            format!("Invalid glob `{g}`: {}", e.kind()),
                        );
                    } else if *event != HookEvent::PostEdit {
                        issue(
                            &mut out,
                            "warning",
                            format!("{hp}.glob"),
                            "The glob only applies to post_edit hooks.",
                        );
                    }
                }
                if let Some(mt) = &h.matcher {
                    if let Err(e) = Regex::new(&format!("^(?:{mt})$")) {
                        let msg = e.to_string();
                        let short = msg.lines().last().unwrap_or("invalid pattern");
                        issue(
                            &mut out,
                            "error",
                            format!("{hp}.matcher"),
                            format!("Invalid regex: {short}"),
                        );
                    } else if !matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse) {
                        issue(
                            &mut out,
                            "warning",
                            format!("{hp}.matcher"),
                            "The matcher only applies to pre_tool_use and post_tool_use hooks.",
                        );
                    }
                }
                if !["ignore", "block", "fix_turn"].contains(&h.on_failure.as_str()) {
                    issue(
                        &mut out,
                        "error",
                        format!("{hp}.onFailure"),
                        format!("`{}` is not ignore, block or fix_turn.", h.on_failure),
                    );
                }
                if h.timeout_secs == 0 {
                    issue(
                        &mut out,
                        "error",
                        format!("{hp}.timeoutSecs"),
                        "The timeout must be at least 1 second.",
                    );
                }
                if h.on_failure == "fix_turn" && h.max_fix_turns == 0 {
                    issue(
                        &mut out,
                        "warning",
                        format!("{hp}.maxFixTurns"),
                        "With 0 fix turns a failing hook never triggers a fix.",
                    );
                }
            }
        }
    }
    out
}

/// [`validate_doc`] with the project's configured profile names.
pub fn validate(project: &Path, user_dir: Option<&Path>, doc: &ModesDoc) -> Vec<Issue> {
    let profiles = profile_names(project, user_dir);
    validate_doc(doc, profiles.as_deref())
}

fn profile_names(project: &Path, user_dir: Option<&Path>) -> Option<Vec<String>> {
    Config::load(project, user_dir)
        .ok()
        .map(|c| c.profiles.keys().cloned().collect())
}

// ----- get -----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub kind: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MiseTaskInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileEntry {
    pub name: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HookEventInfo {
    pub name: String,
    /// `glob` applies (post_edit).
    pub glob: bool,
    /// `matcher` applies (pre/post_tool_use).
    pub matcher: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateInfo {
    /// `full` (the six lifecycle modes) or `minimal` (plan/build/verify).
    pub set: String,
    pub mode: ModeDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub builtin_tools: Vec<ToolInfo>,
    pub mise_tasks: Vec<MiseTaskInfo>,
    /// Configured MCP server names (tools are only known once a server runs, so the editor
    /// suggests `mcp__<server>__*` and accepts free-text globs).
    pub mcp_servers: Vec<String>,
    pub profiles: Vec<ProfileEntry>,
    pub default_profile: Option<String>,
    pub hook_events: Vec<HookEventInfo>,
    pub permission_values: Vec<String>,
    pub on_failure_values: Vec<String>,
    pub templates: Vec<TemplateInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModesView {
    /// `missing`, `broken` or `ready`.
    pub status: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mode: Option<String>,
    #[serde(default)]
    pub modes: Vec<ModeDto>,
    /// Merged `[hooks]` of user and project `config.toml`; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_hooks: Option<BTreeMap<String, Vec<HookDto>>>,
    pub catalog: Catalog,
}

fn hooks_map(h: &HooksConfig) -> BTreeMap<String, Vec<HookDto>> {
    HOOK_EVENTS
        .iter()
        .filter(|e| !h.get(**e).is_empty())
        .map(|e| {
            (
                e.as_str().to_string(),
                h.get(*e).iter().map(HookDto::from_spec).collect(),
            )
        })
        .collect()
}

fn event_info() -> Vec<HookEventInfo> {
    let d = |n: &str, glob, matcher, text: &str| HookEventInfo {
        name: n.into(),
        glob,
        matcher,
        description: text.into(),
    };
    vec![
        d(
            "session_start",
            false,
            false,
            "When a session starts or resumes.",
        ),
        d(
            "user_prompt_submit",
            false,
            false,
            "Before a prompt is sent to the model.",
        ),
        d(
            "pre_tool_use",
            false,
            true,
            "Before a tool runs; a failing hook can block the call.",
        ),
        d("post_tool_use", false, true, "After a tool ran."),
        d(
            "post_edit",
            true,
            false,
            "After a file edit; {files} expands to the edited files.",
        ),
        d("mode_change", false, false, "After the mode changed."),
        d("stop", false, false, "When the agent finishes a turn."),
    ]
}

fn catalog_templates() -> Vec<TemplateInfo> {
    let mut out = Vec::new();
    for (set, text) in [("full", init::FULL), ("minimal", init::MINIMAL)] {
        if let Ok(doc) = parse_doc(text) {
            out.extend(doc.modes.into_iter().map(|mode| TemplateInfo {
                set: set.into(),
                mode,
            }));
        }
    }
    out
}

/// The project-independent part of the catalog: built-in tools, hook events, permission and
/// `on_failure` values and the lifecycle templates. Mise tasks, MCP servers and profiles are
/// empty (they come from a project; see [`catalog`]).
pub fn base_catalog() -> Catalog {
    let mut builtin: Vec<ToolInfo> = ez_tools::builtin_tools()
        .iter()
        .map(|t| ToolInfo {
            name: t.name().into(),
            kind: t.kind().as_str().into(),
            read_only: t.kind().is_read_only()
                || matches!(t.name(), "TodoWrite" | "AskUserQuestion"),
        })
        .collect();
    builtin.push(ToolInfo {
        name: "SwitchMode".into(),
        kind: "switch_mode".into(),
        read_only: true,
    });
    Catalog {
        builtin_tools: builtin,
        mise_tasks: Vec::new(),
        mcp_servers: Vec::new(),
        profiles: Vec::new(),
        default_profile: None,
        hook_events: event_info(),
        permission_values: ["allow", "ask", "deny"].map(String::from).to_vec(),
        on_failure_values: ["ignore", "block", "fix_turn"].map(String::from).to_vec(),
        templates: catalog_templates(),
    }
}

pub async fn catalog(project: &Path, user_dir: Option<&Path>) -> Catalog {
    let cfg = Config::load(project, user_dir).ok();
    Catalog {
        mise_tasks: ez_mise::discover(project)
            .await
            .into_iter()
            .map(|t| MiseTaskInfo {
                name: t.name,
                description: t.description,
            })
            .collect(),
        mcp_servers: cfg
            .as_ref()
            .map(|c| c.mcp_servers.keys().cloned().collect())
            .unwrap_or_default(),
        profiles: cfg
            .as_ref()
            .map(|c| {
                c.profiles
                    .values()
                    .map(|p| ProfileEntry {
                        name: p.name.clone(),
                        models: p.models.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        default_profile: cfg.map(|c| c.default_profile),
        ..base_catalog()
    }
}

/// The editor's view of the project: file status, the document and the catalog.
pub async fn get(project: &Path, user_dir: Option<&Path>) -> ModesView {
    let path = modes_path(project);
    let catalog = catalog(project, user_dir).await;
    let global_hooks = Config::load(project, user_dir)
        .ok()
        .map(|c| hooks_map(&c.hooks))
        .filter(|h| !h.is_empty());
    let mut view = ModesView {
        status: "missing".into(),
        path: path.display().to_string(),
        error: None,
        default_mode: None,
        modes: vec![],
        global_hooks,
        catalog,
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return view;
    };
    match parse_doc(&text) {
        Err(e) => {
            view.status = "broken".into();
            view.error = Some(format!("{e:#}"));
        }
        Ok(doc) => {
            view.status = "ready".into();
            view.default_mode = Some(doc.default_mode);
            view.modes = doc
                .modes
                .into_iter()
                .map(|mut m| {
                    m.original_id = Some(m.id.clone());
                    m
                })
                .collect();
        }
    }
    view
}

// ----- preview / apply -----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    /// Unified diff of `.ez/modes.toml` (empty when nothing changes).
    pub diff: String,
    pub changed: bool,
    /// Modes whose layout the editor could not edit in place and wrote out fresh (their comments
    /// are lost).
    pub rewritten: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApplyView {
    pub path: String,
    pub changed: bool,
}

fn check_errors(project: &Path, user_dir: Option<&Path>, doc: &ModesDoc) -> Result<()> {
    let errors: Vec<String> = validate(project, user_dir, doc)
        .into_iter()
        .filter(|i| i.severity == "error")
        .map(|i| format!("{}: {}", i.path, i.message))
        .collect();
    if !errors.is_empty() {
        bail!("modes.toml would be invalid: {}", errors.join("; "));
    }
    Ok(())
}

fn render_for(
    project: &Path,
    user_dir: Option<&Path>,
    doc: &ModesDoc,
) -> Result<(Option<String>, crate::modes_edit::Rendered)> {
    check_errors(project, user_dir, doc)?;
    let old = std::fs::read_to_string(modes_path(project)).ok();
    let rendered = crate::modes_edit::render(old.as_deref(), doc)?;
    Ok((old, rendered))
}

pub fn preview(project: &Path, user_dir: Option<&Path>, doc: &ModesDoc) -> Result<PreviewView> {
    let (old, r) = render_for(project, user_dir, doc)?;
    let old = old.unwrap_or_default();
    let changed = old != r.text;
    Ok(PreviewView {
        diff: if changed {
            similar::TextDiff::from_lines(&old, &r.text)
                .unified_diff()
                .context_radius(3)
                .header("current modes.toml", "new modes.toml")
                .to_string()
        } else {
            String::new()
        },
        changed,
        rewritten: r.rewritten,
    })
}

pub fn apply(project: &Path, user_dir: Option<&Path>, doc: &ModesDoc) -> Result<ApplyView> {
    let (old, r) = render_for(project, user_dir, doc)?;
    let path = modes_path(project);
    let changed = old.as_deref() != Some(r.text.as_str());
    if changed {
        std::fs::create_dir_all(path.parent().expect("has parent"))?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, &r.text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
    }
    Ok(ApplyView {
        path: path.display().to_string(),
        changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::{InitOptions, generate};
    use ez_mise::MiseTask;

    fn task(name: &str) -> MiseTask {
        serde_json::from_value(serde_json::json!({"name": name})).unwrap()
    }

    fn template_text() -> String {
        let tasks: Vec<MiseTask> = ["test", "lint", "fmt", "build", "release", "docs", "weird"]
            .map(task)
            .to_vec();
        generate(&tasks, &InitOptions::default()).unwrap()
    }

    fn load(text: &str) -> ModesDoc {
        let mut d = parse_doc(text).unwrap();
        for m in &mut d.modes {
            m.original_id = Some(m.id.clone());
        }
        d
    }

    fn render(old: &str, doc: &ModesDoc) -> String {
        let r = crate::modes_edit::render(Some(old), doc).unwrap();
        assert!(
            r.rewritten.is_empty(),
            "fell back to a fresh rewrite: {:?}",
            r.rewritten
        );
        r.text
    }

    fn mode<'a>(d: &'a mut ModesDoc, id: &str) -> &'a mut ModeDto {
        d.modes.iter_mut().find(|m| m.id == id).unwrap()
    }

    /// Lines removed and added between two texts.
    fn changed(a: &str, b: &str) -> (Vec<String>, Vec<String>) {
        let d = similar::TextDiff::from_lines(a, b);
        let (mut del, mut ins) = (vec![], vec![]);
        for c in d.iter_all_changes() {
            let l = c.value().trim_end().to_string();
            match c.tag() {
                similar::ChangeTag::Delete => del.push(l),
                similar::ChangeTag::Insert => ins.push(l),
                _ => {}
            }
        }
        (del, ins)
    }

    #[test]
    fn unchanged_template_round_trips_byte_identical() {
        for text in [
            template_text(),
            generate(
                &[],
                &InitOptions {
                    minimal: true,
                    only: None,
                },
            )
            .unwrap(),
            generate(&[task("test")], &InitOptions::default()).unwrap(),
        ] {
            assert_eq!(render(&text, &load(&text)), text);
        }
    }

    #[test]
    fn prompt_and_tools_edit_touch_only_those_lines() {
        let text = template_text();
        let mut d = load(&text);
        let m = mode(&mut d, "design");
        m.prompt = "Plan carefully.\nSecond line.".into();
        m.tools = Some(vec!["Read".into(), "Grep".into()]);
        let out = render(&text, &d);
        let (del, ins) = changed(&text, &out);
        assert!(del.iter().any(|l| l.starts_with("tools = [")), "{del:?}");
        assert!(
            ins.contains(&"tools = [\"Read\", \"Grep\"]".to_string()),
            "{ins:?}"
        );
        assert!(ins.contains(&"Plan carefully.".to_string()));
        // comments (the hint above `description`) survive
        assert!(out.contains("# profile = \"openai\""));
        assert!(out.starts_with("# Generated by `ez init`"));
        assert!(out.trim_end().ends_with("# Unassigned mise tasks: weird"));
        let back = parse_doc(&out).unwrap();
        let bm = back.modes.iter().find(|m| m.id == "design").unwrap();
        assert_eq!(bm.prompt, "Plan carefully.\nSecond line.");
        // other modes are byte-identical
        let strip = |s: &str, id: &str| {
            let start = s.find(&format!("[modes.{id}]")).unwrap();
            let end = s[start + 1..]
                .find("\n[modes.")
                .map_or(s.len(), |e| start + 1 + e);
            s[start..end].to_string()
        };
        for id in ["explore", "implement", "test"] {
            assert_eq!(strip(&text, id), strip(&out, id));
        }
    }

    #[test]
    fn defaults_remove_keys_and_set_adds_them() {
        let text = template_text();
        let mut d = load(&text);
        let m = mode(&mut d, "explore");
        m.name = None;
        m.mise_tasks.clear();
        m.profile = Some("openai".into());
        m.model = Some("claude-haiku-5-5".into());
        let out = render(&text, &d);
        let (del, ins) = changed(&text, &out);
        assert!(del.contains(&"name = \"Explore\"".to_string()));
        assert!(del.iter().any(|l| l.starts_with("mise_tasks")));
        assert!(ins.contains(&"profile = \"openai\"".to_string()));
        assert!(ins.contains(&"model = \"claude-haiku-5-5\"".to_string()));
        // the comment that sat above `description` is not lost when `name` goes
        assert!(out.contains("# model = \"claude-haiku-5-5\"   # model id"));
        let back = parse_doc(&out).unwrap();
        assert_eq!(back.modes[0].name, None);
        assert_eq!(back.modes[0].profile.as_deref(), Some("openai"));
    }

    #[test]
    fn permissions_add_change_remove_and_drop_section() {
        let text = template_text();
        let mut d = load(&text);
        let m = mode(&mut d, "implement");
        m.permissions.insert("Bash".into(), "deny".into());
        m.permissions.remove("Write");
        m.permissions.insert("mise__*".into(), "allow".into());
        let out = render(&text, &d);
        let (del, ins) = changed(&text, &out);
        assert_eq!(del.len(), 2, "{del:?}");
        assert!(
            ins.contains(&"\"mise__*\" = \"allow\"".to_string()),
            "{ins:?}"
        );
        let back = parse_doc(&out).unwrap();
        let p = &back.modes[2].permissions;
        assert_eq!(p["Bash"], "deny");
        assert!(!p.contains_key("Write") && p["mise__*"] == "allow");

        // all removed: the section header goes too
        let mut d = load(&text);
        mode(&mut d, "review").permissions.clear();
        let out = render(&text, &d);
        assert!(!out.contains("[modes.review.permissions]"));
        assert!(out.contains("[modes.release.permissions]"));

        // a mode without a permissions section gets one, before its hooks
        let mut d = load(&text);
        let m = mode(&mut d, "design");
        m.permissions.insert("Write".into(), "ask".into());
        let out = render(&text, &d);
        assert!(out.contains("[modes.design.permissions]\nWrite = \"ask\"\n\n[modes.implement]"));
    }

    #[test]
    fn hooks_add_edit_reorder_remove() {
        let text = template_text();
        let hook = |run: &str| HookDto {
            run: run.into(),
            glob: None,
            matcher: None,
            on_failure: "ignore".into(),
            max_fix_turns: 2,
            timeout_secs: 120,
        };
        // implement already has two post_edit hooks (fmt, lint)
        let mut d = load(&text);
        let m = mode(&mut d, "implement");
        let orig = m.hooks["post_edit"].clone();
        let list = m.hooks.get_mut("post_edit").unwrap();
        list.push(HookDto {
            glob: Some("*.rs".into()),
            on_failure: "fix_turn".into(),
            max_fix_turns: 3,
            ..hook("cargo fmt {files}")
        });
        let out = render(&text, &d);
        let (del, ins) = changed(&text, &out);
        assert!(del.is_empty(), "{del:?}");
        assert!(ins.contains(&"run = \"cargo fmt {files}\"".to_string()));
        assert!(ins.contains(&"glob = \"*.rs\"".to_string()));
        assert!(ins.contains(&"max_fix_turns = 3".to_string()));
        let back = parse_doc(&out).unwrap();
        assert_eq!(back.modes[2].hooks["post_edit"].len(), 3);
        assert_eq!(back.modes[2].hooks["post_edit"][..2], orig[..]);

        // reorder + new event on a mode without hooks
        let mut d = load(&text);
        let m = mode(&mut d, "implement");
        m.hooks.get_mut("post_edit").unwrap().reverse();
        let out = render(&text, &d);
        let back = parse_doc(&out).unwrap();
        assert_eq!(
            back.modes[2].hooks["post_edit"],
            orig.iter().rev().cloned().collect::<Vec<_>>()
        );
        let mut d = load(&out);
        mode(&mut d, "review").hooks.insert(
            "pre_tool_use".into(),
            vec![HookDto {
                matcher: Some("Bash|Edit".into()),
                on_failure: "block".into(),
                ..hook("./check.sh")
            }],
        );
        mode(&mut d, "review")
            .hooks
            .insert("stop".into(), vec![hook("echo done")]);
        let out2 = render(&out, &d);
        let back = parse_doc(&out2).unwrap();
        let r = back.modes.iter().find(|m| m.id == "review").unwrap();
        assert_eq!(
            r.hooks["pre_tool_use"][0].matcher.as_deref(),
            Some("Bash|Edit")
        );
        assert_eq!(r.hooks["stop"][0].run, "echo done");

        // edit one field in place, keeping a trailing comment
        let src = "[modes.a]\n\n[[modes.a.hooks.post_edit]]\nrun = \"fmt\" # keep me\non_failure = \"block\"\n";
        let mut d = load(src);
        mode(&mut d, "a").hooks.get_mut("post_edit").unwrap()[0].run = "fmt2".into();
        assert_eq!(
            render(src, &d),
            "[modes.a]\n\n[[modes.a.hooks.post_edit]]\nrun = \"fmt2\" # keep me\non_failure = \"block\"\n"
        );
        // remove all hooks of an event
        let mut d = load(src);
        mode(&mut d, "a").hooks.clear();
        assert_eq!(render(src, &d), "[modes.a]\n");
    }

    #[test]
    fn rename_updates_references_and_headers() {
        let text = template_text();
        let mut d = load(&text);
        d.rename_mode("implement", "build");
        assert_eq!(d.modes[2].id, "build");
        assert!(d.modes[1].next.contains(&"build".to_string()));
        let out = render(&text, &d);
        assert!(out.contains("[modes.build]"));
        assert!(out.contains("[modes.build.permissions]"));
        assert!(out.contains("[[modes.build.hooks.post_edit]]"));
        assert!(!out.contains("modes.implement"));
        assert!(out.contains("# mcp_tools = [\"mcp__github__*\"]"));
        let (del, ins) = changed(&text, &out);
        // only the headers and the references changed
        assert!(del.len() <= 8 && del.len() == ins.len(), "{del:?} {ins:?}");
        let set = crate::ModeSet::from_toml_str(&out).unwrap();
        assert_eq!(set.get("design").unwrap().next, ["build"]);
        assert_eq!(set.get("test").unwrap().next, ["build", "review"]);

        // default_mode follows
        let mut d = load(&text);
        d.rename_mode("explore", "scout");
        let out = render(&text, &d);
        assert!(out.contains("default_mode = \"scout\""));
        crate::ModeSet::from_toml_str(&out).unwrap();
    }

    #[test]
    fn reorder_add_remove_modes() {
        let text = template_text();
        let mut d = load(&text);
        d.modes.swap(1, 4); // design <-> review
        let out = render(&text, &d);
        let set = crate::ModeSet::from_toml_str(&out).unwrap();
        assert_eq!(
            set.ids(),
            [
                "explore",
                "review",
                "implement",
                "test",
                "design",
                "release"
            ]
        );
        assert!(out.contains("# profile = \"openai\""));
        assert!(out.trim_end().ends_with("# Unassigned mise tasks: weird"));
        // moving the last mode up keeps the trailing comment last
        let mut d = load(&text);
        let last = d.modes.pop().unwrap();
        d.modes.insert(0, last);
        d.default_mode = "release".into();
        let out = render(&text, &d);
        assert!(out.trim_end().ends_with("# Unassigned mise tasks: weird"));
        assert_eq!(
            crate::ModeSet::from_toml_str(&out).unwrap().ids()[0],
            "release"
        );
        assert!(out.contains("default_mode = \"release\""));

        // add (at the end and in the middle) and remove
        let mut d = load(&text);
        d.modes.push(ModeDto {
            id: "docs".into(),
            description: "Write docs".into(),
            prompt: "Write.\nMore.".into(),
            tools: Some(vec!["Read".into()]),
            permissions: [("Write".to_string(), "allow".to_string())].into(),
            ..Default::default()
        });
        d.modes.insert(
            2,
            ModeDto {
                id: "spike".into(),
                ..Default::default()
            },
        );
        d.modes.retain(|m| m.id != "release");
        for m in &mut d.modes {
            m.next.retain(|n| n != "release");
            m.auto_advance_to.retain(|n| n != "release");
        }
        let out = render(&text, &d);
        let set = crate::ModeSet::from_toml_str(&out).unwrap();
        assert_eq!(
            set.ids(),
            [
                "explore",
                "design",
                "spike",
                "implement",
                "test",
                "review",
                "docs"
            ]
        );
        assert!(out.trim_end().ends_with("# Unassigned mise tasks: weird"));
        assert_eq!(set.get("docs").unwrap().prompt, "Write.\nMore.");
        assert!(!out.contains("modes.release"));
        // stable: applying the result again changes nothing
        let again = load(&out);
        assert_eq!(render(&out, &again), out);
    }

    #[test]
    fn default_mode_edit_and_implicit() {
        let text = "[modes.a]\n[modes.b]\n";
        let mut d = load(text);
        assert_eq!(d.default_mode, "a");
        assert_eq!(render(text, &d), text);
        d.default_mode = "b".into();
        assert_eq!(
            render(text, &d),
            "default_mode = \"b\"\n\n[modes.a]\n[modes.b]\n"
        );
        // moving another mode first keeps the explicit default
        let mut d = load(text);
        d.modes.swap(0, 1);
        let out = render(text, &d);
        assert_eq!(parse_doc(&out).unwrap().default_mode, "a");
    }

    #[test]
    fn unusual_layout_is_rewritten_only_when_edited() {
        let text = "default_mode = \"a\"\n\n# keep\n[modes.a]\npermissions = { Bash = \"ask\" }\nhooks.post_edit = [{ run = \"x\" }]\n\n[modes.b]\n# note\nname = \"B\"\n";
        let d = load(text);
        assert_eq!(render(text, &d), text);
        let mut d = load(text);
        mode(&mut d, "b").name = Some("Bee".into());
        let out = render(text, &d);
        assert!(out.contains("# keep") && out.contains("permissions = { Bash"));
        assert!(out.contains("# note\nname = \"Bee\""));
        let mut d = load(text);
        mode(&mut d, "a").description = "x".into();
        let r = crate::modes_edit::render(Some(text), &d).unwrap();
        assert_eq!(r.rewritten, ["a"]);
        let back = parse_doc(&r.text).unwrap();
        assert_eq!(back.modes[0].permissions["Bash"], "ask");
        assert_eq!(back.modes[0].hooks["post_edit"][0].run, "x");
    }

    #[test]
    fn crlf_and_missing_file() {
        let text = template_text().replace('\n', "\r\n");
        let mut d = load(&text);
        assert_eq!(render(&text, &d), text);
        mode(&mut d, "test").description = "changed".into();
        let out = render(&text, &d);
        assert!(!out.replace("\r\n", "").contains('\n'));
        assert!(out.contains("description = \"changed\"\r\n"));

        // no file: fresh
        let doc = ModesDoc {
            default_mode: "x".into(),
            modes: vec![
                ModeDto {
                    id: "x".into(),
                    next: vec!["y".into()],
                    ..Default::default()
                },
                ModeDto {
                    id: "y".into(),
                    prompt: "a \"quoted\" \"\"\"triple\"\"\" \\ back".into(),
                    ..Default::default()
                },
            ],
        };
        let out = crate::modes_edit::render(None, &doc).unwrap().text;
        assert!(out.starts_with("default_mode = \"x\"\n\n[modes.x]\n"));
        let back = parse_doc(&out).unwrap();
        assert_eq!(back.modes[1].prompt, doc.modes[1].prompt);
        // multi-line with awkward characters survives
        let mut d = load(&out);
        let tricky = "line1\n\"\"\"\n\\n [not header]\n# hash\n\ttab\n";
        mode(&mut d, "y").prompt = tricky.into();
        let out2 = render(&out, &d);
        assert_eq!(parse_doc(&out2).unwrap().modes[1].prompt, tricky);
    }

    #[test]
    fn prompt_lookalike_headers_do_not_split_sections() {
        let text = "[modes.a]\nprompt = \"\"\"\n[modes.fake]\n# x\n\"\"\"\n\n[modes.b]\n";
        let mut d = load(text);
        assert_eq!(render(text, &d), text);
        mode(&mut d, "b").description = "d".into();
        let out = render(text, &d);
        assert!(out.starts_with("[modes.a]\nprompt = \"\"\"\n[modes.fake]"));
        assert_eq!(parse_doc(&out).unwrap().modes.len(), 2);
    }

    #[test]
    fn validation_covers_the_rules() {
        let hook = |ev: &str, h: HookDto| -> BTreeMap<String, Vec<HookDto>> {
            [(ev.to_string(), vec![h])].into()
        };
        let base_hook = HookDto {
            run: "x".into(),
            glob: None,
            matcher: None,
            on_failure: "ignore".into(),
            max_fix_turns: 2,
            timeout_secs: 120,
        };
        let doc = ModesDoc {
            default_mode: "nope".into(),
            modes: vec![
                ModeDto {
                    id: "a".into(),
                    tools: Some(vec!["Read".into(), "Nope".into(), "mise__x".into()]),
                    mise_tasks: vec!["[".into()],
                    mcp_tools: vec!["github__*".into(), "mcp__a__*".into()],
                    next: vec!["ghost".into(), "a".into()],
                    auto_advance_to: vec!["b".into()],
                    profile: Some("zzz".into()),
                    permissions: [
                        ("Bash".to_string(), "maybe".to_string()),
                        ("[".to_string(), "allow".to_string()),
                    ]
                    .into(),
                    hooks: {
                        let mut h = hook(
                            "post_edit",
                            HookDto {
                                run: "  ".into(),
                                glob: Some("[".into()),
                                matcher: Some("(".into()),
                                on_failure: "boom".into(),
                                timeout_secs: 0,
                                ..base_hook.clone()
                            },
                        );
                        h.insert("nonsense".into(), vec![base_hook.clone()]);
                        h.insert(
                            "stop".into(),
                            vec![HookDto {
                                glob: Some("*.rs".into()),
                                matcher: Some("Bash".into()),
                                ..base_hook.clone()
                            }],
                        );
                        h
                    },
                    ..Default::default()
                },
                ModeDto {
                    id: "a".into(),
                    ..Default::default()
                },
                ModeDto {
                    id: "bad id".into(),
                    ..Default::default()
                },
                ModeDto {
                    id: "b".into(),
                    ..Default::default()
                },
            ],
        };
        let issues = validate_doc(&doc, Some(&["main".to_string()]));
        let has =
            |path: &str, sev: &str| issues.iter().any(|i| i.path == path && i.severity == sev);
        assert!(has("defaultMode", "error"));
        assert!(has("modes[1].id", "error"), "dup");
        assert!(has("modes[2].id", "error"), "chars");
        assert!(has("modes[0].tools[1]", "warning"));
        assert!(!issues.iter().any(|i| i.path == "modes[0].tools[2]"));
        assert!(has("modes[0].miseTasks[0]", "error"));
        assert!(has("modes[0].mcpTools[0]", "warning"));
        assert!(!issues.iter().any(|i| i.path == "modes[0].mcpTools[1]"));
        assert!(has("modes[0].next[0]", "error"));
        assert!(has("modes[0].next[1]", "warning"));
        assert!(has("modes[0].autoAdvanceTo[0]", "warning"));
        assert!(has("modes[0].profile", "warning"));
        assert!(has("modes[0].permissions.Bash", "error"));
        assert!(has("modes[0].permissions.[", "error"));
        assert!(has("modes[0].hooks.post_edit[0].run", "error"));
        assert!(has("modes[0].hooks.post_edit[0].glob", "error"));
        assert!(has("modes[0].hooks.post_edit[0].matcher", "error"));
        assert!(has("modes[0].hooks.post_edit[0].onFailure", "error"));
        assert!(has("modes[0].hooks.post_edit[0].timeoutSecs", "error"));
        assert!(has("modes[0].hooks.nonsense", "error"));
        assert!(has("modes[0].hooks.stop[0].glob", "warning"));
        assert!(has("modes[0].hooks.stop[0].matcher", "warning"));
        assert!(
            validate_doc(&ModesDoc::default(), None)
                .iter()
                .any(|i| i.path == "modes" && i.severity == "error")
        );
        let ok = load(&template_text());
        assert!(
            validate_doc(&ok, Some(&[]))
                .iter()
                .all(|i| i.severity != "error")
        );
    }

    #[test]
    fn get_preview_apply_end_to_end() {
        let proj = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let v = rt.block_on(get(proj.path(), Some(user.path())));
        assert_eq!(v.status, "missing");
        assert!(
            v.catalog
                .builtin_tools
                .iter()
                .any(|t| t.name == "Bash" && !t.read_only)
        );
        assert!(
            v.catalog
                .builtin_tools
                .iter()
                .any(|t| t.name == "Read" && t.read_only)
        );
        assert_eq!(
            v.catalog
                .templates
                .iter()
                .filter(|t| t.set == "full")
                .count(),
            6
        );
        let json = serde_json::to_value(&v).unwrap();
        assert!(json["catalog"]["builtinTools"][0]["readOnly"].is_boolean());
        assert!(json.get("defaultMode").is_none());

        // create via apply (no file yet)
        let doc = ModesDoc {
            default_mode: "a".into(),
            modes: vec![ModeDto {
                id: "a".into(),
                description: "d".into(),
                ..Default::default()
            }],
        };
        let p = preview(proj.path(), Some(user.path()), &doc).unwrap();
        assert!(p.changed && p.diff.contains("+[modes.a]"));
        assert!(!modes_path(proj.path()).exists());
        let r = apply(proj.path(), Some(user.path()), &doc).unwrap();
        assert!(r.changed && r.path.ends_with("modes.toml"));
        let v = rt.block_on(get(proj.path(), Some(user.path())));
        assert_eq!(v.status, "ready");
        assert_eq!(v.modes[0].original_id.as_deref(), Some("a"));
        // unchanged: no write, empty diff
        let again = ModesDoc {
            default_mode: v.default_mode.clone().unwrap(),
            modes: v.modes.clone(),
        };
        assert!(
            !preview(proj.path(), Some(user.path()), &again)
                .unwrap()
                .changed
        );
        assert!(
            !apply(proj.path(), Some(user.path()), &again)
                .unwrap()
                .changed
        );
        // invalid docs are refused
        let mut bad = again.clone();
        bad.modes[0].next = vec!["zzz".into()];
        let e = apply(proj.path(), Some(user.path()), &bad)
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a defined mode"), "{e}");
        // broken file
        std::fs::write(modes_path(proj.path()), "[modes.a\n").unwrap();
        let v = rt.block_on(get(proj.path(), Some(user.path())));
        assert_eq!(v.status, "broken");
        assert!(v.error.is_some());
        // lenient: dangling references still open
        std::fs::write(modes_path(proj.path()), "[modes.a]\nnext=['q']\n").unwrap();
        let v = rt.block_on(get(proj.path(), Some(user.path())));
        assert_eq!(v.status, "ready");
        // project config hooks are shown read-only
        std::fs::write(
            proj.path().join(".ez/config.toml"),
            "[[hooks.session_start]]\nrun = \"echo hi\"\n",
        )
        .unwrap();
        let v = rt.block_on(get(proj.path(), Some(user.path())));
        assert_eq!(v.global_hooks.unwrap()["session_start"][0].run, "echo hi");
    }
}
