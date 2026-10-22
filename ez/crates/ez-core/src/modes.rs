//! Lifecycle modes loaded from `modes.toml`.

use anyhow::{Context, Result, bail};
use ez_hooks::HooksConfig;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

use crate::config::user_config_dir;
use crate::permissions::Permission;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ModeDef {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
    /// Appended to the system prompt while the mode is active.
    #[serde(default)]
    pub prompt: String,
    /// Built-in tool allowlist; `None` means all built-ins.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// Task selectors (globs over mise task names).
    #[serde(default)]
    pub mise_tasks: Vec<String>,
    /// Glob selectors over full MCP tool names (e.g. `mcp__github__*`). Empty means no MCP
    /// tools in this mode (the implicit default mode selects `mcp__*`).
    #[serde(default)]
    pub mcp_tools: Vec<String>,
    /// Modes `SwitchMode` may target; empty means any other mode.
    #[serde(default)]
    pub next: Vec<String>,
    /// Transitions that skip user confirmation.
    #[serde(default)]
    pub auto_advance_to: Vec<String>,
    /// Profile name (from `config.toml`) used while this mode is active; defaults to the
    /// session profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Model id used while this mode is active, overriding the profile's model and the CLI
    /// `--model`. For backward compatibility a value equal to a configured profile name (with
    /// `profile` unset) selects that profile instead.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub permissions: BTreeMap<String, Permission>,
    #[serde(default)]
    pub hooks: HooksConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModeSet {
    pub default_mode: String,
    /// In file order.
    pub modes: Vec<(String, ModeDef)>,
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

impl ModeSet {
    /// A single permissive `default` mode, used when no modes file exists.
    pub fn implicit() -> Self {
        let def = ModeDef {
            description: "General-purpose mode with all tools".into(),
            mise_tasks: vec!["*".into()],
            mcp_tools: vec!["mcp__*".into()],
            ..Default::default()
        };
        Self {
            default_mode: "default".into(),
            modes: vec![("default".into(), def)],
        }
    }

    pub fn from_toml_str(text: &str) -> Result<Self> {
        let table: toml::Table = text.parse().context("parsing modes.toml")?;
        let mut modes = Vec::new();
        if let Some(t) = table.get("modes") {
            let t = t.as_table().context("`modes` must be a table")?;
            for (id, v) in t {
                let def: ModeDef = v
                    .clone()
                    .try_into()
                    .with_context(|| format!("invalid mode `{id}`"))?;
                modes.push((id.clone(), def));
            }
        }
        if modes.is_empty() {
            bail!("modes.toml defines no modes");
        }
        let default_mode = match table.get("default_mode").and_then(|v| v.as_str()) {
            Some(d) => d.to_string(),
            None => modes[0].0.clone(),
        };
        let set = Self {
            default_mode,
            modes,
        };
        set.validate()?;
        Ok(set)
    }

    /// Project `.ez/modes.toml`, else the user's, else the implicit set.
    pub fn load(project_dir: &Path, user_dir: Option<&Path>) -> Result<Self> {
        let candidates = [
            Some(project_dir.join(".ez/modes.toml")),
            user_config_dir(user_dir).map(|d| d.join("modes.toml")),
        ];
        for path in candidates.into_iter().flatten() {
            if let Ok(text) = std::fs::read_to_string(&path) {
                return Self::from_toml_str(&text)
                    .with_context(|| format!("loading {}", path.display()));
            }
        }
        Ok(Self::implicit())
    }

    fn validate(&self) -> Result<()> {
        if self.get(&self.default_mode).is_none() {
            bail!("default_mode `{}` is not defined", self.default_mode);
        }
        for (id, m) in &self.modes {
            for r in m.next.iter().chain(&m.auto_advance_to) {
                if self.get(r).is_none() {
                    bail!("mode `{id}` references unknown mode `{r}`");
                }
            }
            for t in m.tools.iter().flatten() {
                if !is_known_tool(t) {
                    tracing::warn!("mode `{id}` lists unknown tool `{t}`");
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&ModeDef> {
        self.modes.iter().find(|(i, _)| i == id).map(|(_, m)| m)
    }

    pub fn ids(&self) -> Vec<&str> {
        self.modes.iter().map(|(i, _)| i.as_str()).collect()
    }

    /// Display name for a mode id.
    pub fn display_name(&self, id: &str) -> String {
        self.get(id)
            .and_then(|m| m.name.clone())
            .unwrap_or_else(|| capitalize(id))
    }

    /// Modes `SwitchMode` may target from `from`, in file order.
    pub fn switch_targets(&self, from: &str) -> Vec<&str> {
        let next = self.get(from).map(|m| m.next.as_slice()).unwrap_or(&[]);
        self.modes
            .iter()
            .map(|(i, _)| i.as_str())
            .filter(|i| *i != from && (next.is_empty() || next.iter().any(|n| n == i)))
            .collect()
    }
}

fn is_known_tool(name: &str) -> bool {
    name == "SwitchMode"
        || name.starts_with("mise__")
        || name.starts_with("mcp__")
        || ez_tools::builtin_tools().iter().any(|t| t.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
default_mode = "explore"
[modes.explore]
description = "look"
tools = ["Read", "Nope"]
mise_tasks = ["docs*"]
mcp_tools = ["mcp__github__*"]
next = ["design"]
[modes.explore.permissions]
Bash = "deny"
[[modes.explore.hooks.post_edit]]
run = "mise run fmt"
on_failure = "fix_turn"
[modes.design]
name = "Designing"
auto_advance_to = ["explore"]
"#;

    #[test]
    fn parses_in_order() {
        let s = ModeSet::from_toml_str(SAMPLE).unwrap();
        assert_eq!(s.ids(), ["explore", "design"]);
        assert_eq!(s.display_name("explore"), "Explore");
        assert_eq!(s.display_name("design"), "Designing");
        let e = s.get("explore").unwrap();
        assert_eq!(e.permissions["Bash"], Permission::Deny);
        assert_eq!(e.hooks.post_edit.len(), 1);
        assert_eq!(e.mcp_tools, ["mcp__github__*"]);
        assert!(s.get("design").unwrap().mcp_tools.is_empty());
        assert_eq!(
            ModeSet::implicit().get("default").unwrap().mcp_tools,
            ["mcp__*"]
        );
        assert_eq!(s.switch_targets("explore"), ["design"]);
        assert_eq!(s.switch_targets("design"), ["explore"]);
    }

    #[test]
    fn validation() {
        assert!(ModeSet::from_toml_str("default_mode='x'\n[modes.a]").is_err());
        assert!(ModeSet::from_toml_str("[modes.a]\nnext=['zzz']").is_err());
        assert!(ModeSet::from_toml_str("").is_err());
    }

    #[test]
    fn load_prefers_project_then_user_then_implicit() {
        let user = tempfile::tempdir().unwrap();
        let proj = tempfile::tempdir().unwrap();
        let implicit = ModeSet::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(implicit.ids(), ["default"]);
        std::fs::write(user.path().join("modes.toml"), "[modes.u]").unwrap();
        let m = ModeSet::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(m.ids(), ["u"]);
        std::fs::create_dir_all(proj.path().join(".ez")).unwrap();
        std::fs::write(proj.path().join(".ez/modes.toml"), "[modes.p]").unwrap();
        let m = ModeSet::load(proj.path(), Some(user.path())).unwrap();
        assert_eq!(m.ids(), ["p"]);
    }
}
