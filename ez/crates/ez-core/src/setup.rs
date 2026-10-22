//! Onboarding backend: provider presets, and writing profiles, credentials and per-mode models.
//!
//! Everything here is plain file manipulation (via `toml_edit`, so existing comments and
//! formatting survive) so the setup wizard and `ez setup --non-interactive` share it and it can
//! be tested without a terminal.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use toml_edit::{Array, DocumentMut, Item, Table, value};

use crate::config::{Profile, ProviderKind, credentials_path, user_config_dir};
use crate::keys::{self, KeyScope};

/// A quick-fill endpoint offered by the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointSuggestion {
    /// Also the default profile name.
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    /// The API flavor this endpoint speaks.
    pub kind: ProviderKind,
}

pub const ENDPOINT_SUGGESTIONS: &[EndpointSuggestion] = &[
    EndpointSuggestion {
        id: "anthropic",
        label: "Anthropic",
        url: "https://api.anthropic.com",
        kind: ProviderKind::Anthropic,
    },
    EndpointSuggestion {
        id: "openai",
        label: "OpenAI",
        url: "https://api.openai.com/v1",
        kind: ProviderKind::Openai,
    },
    EndpointSuggestion {
        id: "openrouter",
        label: "OpenRouter",
        url: "https://openrouter.ai/api/v1",
        kind: ProviderKind::Openai,
    },
    EndpointSuggestion {
        id: "ollama",
        label: "Ollama",
        url: "http://localhost:11434/v1",
        kind: ProviderKind::Openai,
    },
    EndpointSuggestion {
        id: "lmstudio",
        label: "LM Studio",
        url: "http://localhost:1234/v1",
        kind: ProviderKind::Openai,
    },
    EndpointSuggestion {
        id: "vllm",
        label: "vLLM",
        url: "http://localhost:8000/v1",
        kind: ProviderKind::Openai,
    },
];

/// The endpoint suggestion implied by the shape of an API key (a hint only, always editable).
pub fn suggestion_for_key(key: &str) -> Option<&'static EndpointSuggestion> {
    let id = if key.starts_with("sk-ant-") {
        "anthropic"
    } else if key.starts_with("sk-or-") {
        "openrouter"
    } else if key.starts_with("sk-") {
        "openai"
    } else {
        return None;
    };
    ENDPOINT_SUGGESTIONS.iter().find(|s| s.id == id)
}

/// Validate an endpoint typed by the user: must be an absolute http(s) URL with a host and no
/// whitespace. Returns it trimmed and without trailing slashes.
pub fn validate_endpoint(raw: &str) -> Result<String, String> {
    let u = raw.trim();
    if u.is_empty() {
        return Err("the endpoint URL is required".into());
    }
    let lower = u.to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &u[8..]
    } else if lower.starts_with("http://") {
        &u[7..]
    } else {
        return Err("the endpoint must start with http:// or https://".into());
    };
    if u.chars().any(char::is_whitespace) {
        return Err("the endpoint must not contain spaces".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let hostname = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    if hostname.is_empty() {
        return Err("the endpoint needs a host, e.g. https://example.com/v1".into());
    }
    Ok(u.trim_end_matches('/').to_string())
}

fn host_and_port(url: &str) -> (String, Option<u16>) {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    let (h, port) = match host.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.contains(']') => (h, p.parse().ok()),
        _ => (host, None),
    };
    (h.trim_matches(['[', ']']).to_ascii_lowercase(), port)
}

fn is_local(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
}

/// The suggestion whose endpoint `url` points at (same host; for local servers the same port).
pub fn suggestion_for_url(url: &str) -> Option<&'static EndpointSuggestion> {
    let (host, port) = host_and_port(url);
    ENDPOINT_SUGGESTIONS.iter().find(|s| {
        let (sh, sp) = host_and_port(s.url);
        if is_local(&sh) {
            is_local(&host) && port == sp
        } else {
            host == sh
        }
    })
}

/// True for endpoints on this machine or LAN (loopback, private ranges, `*.local`): these usually
/// need no API key.
pub fn is_local_endpoint(url: &str) -> bool {
    let (host, _) = host_and_port(url);
    if is_local(&host) || host.ends_with(".local") || host.starts_with("127.") {
        return true;
    }
    let o: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    host.split('.').count() == 4
        && o.len() == 4
        && (o[0] == 10 || (o[0] == 192 && o[1] == 168) || (o[0] == 172 && (16..32).contains(&o[1])))
}

/// Default profile name from provider + endpoint: `anthropic`, `openai`, `openrouter`, `ollama`,
/// `lmstudio`, `vllm`, or the sanitised hostname.
pub fn suggested_profile_name(kind: ProviderKind, url: &str) -> String {
    let (host, _) = host_and_port(url);
    if kind == ProviderKind::Anthropic && (host.is_empty() || host == "api.anthropic.com") {
        return "anthropic".into();
    }
    if kind != ProviderKind::Anthropic
        && let Some(s) = suggestion_for_url(url)
    {
        return s.id.into();
    }
    let name: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        if kind == ProviderKind::Anthropic {
            "anthropic".into()
        } else {
            "openai-compatible".into()
        }
    } else {
        name
    }
}

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// The endpoint a provider kind uses when `base_url` is unset.
pub fn default_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => ANTHROPIC_BASE_URL,
        _ => "https://api.openai.com/v1",
    }
}

/// `base_url` as stored in config: `None` when it equals the provider's built-in default.
pub fn stored_base_url(kind: ProviderKind, url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    if url.is_empty() || url == default_base_url(kind) {
        None
    } else {
        Some(url.to_string())
    }
}

/// What the wizard (or `--non-interactive`) decided.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupPlan {
    pub profile_name: String,
    pub profile: Profile,
    /// Make this the `default_profile` (always done when the user config has none yet).
    pub set_default: bool,
    /// A new named key to store in `credentials.toml` (never in a config file). The profile
    /// should reference it through `profile.api_key_name`.
    pub new_key: Option<NewKey>,
    /// Where `profile.api_key_name` is saved: the user's profile or this project's config.
    pub key_scope: KeyScope,
    /// Per mode: `Some((profile, model))` pins it, `None` means "profile default".
    pub mode_models: Vec<(String, Option<(String, String)>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewKey {
    pub name: String,
    pub secret: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetupReport {
    pub config_path: PathBuf,
    /// Set when the key reference went to the project's `.ez/config.toml`.
    pub project_config_path: Option<PathBuf>,
    pub credentials_path: Option<PathBuf>,
    pub modes_path: Option<PathBuf>,
    /// Modes whose `profile`/`model` keys were changed.
    pub modes_updated: usize,
}

/// True when no configured profile is usable: one needs a stored named key that resolves (or be
/// the mock provider, or a keyless profile on a local/LAN endpoint). Environment variables never
/// count as configuration.
pub fn needs_onboarding(project: &Path, user_dir: Option<&Path>) -> bool {
    !configured_profiles(project, user_dir)
        .iter()
        .any(|p| match (&p.api_key_name, p.provider) {
            // Claude Code holds its own credentials (CLAUDE_CODE_OAUTH_TOKEN or its login).
            (_, ProviderKind::Mock | ProviderKind::ClaudeCode) => true,
            (Some(n), _) => crate::config::named_key(user_dir, n).is_some(),
            (None, ProviderKind::Openai) => {
                p.api_key_env.is_none() && p.base_url.as_deref().is_some_and(is_local_endpoint)
            }
            (None, _) => false,
        })
}

/// Profiles defined in the user or project config files (not the built-in defaults).
pub fn configured_profiles(project: &Path, user_dir: Option<&Path>) -> Vec<Profile> {
    let Ok(cfg) = crate::Config::load(project, user_dir) else {
        return vec![];
    };
    let mut names = std::collections::BTreeSet::new();
    let files = [
        user_config_dir(user_dir).map(|d| d.join("config.toml")),
        Some(project_config_file(project)),
    ];
    for f in files.into_iter().flatten() {
        if let Ok(text) = std::fs::read_to_string(f)
            && let Ok(t) = text.parse::<toml::Table>()
            && let Some(toml::Value::Table(p)) = t.get("profiles")
        {
            names.extend(p.keys().cloned());
        }
    }
    cfg.profiles
        .into_values()
        .filter(|p| names.contains(&p.name))
        .collect()
}

fn config_file(user_dir: Option<&Path>) -> Result<PathBuf> {
    Ok(user_config_dir(user_dir)
        .context("cannot determine the user config directory")?
        .join("config.toml"))
}

fn read_or_empty(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn set_str(t: &mut Table, key: &str, val: &str) {
    if t.get(key).and_then(Item::as_str) != Some(val) {
        t[key] = value(val);
    }
}

fn sub_table<'a>(parent: &'a mut Table, key: &str) -> Result<&'a mut Table> {
    if !parent.contains_key(key) {
        let mut t = Table::new();
        t.set_implicit(true);
        parent.insert(key, Item::Table(t));
    }
    parent
        .get_mut(key)
        .and_then(Item::as_table_mut)
        .with_context(|| format!("`{key}` must be a table"))
}

/// `config.toml` text after applying the plan to `existing`.
pub fn render_config(existing: &str, plan: &SetupPlan) -> Result<String> {
    let mut doc: DocumentMut = existing.parse().context("parsing config.toml")?;
    let profiles = sub_table(doc.as_table_mut(), "profiles")?;
    let t = sub_table(profiles, &plan.profile_name)?;
    t.set_implicit(false);
    let p = &plan.profile;
    set_str(t, "provider", p.provider.as_str());
    set_str(t, "model", &p.model);
    match &p.base_url {
        Some(u) => set_str(t, "base_url", u),
        None => {
            t.remove("base_url");
        }
    }
    match &p.api_key_env {
        Some(e) => set_str(t, "api_key_env", e),
        None => {
            t.remove("api_key_env");
        }
    }
    if plan.key_scope == KeyScope::User {
        match &p.api_key_name {
            Some(n) => set_str(t, "api_key_name", n),
            None => {
                t.remove("api_key_name");
            }
        }
    }
    let current: Vec<String> = t
        .get("models")
        .and_then(Item::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if current != p.models {
        if p.models.is_empty() {
            t.remove("models");
        } else {
            let mut arr = Array::new();
            for m in &p.models {
                arr.push(m.as_str());
            }
            if arr.len() > 3 {
                arr.set_trailing_comma(true);
                arr.set_trailing("\n");
                for v in arr.iter_mut() {
                    v.decor_mut().set_prefix("\n    ");
                }
            }
            t["models"] = value(arr);
        }
    }
    if plan.set_default || doc.get("default_profile").is_none() {
        set_str(doc.as_table_mut(), "default_profile", &plan.profile_name);
    }
    Ok(doc.to_string())
}

/// Project `.ez/config.toml` text after applying the plan's key reference, if it has one.
pub fn render_project_config(existing: &str, plan: &SetupPlan) -> Result<Option<String>> {
    match (&plan.key_scope, &plan.profile.api_key_name) {
        (KeyScope::Project, Some(n)) => Ok(Some(keys::render_key_ref(
            existing,
            &plan.profile_name,
            Some(n),
        )?)),
        _ => Ok(None),
    }
}

fn project_config_file(project: &Path) -> PathBuf {
    project.join(".ez").join("config.toml")
}

/// `modes.toml` text after applying the per-mode choices; `None` if there is nothing to write.
pub fn render_modes(existing: Option<&str>, plan: &SetupPlan) -> Result<Option<String>> {
    if plan.mode_models.is_empty() {
        return Ok(None);
    }
    let Some(existing) = existing else {
        if plan.mode_models.iter().any(|(_, c)| c.is_some()) {
            bail!(".ez/modes.toml does not exist; run `ez init` first");
        }
        return Ok(None);
    };
    let mut doc: DocumentMut = existing.parse().context("parsing modes.toml")?;
    let modes = doc
        .get_mut("modes")
        .and_then(Item::as_table_mut)
        .context("modes.toml has no [modes.*] tables")?;
    for (id, choice) in &plan.mode_models {
        let Some(t) = modes.get_mut(id).and_then(Item::as_table_mut) else {
            bail!("mode `{id}` is not defined in modes.toml");
        };
        match choice {
            Some((profile, model)) => {
                set_str(t, "profile", profile);
                set_str(t, "model", model);
            }
            None => {
                t.remove("profile");
                t.remove("model");
            }
        }
    }
    Ok(Some(doc.to_string()))
}

fn modes_file(project: &Path) -> PathBuf {
    project.join(".ez").join("modes.toml")
}

/// Apply a plan: user `config.toml`, `credentials.toml` and the project's `modes.toml`.
/// Nothing is written if any of the documents fails to render.
pub fn apply(plan: &SetupPlan, user_dir: Option<&Path>, project: &Path) -> Result<SetupReport> {
    let cfg_path = config_file(user_dir)?;
    let new_cfg = render_config(&read_or_empty(&cfg_path)?, plan)?;
    let creds = match &plan.new_key {
        Some(k) => {
            let path = credentials_path(user_dir).context("no credentials path")?;
            let text = keys::render_key(
                &read_or_empty(&path)?,
                &k.name,
                &k.secret,
                k.description.as_deref(),
            )?;
            Some((path, text))
        }
        None => None,
    };
    let proj_cfg_path = project_config_file(project);
    let proj_cfg = render_project_config(&read_or_empty(&proj_cfg_path)?, plan)?;
    let modes_path = modes_file(project);
    let existing_modes = std::fs::read_to_string(&modes_path).ok();
    let new_modes = render_modes(existing_modes.as_deref(), plan)?;

    let mut report = SetupReport {
        config_path: cfg_path.clone(),
        ..Default::default()
    };
    if let Some(dir) = cfg_path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&cfg_path, new_cfg)
        .with_context(|| format!("writing {}", cfg_path.display()))?;
    if let Some((path, text)) = creds {
        keys::write_private(&path, &text)?;
        report.credentials_path = Some(path);
    }
    if let Some(text) = proj_cfg {
        if let Some(d) = proj_cfg_path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&proj_cfg_path, text)
            .with_context(|| format!("writing {}", proj_cfg_path.display()))?;
        report.project_config_path = Some(proj_cfg_path);
    }
    if let Some(text) = new_modes
        && existing_modes.as_deref() != Some(text.as_str())
    {
        std::fs::write(&modes_path, text)
            .with_context(|| format!("writing {}", modes_path.display()))?;
        report.modes_path = Some(modes_path);
        report.modes_updated = plan.mode_models.len();
    }
    Ok(report)
}

fn unified(name: &str, old: &str, new: &str) -> String {
    similar::TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(2)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// Human-readable account of what [`apply`] would write. Never contains the API key.
pub fn preview(plan: &SetupPlan, user_dir: Option<&Path>, project: &Path) -> Result<String> {
    let cfg_path = config_file(user_dir)?;
    let old_cfg = read_or_empty(&cfg_path)?;
    let new_cfg = render_config(&old_cfg, plan)?;
    let mut out = String::new();
    out.push_str(&format!("# {}\n", cfg_path.display()));
    if old_cfg == new_cfg {
        out.push_str("(no changes)\n");
    } else {
        out.push_str(&unified("config.toml", &old_cfg, &new_cfg));
    }
    if let Some(k) = &plan.new_key {
        let path = credentials_path(user_dir).context("no credentials path")?;
        out.push_str(&format!(
            "\nkey `{}` will be saved to {} (mode 0600)\n",
            k.name,
            path.display()
        ));
    }
    let pc_path = project_config_file(project);
    let old_pc = read_or_empty(&pc_path)?;
    if let Some(new_pc) = render_project_config(&old_pc, plan)? {
        out.push_str(&format!("\n# {} (this project only)\n", pc_path.display()));
        out.push_str(&unified("config.toml", &old_pc, &new_pc));
    }
    let modes_path = modes_file(project);
    let old_modes = std::fs::read_to_string(&modes_path).ok();
    if let Some(new_modes) = render_modes(old_modes.as_deref(), plan)? {
        out.push_str(&format!("\n# {}\n", modes_path.display()));
        let old = old_modes.unwrap_or_default();
        if old == new_modes {
            out.push_str("(no changes)\n");
        } else {
            out.push_str(&unified("modes.toml", &old, &new_modes));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, ModeSet};

    fn profile(name: &str, models: &[&str]) -> Profile {
        Profile {
            name: name.into(),
            provider: ProviderKind::Openai,
            model: models.first().copied().unwrap_or("m").into(),
            models: models.iter().map(|s| s.to_string()).collect(),
            api_key_name: None,
            api_key_env: Some("OPENROUTER_API_KEY".into()),
            base_url: Some("https://openrouter.ai/api/v1".into()),
            max_tokens: 32_000,
            context_window: 200_000,
            thinking: None,
        }
    }

    fn plan(models: &[&str]) -> SetupPlan {
        SetupPlan {
            profile_name: "openrouter".into(),
            profile: profile("openrouter", models),
            set_default: false,
            new_key: None,
            key_scope: KeyScope::User,
            mode_models: vec![],
        }
    }

    #[test]
    fn onboarding_detection() {
        let d = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let cfg = |t: &str| std::fs::write(d.path().join("config.toml"), t).unwrap();
        assert!(needs_onboarding(proj.path(), Some(d.path())));
        // A config file alone, or an env-var-only profile, is not "configured".
        cfg(
            "[profiles.a]\nprovider=\"anthropic\"\nmodel=\"m\"\napi_key_env=\"ANTHROPIC_API_KEY\"\n",
        );
        assert!(needs_onboarding(proj.path(), Some(d.path())));
        // A named key that is not stored does not count; a stored one does.
        cfg("[profiles.a]\nprovider=\"anthropic\"\nmodel=\"m\"\napi_key_name=\"k\"\n");
        assert!(needs_onboarding(proj.path(), Some(d.path())));
        keys::add_key(Some(d.path()), "k", "sk-1", None).unwrap();
        assert!(!needs_onboarding(proj.path(), Some(d.path())));
        // Keyless local endpoint counts; keyless remote does not.
        cfg(
            "[profiles.o]\nprovider=\"openai\"\nmodel=\"m\"\nbase_url=\"http://localhost:11434/v1\"\n",
        );
        assert!(!needs_onboarding(proj.path(), Some(d.path())));
        cfg(
            "[profiles.o]\nprovider=\"openai\"\nmodel=\"m\"\nbase_url=\"https://api.openai.com/v1\"\n",
        );
        assert!(needs_onboarding(proj.path(), Some(d.path())));
    }

    #[test]
    fn config_edit_preserves_comments_and_loads_back() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let original = "# my settings\nmax_turn_requests = 10 # keep\n\n[profiles.mine]\n# my own\nprovider = \"mock\"\nmodel = \"x\"\n";
        std::fs::write(user.path().join("config.toml"), original).unwrap();
        let mut p = plan(&["a/one", "b/two"]);
        p.set_default = true;
        let r = apply(&p, Some(user.path()), proj.path()).unwrap();
        assert_eq!(r.config_path, user.path().join("config.toml"));
        let text = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        assert!(text.contains("# my settings"), "{text}");
        assert!(text.contains("max_turn_requests = 10 # keep"), "{text}");
        assert!(text.contains("# my own"), "{text}");
        assert!(!text.contains("sk-"), "{text}");
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(cfg.default_profile, "openrouter");
        let o = &cfg.profiles["openrouter"];
        assert_eq!(o.name, "openrouter");
        assert_eq!(o.models, ["a/one", "b/two"]);
        assert_eq!(o.model, "a/one");
        assert_eq!(o.provider, ProviderKind::Openai);
        assert_eq!(o.api_key_env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(cfg.profiles["mine"].model, "x");
        assert_eq!(cfg.max_turn_requests, 10);
        // Re-applying is idempotent.
        apply(&p, Some(user.path()), proj.path()).unwrap();
        let again = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        assert_eq!(text, again);
    }

    #[test]
    fn existing_profile_is_updated_in_place() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            "default_profile = \"openrouter\"\n[profiles.openrouter]\nprovider = \"openai\"\nmodel = \"old\"\napi_key_env = \"OLD\" # old var\nthinking = \"high\"\n",
        )
        .unwrap();
        let mut p = plan(&["new"]);
        p.profile.api_key_env = None;
        apply(&p, Some(user.path()), proj.path()).unwrap();
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        let o = &cfg.profiles["openrouter"];
        assert_eq!(o.model, "new");
        assert_eq!(o.api_key_env, None);
        assert_eq!(o.thinking.as_ref().and_then(|v| v.as_str()), Some("high"));
    }

    #[test]
    fn first_profile_becomes_default_without_set_default() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        apply(&plan(&["m"]), Some(user.path()), proj.path()).unwrap();
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(cfg.default_profile, "openrouter");
        // A later profile does not steal the default.
        let mut p2 = plan(&["z"]);
        p2.profile_name = "other".into();
        p2.profile.name = "other".into();
        apply(&p2, Some(user.path()), proj.path()).unwrap();
        let cfg = Config::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(cfg.default_profile, "openrouter");
        assert!(cfg.profiles.contains_key("other"));
    }

    #[cfg(unix)]
    #[test]
    fn named_keys_are_private_and_never_in_config() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let user = base.path().join("nested/ez");
        let proj = tempfile::tempdir().unwrap();
        let mut p = plan(&["m"]);
        p.new_key = Some(NewKey {
            name: "work".into(),
            secret: "sk-secret-123".into(),
            description: Some("team".into()),
        });
        p.profile.api_key_env = None;
        p.profile.api_key_name = Some("work".into());
        let r = apply(&p, Some(&user), proj.path()).unwrap();
        let cred = user.join("credentials.toml");
        assert_eq!(r.credentials_path.as_deref(), Some(cred.as_path()));
        assert_eq!(
            std::fs::metadata(&cred).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let cfg = std::fs::read_to_string(user.join("config.toml")).unwrap();
        assert!(
            !cfg.contains("sk-secret") && cfg.contains("api_key_name = \"work\""),
            "{cfg}"
        );
        assert_eq!(
            crate::config::named_key(Some(&user), "work").as_deref(),
            Some("sk-secret-123")
        );
        // Shared by a second profile; file stays private.
        let mut p2 = plan(&["m"]);
        p2.profile_name = "second".into();
        p2.profile.name = "second".into();
        p2.profile.api_key_name = Some("work".into());
        apply(&p2, Some(&user), proj.path()).unwrap();
        let c = crate::Config::load(proj.path(), Some(&user)).unwrap();
        assert_eq!(c.profiles["second"].api_key_name.as_deref(), Some("work"));
        assert_eq!(
            std::fs::metadata(&cred).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let pv = preview(&p, Some(&user), proj.path()).unwrap();
        assert!(
            pv.contains("credentials.toml") && !pv.contains("sk-secret"),
            "{pv}"
        );
    }

    #[test]
    fn project_scope_writes_reference_to_project_config_only() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let mut p = plan(&["m"]);
        p.key_scope = KeyScope::Project;
        p.profile.api_key_name = Some("client-acme".into());
        p.new_key = Some(NewKey {
            name: "client-acme".into(),
            secret: "sk-acme-0000".into(),
            description: None,
        });
        let r = apply(&p, Some(user.path()), proj.path()).unwrap();
        assert!(r.project_config_path.is_some());
        let u = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        assert!(!u.contains("api_key_name"), "{u}");
        let pc = std::fs::read_to_string(proj.path().join(".ez/config.toml")).unwrap();
        assert!(
            pc.contains("api_key_name = \"client-acme\"") && !pc.contains("sk-acme"),
            "{pc}"
        );
        let c = crate::Config::load(proj.path(), Some(user.path())).unwrap();
        let o = &c.profiles["openrouter"];
        // Project overrides only the key reference; the rest comes from the user profile.
        assert_eq!(o.api_key_name.as_deref(), Some("client-acme"));
        assert_eq!(o.model, "m");
        assert_eq!(o.provider, ProviderKind::Openai);
    }

    #[test]
    fn key_resolution_order_and_missing_key_error() {
        let user = tempfile::tempdir().unwrap();
        keys::add_key(Some(user.path()), "k1", "from-file", None).unwrap();
        let mut prof = profile("p", &[]);
        prof.provider = ProviderKind::Anthropic;
        prof.api_key_env = Some("EZ_TEST_SURELY_UNSET_VAR".into());
        // No name, env unset -> none -> anthropic errors.
        assert_eq!(
            crate::config::resolve_api_key(&prof, Some(user.path())).unwrap(),
            None
        );
        let e = crate::config::make_provider_with(&prof, Some(user.path()))
            .err()
            .unwrap()
            .to_string();
        assert!(e.contains("ez setup"), "{e}");
        // Named key wins over env.
        // SAFETY: test-only; set before or while no other thread reads this variable.
        unsafe {
            std::env::set_var("EZ_TEST_SET_VAR", "from-env");
        }
        prof.api_key_env = Some("EZ_TEST_SET_VAR".into());
        assert_eq!(
            crate::config::resolve_api_key(&prof, Some(user.path()))
                .unwrap()
                .as_deref(),
            Some("from-env")
        );
        prof.api_key_name = Some("k1".into());
        assert_eq!(
            crate::config::resolve_api_key(&prof, Some(user.path()))
                .unwrap()
                .as_deref(),
            Some("from-file")
        );
        assert!(crate::config::make_provider_with(&prof, Some(user.path())).is_ok());
        // Missing named key is an explicit error (no silent fall back to the env var).
        prof.api_key_name = Some("gone".into());
        let e = crate::config::make_provider_with(&prof, Some(user.path()))
            .err()
            .unwrap()
            .to_string();
        assert!(
            e.contains("named key `gone`") && e.contains("ez keys add gone"),
            "{e}"
        );
    }

    #[test]
    fn project_profile_override_is_field_wise() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join("config.toml"),
            "[profiles.anthropic]\nmodel = \"u-model\"\nmax_tokens = 123\napi_key_name = \"personal\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
        std::fs::write(
            proj.path().join(".ez/config.toml"),
            "[profiles.anthropic]\napi_key_name = \"client-acme\"\n",
        )
        .unwrap();
        let c = crate::Config::load(proj.path(), Some(user.path())).unwrap();
        let a = &c.profiles["anthropic"];
        assert_eq!(a.api_key_name.as_deref(), Some("client-acme"));
        assert_eq!((a.model.as_str(), a.max_tokens), ("u-model", 123));
        assert_eq!(a.api_key_env, None);
    }

    const MODES: &str = "# team modes\ndefault_mode = \"explore\"\n\n[modes.explore]\n# model = \"claude-haiku-5-5\"\ndescription = \"look\"\nmodel = \"legacy\"\n\n[modes.explore.permissions]\nBash = \"ask\"\n\n[modes.test]\ndescription = \"test\"\nprofile = \"gone\"\nmodel = \"x\"\n";

    #[test]
    fn mode_models_set_and_remove() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
        let mp = proj.path().join(".ez/modes.toml");
        std::fs::write(&mp, MODES).unwrap();
        let mut p = plan(&["fast", "big"]);
        p.mode_models = vec![
            ("explore".into(), Some(("openrouter".into(), "fast".into()))),
            ("test".into(), None),
        ];
        let r = apply(&p, Some(user.path()), proj.path()).unwrap();
        assert_eq!(r.modes_path.as_deref(), Some(mp.as_path()));
        let text = std::fs::read_to_string(&mp).unwrap();
        assert!(text.contains("# team modes"), "{text}");
        assert!(text.contains("# model = \"claude-haiku-5-5\""), "{text}");
        assert!(text.contains("[modes.explore.permissions]"), "{text}");
        let ms = ModeSet::load(proj.path(), Some(user.path())).unwrap();
        let e = ms.get("explore").unwrap();
        assert_eq!(e.profile.as_deref(), Some("openrouter"));
        assert_eq!(e.model.as_deref(), Some("fast"));
        let t = ms.get("test").unwrap();
        assert_eq!((t.profile.clone(), t.model.clone()), (None, None));
        // Unknown mode is an error and nothing is written.
        let mut bad = plan(&["m"]);
        bad.mode_models = vec![("nope".into(), None)];
        let before = std::fs::read_to_string(user.path().join("config.toml")).unwrap();
        bad.profile_name = "zzz".into();
        assert!(apply(&bad, Some(user.path()), proj.path()).is_err());
        assert_eq!(
            before,
            std::fs::read_to_string(user.path().join("config.toml")).unwrap()
        );
        // Missing modes.toml: pinning is an error, clearing is a no-op.
        let empty = tempfile::tempdir().unwrap();
        assert!(apply(&p, Some(user.path()), empty.path()).is_err());
    }

    #[test]
    fn preview_shows_diffs() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
        std::fs::write(proj.path().join(".ez/modes.toml"), MODES).unwrap();
        let mut p = plan(&["fast"]);
        p.mode_models = vec![("explore".into(), Some(("openrouter".into(), "fast".into())))];
        let pv = preview(&p, Some(user.path()), proj.path()).unwrap();
        assert!(
            pv.contains("+[profiles.openrouter]") || pv.contains("+provider"),
            "{pv}"
        );
        assert!(pv.contains("+profile = \"openrouter\""), "{pv}");
        assert!(pv.contains("modes.toml"), "{pv}");
        // Nothing was written by previewing.
        assert!(!user.path().join("config.toml").exists());
    }

    #[test]
    fn base_url_normalisation() {
        assert_eq!(
            stored_base_url(ProviderKind::Anthropic, "https://api.anthropic.com/"),
            None
        );
        assert_eq!(
            stored_base_url(ProviderKind::Openai, "http://localhost:11434/v1/").as_deref(),
            Some("http://localhost:11434/v1")
        );
    }

    #[test]
    fn endpoint_validation() {
        assert_eq!(
            validate_endpoint("  https://api.openai.com/v1/ ").unwrap(),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            validate_endpoint("HTTP://localhost:11434").unwrap(),
            "HTTP://localhost:11434"
        );
        for bad in [
            "",
            "  ",
            "localhost:11434",
            "ftp://x.com",
            "http://",
            "https:///v1",
            "http://a b",
        ] {
            assert!(validate_endpoint(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn suggestions_drive_profile_name_and_locality() {
        use ProviderKind::{Anthropic, Openai};
        let cases = [
            ("https://api.openai.com/v1", "openai"),
            ("https://openrouter.ai/api/v1", "openrouter"),
            ("http://localhost:11434/v1", "ollama"),
            ("http://127.0.0.1:1234/v1", "lmstudio"),
            ("http://localhost:8000/v1", "vllm"),
            ("http://localhost:9999/v1", "localhost"),
            (
                "https://llm.corp.example.com:8443/v1",
                "llm-corp-example-com",
            ),
        ];
        for (url, name) in cases {
            assert_eq!(suggested_profile_name(Openai, url), name, "{url}");
        }
        assert_eq!(
            suggested_profile_name(Anthropic, "https://api.anthropic.com"),
            "anthropic"
        );
        assert_eq!(
            suggested_profile_name(Anthropic, "https://proxy.example.com"),
            "proxy-example-com"
        );
        for local in [
            "http://localhost:1/v1",
            "http://192.168.1.5:8000",
            "http://10.0.0.2",
            "http://172.20.1.1",
            "http://box.local/v1",
            "http://[::1]:80",
        ] {
            assert!(is_local_endpoint(local), "{local}");
        }
        for remote in [
            "https://api.openai.com/v1",
            "http://172.32.0.1",
            "http://8.8.8.8",
        ] {
            assert!(!is_local_endpoint(remote), "{remote}");
        }
        assert_eq!(
            suggestion_for_key("sk-ant-api03-x").unwrap().id,
            "anthropic"
        );
        assert_eq!(suggestion_for_key("sk-or-v1-x").unwrap().id, "openrouter");
        assert_eq!(suggestion_for_key("sk-proj-x").unwrap().id, "openai");
        assert!(suggestion_for_key("abc").is_none());
        assert_eq!(ENDPOINT_SUGGESTIONS.len(), 6);
    }
}
