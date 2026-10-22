//! Machine-readable face of the setup and keys features, shared by `ez ... --json` (used inside
//! WSL by the desktop app) and the desktop app's local Tauri commands, so both produce exactly the
//! same JSON (camelCase). Secrets only ever flow *in* (`NewKeyInput`, [`ModelsAuth::Secret`]); they
//! are zeroized on drop and no *output* type here contains one (the input types serialize only so
//! the desktop app can forward a plan to `ez` over stdin).

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::ModeSet;
use crate::config::{Profile, ProviderKind};
use crate::init::{self, InitOptions};
use crate::keys::{self, KeyScope};
use crate::setup::{self, NewKey, SetupPlan, SetupReport};

/// A quick-fill endpoint (`id` is also the suggested profile name).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointSuggestionInfo {
    pub id: String,
    pub label: String,
    pub url: String,
    /// `anthropic` or `openai` (OpenAI-compatible): the API flavor this endpoint speaks.
    pub kind: String,
}

/// The quick-fill endpoints, in display order (`ez setup`'s URL step).
pub fn suggestions() -> Vec<EndpointSuggestionInfo> {
    setup::ENDPOINT_SUGGESTIONS
        .iter()
        .map(|s| EndpointSuggestionInfo {
            id: s.id.into(),
            label: s.label.into(),
            url: s.url.into(),
            kind: s.kind.as_str().into(),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInfo {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub models: Vec<String>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key_name: Option<String>,
}

/// A named key as shown to users: never the secret.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyView {
    pub name: String,
    pub description: Option<String>,
    pub created_at: Option<String>,
    /// `…` plus the last four characters (just `…` for short keys).
    pub masked: String,
    /// `profile `p` (user)` / `profile `p` (this project)` for every reference.
    pub used_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MissingRef {
    pub name: String,
    pub used_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeysList {
    pub keys: Vec<KeyView>,
    /// References to keys that are not in credentials.toml.
    pub missing: Vec<MissingRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModeInfo {
    pub id: String,
    pub description: String,
    pub profile: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ModesState {
    Missing,
    Broken {
        error: String,
    },
    #[serde(rename_all = "camelCase")]
    Ready {
        default_mode: String,
        modes: Vec<ModeInfo>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SetupState {
    /// No profile with a stored key (or keyless local endpoint): open the wizard.
    pub needs_onboarding: bool,
    pub project: String,
    pub default_profile: String,
    pub profiles: Vec<ProfileInfo>,
    pub keys: KeysList,
    pub modes: ModesState,
}

pub fn modes_state(project: &Path) -> ModesState {
    match std::fs::read_to_string(project.join(".ez").join("modes.toml")) {
        Err(_) => ModesState::Missing,
        Ok(t) => match ModeSet::from_toml_str(&t) {
            Err(e) => ModesState::Broken {
                error: format!("{e:#}"),
            },
            Ok(m) => ModesState::Ready {
                default_mode: m.default_mode.clone(),
                modes: m
                    .modes
                    .iter()
                    .map(|(id, d)| ModeInfo {
                        id: id.clone(),
                        description: d.description.clone(),
                        profile: d.profile.clone(),
                        model: d.model.clone(),
                    })
                    .collect(),
            },
        },
    }
}

pub fn keys_list(user_dir: Option<&Path>, project: &Path) -> Result<KeysList> {
    let list = keys::list_keys(user_dir)?;
    let refs = keys::key_references(user_dir, project);
    let views = list
        .iter()
        .map(|k| KeyView {
            name: k.name.clone(),
            description: k.description.clone(),
            created_at: k.created_at.clone(),
            masked: k.masked.clone(),
            used_by: refs
                .iter()
                .filter(|(n, _)| *n == k.name)
                .map(|(_, w)| w.clone())
                .collect(),
        })
        .collect();
    let missing = refs
        .iter()
        .filter(|(n, _)| !list.iter().any(|k| k.name == *n))
        .map(|(n, w)| MissingRef {
            name: n.clone(),
            used_by: w.clone(),
        })
        .collect();
    Ok(KeysList {
        keys: views,
        missing,
    })
}

/// Everything the wizard and the keys tab need.
pub fn state(project: &Path, user_dir: Option<&Path>) -> SetupState {
    let profiles: Vec<Profile> = setup::configured_profiles(project, user_dir);
    let default_profile = crate::Config::load(project, user_dir)
        .map(|c| c.default_profile)
        .unwrap_or_default();
    SetupState {
        needs_onboarding: setup::needs_onboarding(project, user_dir),
        project: project.display().to_string(),
        default_profile,
        profiles: profiles
            .into_iter()
            .map(|p| ProfileInfo {
                name: p.name,
                provider: p.provider.as_str().into(),
                model: p.model,
                models: p.models,
                base_url: p.base_url,
                api_key_env: p.api_key_env,
                api_key_name: p.api_key_name,
            })
            .collect(),
        keys: keys_list(user_dir, project).unwrap_or(KeysList {
            keys: vec![],
            missing: vec![],
        }),
        modes: modes_state(project),
    }
}

// ----- plan -----

/// Serializable only so a plan can be forwarded to `ez setup --plan-stdin` (stdin, never argv).
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewKeyInput {
    pub name: String,
    pub secret: String,
    #[serde(default)]
    pub description: Option<String>,
}

impl Drop for NewKeyInput {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl std::fmt::Debug for NewKeyInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewKeyInput")
            .field("name", &self.name)
            .field("secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModeModelInput {
    pub mode: String,
    /// `null`/absent model means "profile default" (clears `profile` and `model` of the mode).
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// The wizard's result as JSON (input of `setup_preview`/`setup_apply`/`ez setup --plan-stdin`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanInput {
    pub profile_name: String,
    /// `anthropic` or `openai`.
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub api_key_name: Option<String>,
    #[serde(default)]
    pub new_key: Option<NewKeyInput>,
    /// `user` (default) or `project`.
    #[serde(default)]
    pub key_scope: Option<String>,
    #[serde(default)]
    pub set_default: bool,
    #[serde(default)]
    pub mode_models: Vec<ModeModelInput>,
}

pub fn parse_kind(s: &str) -> Result<ProviderKind> {
    match s {
        "anthropic" => Ok(ProviderKind::Anthropic),
        "openai" => Ok(ProviderKind::Openai),
        o => bail!("unknown provider kind `{o}`: use anthropic or openai"),
    }
}

impl PlanInput {
    /// Validate and convert. The new key's secret is moved out of `self` (and wiped from it).
    pub fn into_plan(mut self) -> Result<SetupPlan> {
        let kind = parse_kind(&self.provider)?;
        let name = self.profile_name.trim().to_string();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            bail!("invalid profile name `{name}`: use letters, digits, `-`, `_` or `.`");
        }
        if self.model.trim().is_empty() {
            bail!("a default model is required");
        }
        let mut models = self.models.clone();
        if !models.contains(&self.model) {
            models.insert(0, self.model.clone());
        }
        let scope = match self.key_scope.as_deref() {
            None | Some("user") => KeyScope::User,
            Some("project") => KeyScope::Project,
            Some(o) => bail!("unknown key scope `{o}`: use user or project"),
        };
        let base_url = setup::validate_endpoint(self.base_url.as_deref().unwrap_or(""))
            .map_err(anyhow::Error::msg)?;
        let mut api_key_name = self.api_key_name.clone();
        let new_key = match self.new_key.take() {
            Some(k) => {
                keys::validate_key_name(&k.name)?;
                if k.secret.trim().is_empty() {
                    bail!("the new key is empty");
                }
                api_key_name = Some(k.name.clone());
                Some(NewKey {
                    name: k.name.clone(),
                    secret: k.secret.trim().to_string(),
                    description: k.description.clone(),
                })
            }
            None => None,
        };
        if let Some(n) = &api_key_name {
            keys::validate_key_name(n)?;
        }
        Ok(SetupPlan {
            profile_name: name.clone(),
            profile: Profile {
                name,
                provider: kind,
                model: self.model.clone(),
                models,
                api_key_name,
                api_key_env: None,
                base_url: setup::stored_base_url(kind, &base_url),
                max_tokens: 32_000,
                context_window: 200_000,
                thinking: None,
            },
            set_default: self.set_default,
            new_key,
            key_scope: scope,
            mode_models: self
                .mode_models
                .iter()
                .map(|m| {
                    let choice = match (&m.profile, &m.model) {
                        (Some(p), Some(model)) => Some((p.clone(), model.clone())),
                        _ => None,
                    };
                    (m.mode.clone(), choice)
                })
                .collect(),
        })
    }
}

/// Wipes the secret of a plan once it is no longer needed.
pub fn wipe_plan(plan: &mut SetupPlan) {
    if let Some(k) = plan.new_key.as_mut() {
        k.secret.zeroize();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReportView {
    pub config_path: String,
    pub project_config_path: Option<String>,
    pub credentials_path: Option<String>,
    pub modes_path: Option<String>,
    pub modes_updated: usize,
}

impl From<&SetupReport> for ReportView {
    fn from(r: &SetupReport) -> Self {
        let s = |p: &std::path::PathBuf| p.display().to_string();
        Self {
            config_path: s(&r.config_path),
            project_config_path: r.project_config_path.as_ref().map(s),
            credentials_path: r.credentials_path.as_ref().map(s),
            modes_path: r.modes_path.as_ref().map(s),
            modes_updated: r.modes_updated,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    pub diff: String,
}

pub fn preview(input: PlanInput, user_dir: Option<&Path>, project: &Path) -> Result<PreviewView> {
    let mut plan = input.into_plan()?;
    let r = setup::preview(&plan, user_dir, project);
    wipe_plan(&mut plan);
    Ok(PreviewView { diff: r? })
}

pub fn apply(input: PlanInput, user_dir: Option<&Path>, project: &Path) -> Result<ReportView> {
    let mut plan = input.into_plan()?;
    let r = setup::apply(&plan, user_dir, project);
    wipe_plan(&mut plan);
    Ok(ReportView::from(&r?))
}

// ----- models -----

/// How `list_models` authenticates. A `Secret` is for a key that is not saved yet.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelsAuth {
    #[serde(alias = "key_name")]
    KeyName(String),
    Secret(String),
}

impl Drop for ModelsAuth {
    fn drop(&mut self) {
        if let ModelsAuth::Secret(s) = self {
            s.zeroize();
        }
    }
}

impl std::fmt::Debug for ModelsAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyName(n) => write!(f, "KeyName({n})"),
            Self::Secret(_) => write!(f, "Secret(<redacted>)"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    pub display_name: Option<String>,
}

/// Resolve the secret for `auth` (`None` means no key).
pub fn resolve_models_auth(
    auth: Option<&ModelsAuth>,
    user_dir: Option<&Path>,
) -> Result<Option<String>> {
    Ok(match auth {
        None => None,
        Some(ModelsAuth::KeyName(n)) => Some(
            crate::config::named_key(user_dir, n)
                .with_context(|| format!("named key `{n}` not found"))?,
        ),
        Some(ModelsAuth::Secret(s)) => Some(s.trim().to_string()),
    })
}

pub async fn list_models(
    kind: &str,
    base_url: &str,
    auth: Option<ModelsAuth>,
    user_dir: Option<&Path>,
) -> Result<Vec<ModelView>> {
    let kind_p = parse_kind(kind)?;
    let api = match kind_p {
        ProviderKind::Anthropic => ez_llm::ApiKind::Anthropic,
        _ => ez_llm::ApiKind::Openai,
    };
    let base = if base_url.trim().is_empty() {
        setup::default_base_url(kind_p).to_string()
    } else {
        base_url.trim().to_string()
    };
    let mut key = resolve_models_auth(auth.as_ref(), user_dir)?;
    let r = ez_llm::list_models(api, &base, key.as_deref()).await;
    if let Some(k) = key.as_mut() {
        k.zeroize();
    }
    Ok(r?
        .into_iter()
        .map(|m| ModelView {
            id: m.id,
            display_name: m.display_name,
        })
        .collect())
}

// ----- probe -----

/// One request the probe made.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeAttempt {
    /// `anthropic` or `openai`.
    pub kind: String,
    pub url: String,
    /// HTTP status when the server answered.
    pub status: Option<u16>,
    /// `null` on success.
    pub error: Option<String>,
}

/// Result of [`probe`]: the detected API flavor (if any), its models and a human explanation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// `anthropic` / `openai`, or `null` when no probe succeeded.
    pub kind: Option<String>,
    pub models: Vec<ModelView>,
    pub attempts: Vec<ProbeAttempt>,
    /// One line for the user: `Anthropic API ✓ (3 models)` or what went wrong and how to fix it.
    pub message: String,
}

fn kind_label(kind: &str) -> &'static str {
    if kind == "anthropic" {
        "Anthropic API"
    } else {
        "OpenAI-compatible API"
    }
}

fn status_of(err: &str) -> Option<u16> {
    let rest = err.split("HTTP ").nth(1)?;
    rest.get(..3)?.parse().ok()
}

/// Explain a failed probe (`attempts` all failed).
fn failure_message(attempts: &[ProbeAttempt]) -> String {
    let any = |f: &dyn Fn(u16) -> bool| attempts.iter().any(|a| a.status.is_some_and(f));
    if any(&|s| s == 401 || s == 403) {
        let s = attempts
            .iter()
            .find_map(|a| a.status.filter(|s| *s == 401 || *s == 403))
            .unwrap_or(401);
        return format!("The server rejected the API key (HTTP {s}). Check the key and try again.");
    }
    if any(&|s| s == 404) {
        return "Neither API answered at this URL (HTTP 404). Try adding or removing a `/v1` suffix, e.g. https://host/v1.".into();
    }
    if let Some(a) = attempts.iter().find(|a| a.status.is_some()) {
        return format!(
            "The server answered with an error: {}",
            a.error.as_deref().unwrap_or("unexpected response")
        );
    }
    let e = attempts
        .iter()
        .filter_map(|a| a.error.as_deref())
        .next()
        .unwrap_or("");
    let l = e.to_lowercase();
    if l.contains("certificate") || l.contains("tls") || l.contains("ssl") {
        format!("TLS error talking to the server: {e}")
    } else if l.contains("dns") || l.contains("resolve") || l.contains("lookup") {
        format!("Could not resolve the host name: {e}")
    } else if l.contains("refused") || l.contains("connect") {
        format!("Could not connect (is the server running?): {e}")
    } else {
        format!("The request failed: {e}")
    }
}

/// Probe `base_url` with `auth` and detect which API it speaks: Anthropic Messages
/// (`GET {base}/v1/models`) or OpenAI-compatible (`GET {base}/models`). The probes are ordered by
/// hints (an Anthropic host or an `sk-ant-` key goes first). `force` (`anthropic`/`openai`) skips
/// detection and checks only that flavor.
pub async fn probe(
    base_url: &str,
    auth: Option<ModelsAuth>,
    user_dir: Option<&Path>,
    force: Option<&str>,
) -> Result<ProbeResult> {
    let base = setup::validate_endpoint(base_url).map_err(anyhow::Error::msg)?;
    let mut key = resolve_models_auth(auth.as_ref(), user_dir)?;
    let order: Vec<ProviderKind> = match force {
        Some(k) => vec![parse_kind(k)?],
        None => {
            let anthropic_first = key.as_deref().is_some_and(|k| k.starts_with("sk-ant-"))
                || setup::suggestion_for_url(&base)
                    .is_some_and(|s| s.kind == ProviderKind::Anthropic)
                || base.contains("anthropic");
            if anthropic_first {
                vec![ProviderKind::Anthropic, ProviderKind::Openai]
            } else {
                vec![ProviderKind::Openai, ProviderKind::Anthropic]
            }
        }
    };
    let mut attempts = vec![];
    let mut found: Option<(String, Vec<ModelView>)> = None;
    for kind in order {
        let ks = kind.as_str().to_string();
        let (api, url) = match kind {
            ProviderKind::Anthropic => (ez_llm::ApiKind::Anthropic, format!("{base}/v1/models")),
            _ => (ez_llm::ApiKind::Openai, format!("{base}/models")),
        };
        match ez_llm::list_models(api, &base, key.as_deref()).await {
            Ok(models) => {
                attempts.push(ProbeAttempt {
                    kind: ks.clone(),
                    url,
                    status: Some(200),
                    error: None,
                });
                found = Some((
                    ks,
                    models
                        .into_iter()
                        .map(|m| ModelView {
                            id: m.id,
                            display_name: m.display_name,
                        })
                        .collect(),
                ));
                break;
            }
            Err(e) => {
                let text = format!("{e:#}");
                attempts.push(ProbeAttempt {
                    kind: ks,
                    url,
                    status: status_of(&text),
                    error: Some(text),
                });
            }
        }
    }
    if let Some(k) = key.as_mut() {
        k.zeroize();
    }
    Ok(match found {
        Some((kind, models)) => {
            let n = models.len();
            ProbeResult {
                message: format!(
                    "{} ✓ ({n} model{})",
                    kind_label(&kind),
                    if n == 1 { "" } else { "s" }
                ),
                kind: Some(kind),
                models,
                attempts,
            }
        }
        None => ProbeResult {
            message: failure_message(&attempts),
            kind: None,
            models: vec![],
            attempts,
        },
    })
}

// ----- init -----

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitInput {
    #[serde(default)]
    pub minimal: bool,
    #[serde(default)]
    pub modes: Vec<String>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InitResult {
    pub path: String,
    pub tasks_discovered: usize,
    pub modes: ModesState,
}

/// `ez init` (including mise task discovery). Refuses to overwrite unless `force`.
pub async fn init_modes(project: &Path, opts: &InitInput) -> Result<InitResult> {
    let tasks = ez_mise::discover(project).await;
    let generated = init::generate(
        &tasks,
        &InitOptions {
            minimal: opts.minimal,
            only: (!opts.modes.is_empty()).then(|| opts.modes.clone()),
        },
    )?;
    let path = project.join(".ez").join("modes.toml");
    if path.exists() && !opts.force {
        bail!(
            "{} already exists; use force to overwrite it",
            path.display()
        );
    }
    std::fs::create_dir_all(path.parent().expect("has parent"))?;
    std::fs::write(&path, generated)?;
    Ok(InitResult {
        path: path.display().to_string(),
        tasks_discovered: tasks.len(),
        modes: modes_state(project),
    })
}

// ----- key operations with JSON results -----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyAdded {
    pub name: String,
    pub path: String,
}

pub fn keys_add(
    user_dir: Option<&Path>,
    name: &str,
    secret: &str,
    description: Option<&str>,
) -> Result<KeyAdded> {
    let p = keys::add_key(user_dir, name, secret, description)?;
    Ok(KeyAdded {
        name: name.into(),
        path: p.display().to_string(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyRemoved {
    pub name: String,
    /// Profiles that still reference the removed key.
    pub still_used_by: Vec<String>,
}

pub fn keys_remove(user_dir: Option<&Path>, project: &Path, name: &str) -> Result<KeyRemoved> {
    let refs: Vec<String> = keys::key_references(user_dir, project)
        .into_iter()
        .filter(|(n, _)| n == name)
        .map(|(_, w)| w)
        .collect();
    if !keys::remove_key(user_dir, name)? {
        bail!("no key named `{name}`");
    }
    Ok(KeyRemoved {
        name: name.into(),
        still_used_by: refs,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyUsed {
    pub name: String,
    pub profile: String,
    pub scope: String,
    pub path: String,
}

pub fn keys_use(
    user_dir: Option<&Path>,
    project: &Path,
    name: &str,
    profile: Option<&str>,
    project_scope: bool,
) -> Result<KeyUsed> {
    if !keys::list_keys(user_dir)?.iter().any(|k| k.name == name) {
        bail!("no key named `{name}`; add it with `ez keys add {name}`");
    }
    let profile = match profile {
        Some(p) => p.to_string(),
        None => {
            crate::Config::load(project, user_dir)
                .context("loading config")?
                .default_profile
        }
    };
    let scope = if project_scope {
        KeyScope::Project
    } else {
        KeyScope::User
    };
    let p = keys::use_key(name, &profile, scope, user_dir, project)?;
    Ok(KeyUsed {
        name: name.into(),
        profile,
        scope: if project_scope { "project" } else { "user" }.into(),
        path: p.display().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(json: &str) -> PlanInput {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn state_reports_profiles_keys_and_modes_without_secrets() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        keys::add_key(Some(user.path()), "work", "sk-secret-abcd1234", Some("w")).unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            "default_profile=\"o\"\n[profiles.o]\nprovider=\"openai\"\nmodel=\"m\"\napi_key_env=\"MY_VAR\"\napi_key_name=\"work\"\n",
        )
        .unwrap();
        let s = state(proj.path(), Some(user.path()));
        assert!(!s.needs_onboarding);
        assert_eq!(s.default_profile, "o");
        assert_eq!(s.profiles.len(), 1);
        assert_eq!(s.keys.keys[0].masked, "…1234");
        assert_eq!(s.keys.keys[0].used_by, ["profile `o` (user)"]);
        assert_eq!(s.modes, ModesState::Missing);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("sk-secret"), "{json}");
        assert!(json.contains("\"needsOnboarding\":false"), "{json}");
        assert!(json.contains("\"status\":\"missing\""), "{json}");
        // Fresh dir needs onboarding.
        let fresh = tempfile::tempdir().unwrap();
        assert!(state(proj.path(), Some(fresh.path())).needs_onboarding);
        assert!(!json.contains("\"env\""), "{json}");
    }

    #[test]
    fn modes_ready_shape() {
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
        std::fs::write(
            proj.path().join(".ez/modes.toml"),
            "default_mode=\"a\"\n[modes.a]\ndescription=\"d\"\nmodel=\"x\"\nprofile=\"p\"\n",
        )
        .unwrap();
        let v = serde_json::to_value(modes_state(proj.path())).unwrap();
        assert_eq!(v["status"], "ready");
        assert_eq!(v["defaultMode"], "a");
        assert_eq!(v["modes"][0]["profile"], "p");
        std::fs::write(proj.path().join(".ez/modes.toml"), "[modes.a\n").unwrap();
        assert!(matches!(
            modes_state(proj.path()),
            ModesState::Broken { .. }
        ));
    }

    #[test]
    fn plan_conversion_and_apply_never_leaks_secret() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let json = r#"{"profileName":"or","provider":"openai","baseUrl":"https://openrouter.ai/api/v1/",
            "model":"a","models":["b"],"newKey":{"name":"k1","secret":" sk-xyz-0001 ","description":"d"},
            "keyScope":"user","setDefault":true,"modeModels":[]}"#;
        let pv = preview(input(json), Some(user.path()), proj.path()).unwrap();
        assert!(
            pv.diff.contains("api_key_name") && !pv.diff.contains("sk-xyz"),
            "{}",
            pv.diff
        );
        assert!(!user.path().join("config.toml").exists());
        let r = apply(input(json), Some(user.path()), proj.path()).unwrap();
        assert!(r.credentials_path.is_some());
        let cfg = crate::Config::load(proj.path(), Some(user.path())).unwrap();
        let p = &cfg.profiles["or"];
        assert_eq!(p.models, ["a", "b"]);
        assert_eq!(p.api_key_name.as_deref(), Some("k1"));
        assert_eq!(
            crate::config::named_key(Some(user.path()), "k1").as_deref(),
            Some("sk-xyz-0001")
        );
        assert!(!serde_json::to_string(&r).unwrap().contains("sk-xyz"));
        assert!(!format!("{:?}", input(json).new_key.unwrap()).contains("sk-xyz"));
    }

    #[test]
    fn plan_validation() {
        let bad = |s: &str| input(s).into_plan().is_err();
        assert!(bad(
            r#"{"profileName":"a b","provider":"openai","model":"m","baseUrl":"http://h/v1"}"#
        ));
        assert!(bad(
            r#"{"profileName":"a","provider":"nope","model":"m","baseUrl":"http://h/v1"}"#
        ));
        assert!(bad(
            r#"{"profileName":"a","provider":"openai","model":"","baseUrl":"http://h/v1"}"#
        ));
        assert!(bad(
            r#"{"profileName":"a","provider":"openai","model":"m","baseUrl":"http://h/v1","keyScope":"x"}"#
        ));
        // The endpoint URL is required and must be an absolute http(s) URL.
        assert!(bad(
            r#"{"profileName":"a","provider":"openai","model":"m"}"#
        ));
        assert!(bad(
            r#"{"profileName":"a","provider":"openai","model":"m","baseUrl":"localhost:11434"}"#
        ));
        let p = input(
            r#"{"profileName":"a","provider":"anthropic","model":"m","baseUrl":"https://api.anthropic.com/",
            "modeModels":[{"mode":"x","profile":"a","model":"m"},{"mode":"y"}]}"#,
        )
        .into_plan()
        .unwrap();
        assert_eq!(p.profile.base_url, None);
        assert_eq!(p.mode_models[0].1, Some(("a".into(), "m".into())));
        assert_eq!(p.mode_models[1].1, None);
    }

    #[test]
    fn models_auth_resolution() {
        let user = tempfile::tempdir().unwrap();
        keys::add_key(Some(user.path()), "k", "from-file", None).unwrap();
        let r = |a: Option<ModelsAuth>| resolve_models_auth(a.as_ref(), Some(user.path())).unwrap();
        assert_eq!(r(None), None);
        assert_eq!(
            r(Some(ModelsAuth::KeyName("k".into()))).as_deref(),
            Some("from-file")
        );
        assert_eq!(
            r(Some(ModelsAuth::Secret(" s ".into()))).as_deref(),
            Some("s")
        );
        assert!(
            resolve_models_auth(Some(&ModelsAuth::KeyName("nope".into())), Some(user.path()))
                .is_err()
        );
        let a: ModelsAuth = serde_json::from_str(r#"{"keyName":"k"}"#).unwrap();
        assert!(matches!(a, ModelsAuth::KeyName(_)));
        assert!(!format!("{:?}", ModelsAuth::Secret("zzz".into())).contains("zzz"));
    }

    /// Serve `routes` (path prefix -> (status, expected bearer/x-api-key or ""), body) forever.
    async fn fake_server(routes: Vec<(&'static str, &'static str, &'static str)>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // SAFETY: test-only; set before or while no other thread reads this variable.
        unsafe {
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        }
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| path.split('?').next() == Some(p))
                    .map(|(_, s, b)| (*s, *b))
                    .unwrap_or(("404 Not Found", "{}"));
                let r = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(r.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        url
    }

    const MODELS: &str = r#"{"data":[{"id":"m1"},{"id":"m2"}]}"#;

    #[tokio::test]
    async fn probe_detects_each_flavor_and_explains_failures() {
        let secret = || Some(ModelsAuth::Secret("sk-test".into()));
        // OpenAI-compatible server (only /v1/models exists).
        let oa = fake_server(vec![("/v1/models", "200 OK", MODELS)]).await;
        let r = probe(&format!("{oa}/v1/"), secret(), None, None)
            .await
            .unwrap();
        assert_eq!(r.kind.as_deref(), Some("openai"));
        assert_eq!(r.models.len(), 2);
        assert_eq!(r.message, "OpenAI-compatible API ✓ (2 models)");
        assert_eq!(r.attempts.len(), 1);
        // Anthropic-style server (/v1/models at the root); the OpenAI probe 404s first.
        let an = fake_server(vec![(
            "/v1/models",
            "200 OK",
            r#"{"data":[{"id":"claude-x"}]}"#,
        )])
        .await;
        let r = probe(&an, secret(), None, None).await.unwrap();
        assert_eq!(r.kind.as_deref(), Some("anthropic"), "{r:?}");
        assert_eq!(r.attempts.len(), 2);
        assert_eq!(r.message, "Anthropic API ✓ (1 model)");
        // `sk-ant-` keys try Anthropic first.
        let r = probe(&an, Some(ModelsAuth::Secret("sk-ant-1".into())), None, None)
            .await
            .unwrap();
        assert_eq!(
            (r.kind.as_deref(), r.attempts.len()),
            (Some("anthropic"), 1)
        );
        // Forced flavor checks only that one.
        let r = probe(&an, secret(), None, Some("openai")).await.unwrap();
        assert_eq!(r.kind, None);
        assert_eq!(r.attempts.len(), 1);
        // 401 beats 404 in the explanation.
        let bad = fake_server(vec![(
            "/v1/models",
            "401 Unauthorized",
            r#"{"error":{"message":"bad key"}}"#,
        )])
        .await;
        let r = probe(&bad, secret(), None, None).await.unwrap();
        assert_eq!(r.kind, None);
        assert!(
            r.message.contains("rejected the API key (HTTP 401)"),
            "{}",
            r.message
        );
        // Wrong path: everything 404s.
        let r = probe(&format!("{oa}/nope"), secret(), None, None)
            .await
            .unwrap();
        assert!(
            r.message.contains("404") && r.message.contains("/v1"),
            "{}",
            r.message
        );
        assert_eq!(r.attempts[0].status, Some(404));
        // Connection refused: a port that was just bound and released (a fixed port like 1 may be
        // silently dropped by a firewall instead).
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}", l.local_addr().unwrap())
        };
        let r = probe(&closed, None, None, None).await.unwrap();
        assert!(
            r.message.to_lowercase().contains("connect"),
            "{}",
            r.message
        );
        assert_eq!(r.attempts[0].status, None);
        // Invalid URL is an error, a missing named key too.
        assert!(probe("nope", None, None, None).await.is_err());
        assert!(
            probe(&oa, Some(ModelsAuth::KeyName("zz".into())), None, None)
                .await
                .is_err()
        );
    }

    #[test]
    fn key_ops() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        keys_add(Some(user.path()), "k", "abcdefghijkl", Some("d")).unwrap();
        assert!(keys_use(Some(user.path()), proj.path(), "nope", Some("p"), false).is_err());
        let u = keys_use(Some(user.path()), proj.path(), "k", Some("p"), true).unwrap();
        assert_eq!(u.scope, "project");
        let l = keys_list(Some(user.path()), proj.path()).unwrap();
        assert_eq!(l.keys[0].used_by, ["profile `p` (this project)"]);
        let r = keys_remove(Some(user.path()), proj.path(), "k").unwrap();
        assert_eq!(r.still_used_by.len(), 1);
        assert!(keys_remove(Some(user.path()), proj.path(), "k").is_err());
        let l = keys_list(Some(user.path()), proj.path()).unwrap();
        assert_eq!(l.missing[0].name, "k");
    }

    #[tokio::test]
    async fn init_modes_writes_and_refuses_overwrite() {
        let proj = tempfile::tempdir().unwrap();
        let r = init_modes(proj.path(), &InitInput::default())
            .await
            .unwrap();
        assert!(matches!(r.modes, ModesState::Ready { .. }));
        assert!(
            init_modes(proj.path(), &InitInput::default())
                .await
                .is_err()
        );
        let r = init_modes(
            proj.path(),
            &InitInput {
                minimal: true,
                force: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(r.path.ends_with("modes.toml"));
    }
}
