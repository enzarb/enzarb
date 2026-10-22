//! The onboarding wizard: a pure state machine (`SetupWizard`), its renderer and the event loop
//! that drives it. All I/O (model fetch, `ez init`, writing files) happens through [`Effect`]s so
//! every transition can be tested without a terminal.

pub mod input;
pub mod render;
pub mod runner;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ez_core::config::Profile;
use ez_core::keys::{self, KeyInfo, KeyScope};
use ez_core::setup::{self, NewKey, SetupPlan, SetupReport};
use ez_core::setup_api;
use ez_core::{ModeSet, ProviderKind};
use ez_llm::ModelInfo;
use zeroize::Zeroize;

pub use self::input::TextInput;

pub const TOTAL_STEPS: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    ApiKey,
    Endpoint,
    Check,
    Models,
    ProfileName,
    Modes,
    Review,
}

impl Step {
    pub fn index(self) -> usize {
        self as usize + 1
    }

    pub fn title(self) -> &'static str {
        match self {
            Step::ApiKey => "API key",
            Step::Endpoint => "Endpoint URL",
            Step::Check => "Check compatibility",
            Step::Models => "Models",
            Step::ProfileName => "Profile name",
            Step::Modes => "Models per mode",
            Step::Review => "Review & save",
        }
    }

    fn prev(self) -> Option<Step> {
        Some(match self {
            Step::ApiKey => return None,
            Step::Endpoint => Step::ApiKey,
            Step::Check => Step::Endpoint,
            Step::Models => Step::Check,
            Step::ProfileName => Step::Models,
            Step::Modes => Step::ProfileName,
            Step::Review => Step::Modes,
        })
    }
}

/// State of the project's `.ez/modes.toml`.
pub enum ModesState {
    Missing,
    Broken(String),
    Ready(ModeSet),
}

pub struct SetupContext {
    pub project: PathBuf,
    pub user_dir: Option<PathBuf>,
    /// Profiles defined in the user/project config files.
    pub existing_profiles: Vec<Profile>,
    /// Named keys in `credentials.toml`.
    pub keys: Vec<KeyInfo>,
    pub modes: ModesState,
}

impl SetupContext {
    /// Read the current state from disk and the process environment.
    pub fn load(project: &Path, user_dir: Option<&Path>) -> Self {
        let path = project.join(".ez/modes.toml");
        let modes = match std::fs::read_to_string(&path) {
            Err(_) => ModesState::Missing,
            Ok(t) => match ModeSet::from_toml_str(&t) {
                Ok(m) => ModesState::Ready(m),
                Err(e) => ModesState::Broken(format!("{e:#}")),
            },
        };
        Self {
            project: project.to_path_buf(),
            user_dir: user_dir.map(Path::to_path_buf),
            existing_profiles: setup::configured_profiles(project, user_dir),
            keys: keys::list_keys(user_dir).unwrap_or_default(),
            modes,
        }
    }
}

/// Work the event loop must do on the wizard's behalf.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    None,
    /// Probe the endpoint with the key and detect the API flavor.
    Probe(ProbeRequest),
    /// Generate `.ez/modes.toml` (`ez init`).
    RunInit,
    /// Write everything (`setup::apply` with [`SetupWizard::plan`]).
    Apply,
}

#[derive(Clone, PartialEq)]
pub struct ProbeRequest {
    pub generation: u64,
    pub base_url: String,
    pub api_key: Option<String>,
    /// Check only this flavor (manual override); `None` auto-detects.
    pub force: Option<ProviderKind>,
}

impl std::fmt::Debug for ProbeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeRequest")
            .field("generation", &self.generation)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("force", &self.force)
            .finish()
    }
}

/// What the check step shows.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckPhase {
    Checking,
    /// A flavor answered: `message` is e.g. "Anthropic API ✓ (3 models)".
    Detected {
        message: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthChoice {
    /// Paste a key; it is saved as a named key in the per-user credentials file on save.
    New,
    Existing,
    NoKey,
}

pub const AUTH_CHOICES: [AuthChoice; 3] =
    [AuthChoice::New, AuthChoice::Existing, AuthChoice::NoKey];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListState {
    pub items: Vec<ModelInfo>,
    pub cursor: usize,
    pub filter: String,
    /// `/` focuses the filter so every character (including `a` and space) is typed into it.
    pub filter_focus: bool,
}

impl ListState {
    /// Indices of `items` matching the filter.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                f.is_empty()
                    || m.id.to_lowercase().contains(&f)
                    || m.display_name
                        .as_deref()
                        .is_some_and(|d| d.to_lowercase().contains(&f))
            })
            .map(|(i, _)| i)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelsPhase {
    Manual {
        input: TextInput,
        err: Option<String>,
    },
    List(ListState),
    Default {
        choices: Vec<String>,
        cursor: usize,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chooser {
    pub cursor: usize,
    pub filter: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModesPhase {
    NeedInit {
        cursor: usize,
        error: Option<String>,
    },
    Initing,
    Broken,
    Assign {
        row: usize,
        chooser: Option<Chooser>,
    },
}

pub type ModelChoice = (String, String);

pub struct SetupWizard {
    pub ctx: SetupContext,
    pub step: Step,
    pub error: Option<String>,
    pub tick: u64,
    pub aborted: bool,
    pub finished: Option<SetupReport>,

    // 1 API key
    pub auth_cursor: usize,
    pub existing_idx: usize,
    /// Held only in memory until the final save; zeroized on drop.
    pub new_key_secret: TextInput,
    // 2 endpoint
    pub endpoint: TextInput,
    pub endpoint_touched: bool,
    // 3 check
    pub check: CheckPhase,
    pub generation: u64,
    /// The flavor a probe detected (or the manual override).
    pub detected: Option<ProviderKind>,
    /// Manual override of the flavor.
    pub forced: Option<ProviderKind>,
    // 4 models
    pub models: ModelsPhase,
    fetched: Vec<ModelInfo>,
    pub selected: BTreeSet<String>,
    pub chosen: Vec<String>,
    pub default_model: String,
    // 5 profile name
    pub profile_name: TextInput,
    pub profile_touched: bool,
    pub confirm_overwrite: bool,
    // 6
    pub modes_phase: ModesPhase,
    pub assignments: Vec<Option<ModelChoice>>,
    assignments_ready: bool,
    pub modes_skipped: bool,
    // 7
    pub set_default: bool,
    pub preview: String,
    pub preview_scroll: usize,
    pub apply_error: Option<String>,
}

impl Drop for SetupWizard {
    fn drop(&mut self) {
        self.new_key_secret.value.zeroize();
    }
}

fn is_ctrl_c(k: &KeyEvent) -> bool {
    k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c')
}

fn is_profile_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Move a list cursor with the arrow keys (and j/k when `vi`).
fn nav(cursor: &mut usize, len: usize, key: &KeyEvent, vi: bool) -> bool {
    if len == 0 {
        return false;
    }
    match key.code {
        KeyCode::Up => *cursor = (*cursor + len - 1) % len,
        KeyCode::Down => *cursor = (*cursor + 1) % len,
        KeyCode::Char('k') if vi => *cursor = (*cursor + len - 1) % len,
        KeyCode::Char('j') if vi => *cursor = (*cursor + 1) % len,
        _ => return false,
    }
    true
}

impl SetupWizard {
    pub fn new(ctx: SetupContext) -> Self {
        Self {
            ctx,
            step: Step::ApiKey,
            error: None,
            tick: 0,
            aborted: false,
            finished: None,
            auth_cursor: 0,
            existing_idx: 0,
            new_key_secret: TextInput::default(),
            endpoint: TextInput::default(),
            endpoint_touched: false,
            check: CheckPhase::Checking,
            generation: 0,
            detected: None,
            forced: None,
            models: ModelsPhase::Manual {
                input: TextInput::default(),
                err: None,
            },
            fetched: vec![],
            selected: BTreeSet::new(),
            chosen: vec![],
            default_model: String::new(),
            profile_name: TextInput::default(),
            profile_touched: false,
            confirm_overwrite: false,
            modes_phase: ModesPhase::Initing,
            assignments: vec![],
            assignments_ready: false,
            modes_skipped: false,
            set_default: true,
            preview: String::new(),
            preview_scroll: 0,
            apply_error: None,
        }
    }

    /// The API flavor in use: the override, else the detected one, else a guess from the URL.
    pub fn kind(&self) -> ProviderKind {
        self.forced
            .or(self.detected)
            .or_else(|| setup::suggestion_for_url(self.endpoint.trimmed()).map(|s| s.kind))
            .unwrap_or(ProviderKind::Openai)
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn done(&self) -> bool {
        self.aborted || self.finished.is_some()
    }

    // ---- key handling ------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) -> Effect {
        if is_ctrl_c(&key) {
            self.aborted = true;
            return Effect::None;
        }
        self.error = None;
        match self.step {
            Step::ApiKey => self.key_auth(key),
            Step::Endpoint => self.key_endpoint(key),
            Step::Check => self.key_check(key),
            Step::ProfileName => self.key_profile_name(key),
            Step::Models => self.key_models(key),
            Step::Modes => self.key_modes(key),
            Step::Review => self.key_review(key),
        }
    }

    fn back(&mut self) {
        self.error = None;
        match self.step.prev() {
            Some(s) => {
                if s == Step::Check {
                    // Never go back into a stale or running probe: return to the URL.
                    self.generation += 1;
                    self.step = Step::Endpoint;
                    return;
                }
                if s == Step::Models {
                    self.models = if self.fetched.is_empty() {
                        ModelsPhase::Manual {
                            input: TextInput::new(self.chosen.join(", ")),
                            err: None,
                        }
                    } else {
                        ModelsPhase::List(ListState {
                            items: self.fetched.clone(),
                            ..Default::default()
                        })
                    };
                }
                self.step = s
            }
            None => self.aborted = true,
        }
    }

    pub fn auth_choice(&self) -> Option<AuthChoice> {
        AUTH_CHOICES.get(self.auth_cursor).copied()
    }

    fn key_auth(&mut self, key: KeyEvent) -> Effect {
        if nav(&mut self.auth_cursor, AUTH_CHOICES.len(), &key, false) {
            return Effect::None;
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => self.confirm_auth(),
            _ => match self.auth_choice() {
                Some(AuthChoice::New) => {
                    self.new_key_secret.handle(key);
                }
                Some(AuthChoice::Existing) => match key.code {
                    KeyCode::Left if !self.ctx.keys.is_empty() => {
                        let n = self.ctx.keys.len();
                        self.existing_idx = (self.existing_idx + n - 1) % n;
                    }
                    KeyCode::Right if !self.ctx.keys.is_empty() => {
                        self.existing_idx = (self.existing_idx + 1) % self.ctx.keys.len();
                    }
                    _ => {}
                },
                _ => {}
            },
        }
        Effect::None
    }

    fn confirm_auth(&mut self) {
        match self.auth_choice() {
            Some(AuthChoice::Existing) if self.ctx.keys.is_empty() => {
                self.error = Some("no named keys saved yet; paste a key instead".into());
                return;
            }
            Some(AuthChoice::New) if self.new_key_secret.trimmed().is_empty() => {
                self.error =
                    Some("paste your API key (or choose \"no key\" for a local server)".into());
                return;
            }
            _ => {}
        }
        self.step = Step::Endpoint;
    }

    fn key_endpoint(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => match setup::validate_endpoint(self.endpoint.trimmed()) {
                Err(e) => self.error = Some(e),
                Ok(u) => {
                    self.endpoint.set(&u);
                    self.endpoint_touched = true;
                    self.forced = None;
                    return self.start_probe();
                }
            },
            _ => {
                if self.endpoint.handle(key) {
                    self.endpoint_touched = true;
                }
            }
        }
        Effect::None
    }

    /// The key used to probe the endpoint, if any.
    pub fn fetch_key(&self) -> Option<String> {
        match self.auth_choice() {
            Some(AuthChoice::Existing) => self
                .ctx
                .keys
                .get(self.existing_idx)
                .and_then(|k| ez_core::config::named_key(self.ctx.user_dir.as_deref(), &k.name)),
            Some(AuthChoice::New) => Some(self.new_key_secret.trimmed().to_string()),
            _ => None,
        }
    }

    fn start_probe(&mut self) -> Effect {
        self.step = Step::Check;
        self.generation += 1;
        self.check = CheckPhase::Checking;
        self.detected = None;
        Effect::Probe(ProbeRequest {
            generation: self.generation,
            base_url: self.endpoint.trimmed().to_string(),
            api_key: self.fetch_key(),
            force: self.forced,
        })
    }

    /// Result of an [`Effect::Probe`]; stale generations are ignored.
    pub fn probed(&mut self, generation: u64, r: Result<setup_api::ProbeResult, String>) {
        if generation != self.generation
            || self.step != Step::Check
            || self.check != CheckPhase::Checking
        {
            return;
        }
        match r {
            Ok(res) if res.kind.is_some() => {
                self.detected = res
                    .kind
                    .as_deref()
                    .and_then(|k| setup_api::parse_kind(k).ok());
                self.fetched = res
                    .models
                    .into_iter()
                    .map(|m| ModelInfo {
                        id: m.id,
                        display_name: m.display_name,
                    })
                    .collect();
                self.check = CheckPhase::Detected {
                    message: res.message,
                };
            }
            Ok(res) => {
                self.fetched.clear();
                self.check = CheckPhase::Failed {
                    message: res.message,
                };
            }
            Err(e) => {
                self.fetched.clear();
                self.check = CheckPhase::Failed { message: e };
            }
        }
    }

    fn key_check(&mut self, key: KeyEvent) -> Effect {
        match (&self.check, key.code) {
            (_, KeyCode::Esc) => self.back(),
            (CheckPhase::Checking, _) => {}
            (_, KeyCode::Char('r')) => return self.start_probe(),
            (_, KeyCode::Char('o')) => {
                // Manual override: check only the other flavor.
                self.forced = Some(match self.forced.or(self.detected) {
                    Some(ProviderKind::Anthropic) => ProviderKind::Openai,
                    Some(_) => ProviderKind::Anthropic,
                    None => match self.kind() {
                        ProviderKind::Anthropic => ProviderKind::Openai,
                        _ => ProviderKind::Anthropic,
                    },
                });
                return self.start_probe();
            }
            (CheckPhase::Detected { .. }, KeyCode::Enter) => return self.enter_models(),
            (CheckPhase::Failed { .. }, KeyCode::Char('m')) => {
                // Continue without a model list: type the model ids by hand.
                self.detected = Some(self.kind());
                self.fetched.clear();
                return self.enter_models();
            }
            _ => {}
        }
        Effect::None
    }

    fn enter_models(&mut self) -> Effect {
        if !self.profile_touched {
            let n = setup::suggested_profile_name(self.kind(), self.endpoint.trimmed());
            self.profile_name.set(&n);
        }
        self.step = Step::Models;
        self.models = if self.fetched.is_empty() {
            ModelsPhase::Manual {
                input: TextInput::new(self.chosen.join(", ")),
                err: None,
            }
        } else {
            ModelsPhase::List(ListState {
                items: self.fetched.clone(),
                ..Default::default()
            })
        };
        Effect::None
    }

    fn enter_modes_effect(&mut self) -> Effect {
        self.enter_modes();
        Effect::None
    }

    fn profile_exists(&self, name: &str) -> bool {
        self.ctx.existing_profiles.iter().any(|p| p.name == name)
    }

    fn key_profile_name(&mut self, key: KeyEvent) -> Effect {
        if self.confirm_overwrite {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    self.confirm_overwrite = false;
                    return self.enter_modes_effect();
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm_overwrite = false;
                }
                _ => {}
            }
            return Effect::None;
        }
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Enter => {
                let n = self.profile_name.trimmed().to_string();
                if !is_profile_name(&n) {
                    self.error =
                        Some("use letters, digits, `-` or `_` for the profile name".into());
                } else if self.profile_exists(&n) {
                    self.confirm_overwrite = true;
                } else {
                    return self.enter_modes_effect();
                }
            }
            _ => {
                if self.profile_name.handle(key) {
                    self.profile_touched = true;
                }
            }
        }
        Effect::None
    }

    // ---- models ----

    fn key_models(&mut self, key: KeyEvent) -> Effect {
        match self.models.clone() {
            ModelsPhase::Manual { mut input, .. } => {
                match key.code {
                    KeyCode::Esc => {
                        if self.fetched.is_empty() {
                            self.step = Step::Check;
                            self.back();
                        } else {
                            self.models = ModelsPhase::List(ListState {
                                items: self.fetched.clone(),
                                ..Default::default()
                            });
                        }
                    }
                    KeyCode::Enter => {
                        let ids = parse_model_ids(&input.value);
                        if ids.is_empty() {
                            self.models = ModelsPhase::Manual {
                                input,
                                err: Some("enter at least one model id".into()),
                            };
                        } else {
                            self.selected = ids.iter().cloned().collect();
                            self.finish_models(ids);
                        }
                    }
                    _ => {
                        input.handle(key);
                        self.models = ModelsPhase::Manual { input, err: None };
                    }
                }
                Effect::None
            }
            ModelsPhase::List(st) => self.key_model_list(st, key),
            ModelsPhase::Default {
                choices,
                mut cursor,
            } => {
                if nav(&mut cursor, choices.len(), &key, true) {
                    self.models = ModelsPhase::Default { choices, cursor };
                    return Effect::None;
                }
                match key.code {
                    KeyCode::Esc => {
                        self.models = if self.fetched.is_empty() {
                            ModelsPhase::Manual {
                                input: TextInput::new(self.chosen.join(", ")),
                                err: None,
                            }
                        } else {
                            ModelsPhase::List(ListState {
                                items: self.fetched.clone(),
                                ..Default::default()
                            })
                        };
                    }
                    KeyCode::Enter => {
                        self.default_model = choices[cursor].clone();
                        self.step = Step::ProfileName;
                        self.confirm_overwrite = false;
                    }
                    _ => {}
                }
                Effect::None
            }
        }
    }

    fn key_model_list(&mut self, mut st: ListState, key: KeyEvent) -> Effect {
        let vis = st.visible();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if st.filter_focus {
                    st.filter_focus = false;
                } else if !st.filter.is_empty() {
                    st.filter.clear();
                    st.cursor = 0;
                } else {
                    self.back();
                    return Effect::None;
                }
            }
            KeyCode::Enter => {
                if st.filter_focus {
                    st.filter_focus = false;
                } else if self.selected.is_empty() {
                    self.error = Some("select at least one model with Space".into());
                } else {
                    let ids: Vec<String> = st
                        .items
                        .iter()
                        .filter(|m| self.selected.contains(&m.id))
                        .map(|m| m.id.clone())
                        .collect();
                    self.models = ModelsPhase::List(st);
                    self.finish_models(ids);
                    return Effect::None;
                }
            }
            KeyCode::Up | KeyCode::Down => {
                nav(&mut st.cursor, vis.len(), &key, false);
            }
            KeyCode::PageUp => st.cursor = st.cursor.saturating_sub(10),
            KeyCode::PageDown => st.cursor = (st.cursor + 10).min(vis.len().saturating_sub(1)),
            KeyCode::Char('a') if ctrl => self.toggle_all(&st),
            KeyCode::Char(' ') if !st.filter_focus => {
                if let Some(&i) = vis.get(st.cursor) {
                    let id = st.items[i].id.clone();
                    if !self.selected.remove(&id) {
                        self.selected.insert(id);
                    }
                }
            }
            KeyCode::Char('/') if !st.filter_focus && st.filter.is_empty() => {
                st.filter_focus = true;
            }
            KeyCode::Char('a') if !st.filter_focus && st.filter.is_empty() => self.toggle_all(&st),
            KeyCode::Char(c) if !ctrl => {
                st.filter.push(c);
                st.cursor = 0;
            }
            KeyCode::Backspace => {
                st.filter.pop();
                st.cursor = 0;
            }
            _ => {}
        }
        let n = st.visible().len();
        if st.cursor >= n {
            st.cursor = n.saturating_sub(1);
        }
        self.models = ModelsPhase::List(st);
        Effect::None
    }

    /// Select every visible model, or deselect them if all are already selected.
    fn toggle_all(&mut self, st: &ListState) {
        let ids: Vec<String> = st
            .visible()
            .iter()
            .map(|&i| st.items[i].id.clone())
            .collect();
        if ids.iter().all(|i| self.selected.contains(i)) {
            for i in ids {
                self.selected.remove(&i);
            }
        } else {
            self.selected.extend(ids);
        }
    }

    fn finish_models(&mut self, ids: Vec<String>) {
        self.chosen = ids.clone();
        if ids.len() == 1 {
            self.default_model = ids[0].clone();
            self.step = Step::ProfileName;
            self.confirm_overwrite = false;
            return;
        }
        let existing = self
            .ctx
            .existing_profiles
            .iter()
            .find(|p| p.name == self.profile_name.trimmed())
            .map(|p| p.model.clone());
        let cursor = existing
            .and_then(|m| ids.iter().position(|i| *i == m))
            .unwrap_or(0);
        self.models = ModelsPhase::Default {
            choices: ids,
            cursor,
        };
    }

    // ---- per-mode assignment ---------------------------------------------------------------

    pub fn mode_set(&self) -> Option<&ModeSet> {
        match &self.ctx.modes {
            ModesState::Ready(m) => Some(m),
            _ => None,
        }
    }

    /// Every (profile, model) a mode may use: the new profile's models, then other profiles'.
    pub fn model_options(&self) -> Vec<ModelChoice> {
        let name = self.profile_name.trimmed().to_string();
        let mut out: Vec<ModelChoice> = vec![];
        let mut push = |p: &str, m: &str| {
            let c = (p.to_string(), m.to_string());
            if !m.is_empty() && !out.contains(&c) {
                out.push(c);
            }
        };
        for m in &self.chosen {
            push(&name, m);
        }
        for p in self.ctx.existing_profiles.iter().filter(|p| p.name != name) {
            push(&p.name, &p.model);
            for m in &p.models {
                push(&p.name, m);
            }
        }
        // Keep assignments that already exist even if their profile isn't listed above.
        for a in self.assignments.iter().flatten() {
            push(&a.0, &a.1);
        }
        out
    }

    fn enter_modes(&mut self) {
        self.step = Step::Modes;
        match &self.ctx.modes {
            ModesState::Missing => {
                if !matches!(self.modes_phase, ModesPhase::NeedInit { .. }) {
                    self.modes_phase = ModesPhase::NeedInit {
                        cursor: 0,
                        error: None,
                    };
                }
            }
            ModesState::Broken(_) => self.modes_phase = ModesPhase::Broken,
            ModesState::Ready(set) => {
                let n = set.modes.len();
                if !self.assignments_ready {
                    self.assignments = set
                        .modes
                        .iter()
                        .map(|(_, m)| self.existing_assignment(m))
                        .collect();
                    self.assignments_ready = true;
                }
                self.assignments.resize(n, None);
                // Drop assignments to models that were just deselected from this profile.
                let name = self.profile_name.trimmed().to_string();
                let chosen = self.chosen.clone();
                for a in &mut self.assignments {
                    if a.as_ref()
                        .is_some_and(|(p, m)| *p == name && !chosen.contains(m))
                    {
                        *a = None;
                    }
                }
                self.modes_skipped = false;
                if !matches!(self.modes_phase, ModesPhase::Assign { .. }) {
                    self.modes_phase = ModesPhase::Assign {
                        row: 0,
                        chooser: None,
                    };
                }
            }
        }
    }

    fn existing_assignment(&self, m: &ez_core::ModeDef) -> Option<ModelChoice> {
        let profile = m.profile.clone()?;
        let model = m.model.clone().or_else(|| {
            self.ctx
                .existing_profiles
                .iter()
                .find(|p| p.name == profile)
                .map(|p| p.model.clone())
        })?;
        Some((profile, model))
    }

    /// Items of the chooser: "profile default" followed by the options matching `filter`.
    pub fn chooser_items(&self, filter: &str) -> Vec<Option<ModelChoice>> {
        let f = filter.to_lowercase();
        let mut v = vec![None];
        v.extend(
            self.model_options()
                .into_iter()
                .filter(|(p, m)| f.is_empty() || format!("{p}/{m}").to_lowercase().contains(&f))
                .map(Some),
        );
        v
    }

    /// Result of [`Effect::RunInit`].
    pub fn modes_initialized(&mut self, r: Result<ModeSet, String>) {
        if self.step != Step::Modes || self.modes_phase != ModesPhase::Initing {
            return;
        }
        match r {
            Ok(set) => {
                self.ctx.modes = ModesState::Ready(set);
                self.modes_phase = ModesPhase::Assign {
                    row: 0,
                    chooser: None,
                };
                self.assignments_ready = false;
                self.enter_modes();
            }
            Err(e) => {
                self.modes_phase = ModesPhase::NeedInit {
                    cursor: 0,
                    error: Some(e),
                }
            }
        }
    }

    fn key_modes(&mut self, key: KeyEvent) -> Effect {
        match self.modes_phase.clone() {
            ModesPhase::Initing => {
                if key.code == KeyCode::Esc {
                    self.back();
                }
                Effect::None
            }
            ModesPhase::NeedInit { mut cursor, error } => {
                if nav(&mut cursor, 2, &key, true) {
                    self.modes_phase = ModesPhase::NeedInit { cursor, error };
                    return Effect::None;
                }
                match key.code {
                    KeyCode::Esc => self.back(),
                    KeyCode::Enter if cursor == 0 => {
                        self.modes_phase = ModesPhase::Initing;
                        return Effect::RunInit;
                    }
                    KeyCode::Enter => {
                        self.modes_skipped = true;
                        return self.enter_review();
                    }
                    _ => {}
                }
                Effect::None
            }
            ModesPhase::Broken => {
                match key.code {
                    KeyCode::Esc => self.back(),
                    KeyCode::Enter => {
                        self.modes_skipped = true;
                        return self.enter_review();
                    }
                    _ => {}
                }
                Effect::None
            }
            ModesPhase::Assign { mut row, chooser } => {
                let n = self.mode_set().map_or(0, |m| m.modes.len());
                match chooser {
                    Some(ch) => self.key_chooser(row, ch, key),
                    None => {
                        if nav(&mut row, n + 1, &key, true) {
                            self.modes_phase = ModesPhase::Assign { row, chooser: None };
                            return Effect::None;
                        }
                        match key.code {
                            KeyCode::Esc => self.back(),
                            KeyCode::Enter if row == n => return self.enter_review(),
                            KeyCode::Enter => {
                                let cur = self.assignments.get(row).cloned().flatten();
                                let items = self.chooser_items("");
                                let cursor = items.iter().position(|i| *i == cur).unwrap_or(0);
                                self.modes_phase = ModesPhase::Assign {
                                    row,
                                    chooser: Some(Chooser {
                                        cursor,
                                        filter: String::new(),
                                    }),
                                };
                            }
                            KeyCode::Char('c') => return self.enter_review(),
                            _ => {}
                        }
                        Effect::None
                    }
                }
            }
        }
    }

    fn key_chooser(&mut self, row: usize, mut ch: Chooser, key: KeyEvent) -> Effect {
        let items = self.chooser_items(&ch.filter);
        match key.code {
            KeyCode::Esc => {
                self.modes_phase = ModesPhase::Assign { row, chooser: None };
                return Effect::None;
            }
            KeyCode::Enter => {
                if let Some(choice) = items.get(ch.cursor)
                    && let Some(slot) = self.assignments.get_mut(row)
                {
                    *slot = choice.clone();
                }
                self.modes_phase = ModesPhase::Assign { row, chooser: None };
                return Effect::None;
            }
            KeyCode::Up | KeyCode::Down => {
                nav(&mut ch.cursor, items.len(), &key, false);
            }
            KeyCode::PageUp => ch.cursor = ch.cursor.saturating_sub(10),
            KeyCode::PageDown => ch.cursor = (ch.cursor + 10).min(items.len() - 1),
            KeyCode::Backspace => {
                ch.filter.pop();
                ch.cursor = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                ch.filter.push(c);
                ch.cursor = 0;
            }
            _ => {}
        }
        self.modes_phase = ModesPhase::Assign {
            row,
            chooser: Some(ch),
        };
        Effect::None
    }

    // ---- review ----------------------------------------------------------------------------

    /// The plan the wizard currently describes.
    pub fn plan(&self) -> SetupPlan {
        let kind = self.kind();
        let name = self.profile_name.trimmed().to_string();
        let (mut api_key_name, mut new_key) = (None, None);
        match self.auth_choice() {
            Some(AuthChoice::Existing) => {
                api_key_name = self.ctx.keys.get(self.existing_idx).map(|k| k.name.clone())
            }
            Some(AuthChoice::New) => {
                let n = self.new_key_name();
                api_key_name = Some(n.clone());
                new_key = Some(NewKey {
                    name: n,
                    secret: self.new_key_secret.trimmed().to_string(),
                    description: Some(format!("{name} key")),
                });
            }
            _ => {}
        }
        let mode_models = match self.mode_set() {
            Some(set) if !self.modes_skipped => set
                .modes
                .iter()
                .enumerate()
                .map(|(i, (id, _))| (id.clone(), self.assignments.get(i).cloned().flatten()))
                .collect(),
            _ => vec![],
        };
        SetupPlan {
            profile_name: name.clone(),
            profile: Profile {
                name,
                provider: kind,
                model: self.default_model.clone(),
                models: self.chosen.clone(),
                api_key_name,
                api_key_env: None,
                base_url: setup::stored_base_url(kind, self.endpoint.trimmed()),
                max_tokens: 32_000,
                context_window: 200_000,
                thinking: None,
            },
            set_default: self.set_default,
            new_key,
            key_scope: KeyScope::User,
            mode_models,
        }
    }

    /// Name for a pasted key: the profile name, made unique among the saved keys.
    pub fn new_key_name(&self) -> String {
        let base = self.profile_name.trimmed().to_string();
        let taken = |n: &str| self.ctx.keys.iter().any(|k| k.name == n);
        let mut name = base.clone();
        let mut i = 2;
        while taken(&name) {
            name = format!("{base}-{i}");
            i += 1;
        }
        name
    }

    fn enter_review(&mut self) -> Effect {
        self.step = Step::Review;
        self.apply_error = None;
        self.preview_scroll = 0;
        self.refresh_preview();
        Effect::None
    }

    fn refresh_preview(&mut self) {
        self.preview = match setup::preview(
            &self.plan(),
            self.ctx.user_dir.as_deref(),
            &self.ctx.project,
        ) {
            Ok(p) => p,
            Err(e) => format!("cannot preview: {e:#}"),
        };
    }

    fn key_review(&mut self, key: KeyEvent) -> Effect {
        match key.code {
            KeyCode::Esc => self.back(),
            KeyCode::Up => self.preview_scroll = self.preview_scroll.saturating_sub(1),
            KeyCode::Down => self.preview_scroll += 1,
            KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(10),
            KeyCode::PageDown => self.preview_scroll += 10,
            KeyCode::Char('d') => {
                self.set_default = !self.set_default;
                self.refresh_preview();
            }
            KeyCode::Enter => return Effect::Apply,
            _ => {}
        }
        Effect::None
    }

    /// Result of [`Effect::Apply`].
    pub fn applied(&mut self, r: Result<SetupReport, String>) {
        match r {
            Ok(report) => self.finished = Some(report),
            Err(e) => self.apply_error = Some(e),
        }
    }
}

/// Comma/whitespace-separated model ids, de-duplicated in order.
pub fn parse_model_ids(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in s
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests;
