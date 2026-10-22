//! Tool permission resolution.

use ez_tools::ToolKind;
use globset::Glob;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::config::Config;
use crate::modes::ModeDef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Allow,
    Ask,
    Deny,
}

fn lookup(map: &BTreeMap<String, Permission>, tool: &str) -> Option<Permission> {
    if let Some(p) = map.get(tool) {
        return Some(*p);
    }
    map.iter().find_map(|(pat, p)| {
        Glob::new(pat)
            .ok()
            .filter(|g| g.compile_matcher().is_match(tool))
            .map(|_| *p)
    })
}

/// Mode rules, then config rules (exact names before globs), then the default for the tool kind.
pub fn resolve(tool: &str, kind: ToolKind, mode: &ModeDef, config: &Config) -> Permission {
    lookup(&mode.permissions, tool)
        .or_else(|| lookup(&config.permissions, tool))
        .unwrap_or_else(|| {
            if kind.is_read_only() || matches!(tool, "TodoWrite" | "AskUserQuestion") {
                Permission::Allow
            } else {
                Permission::Ask
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(perms: &str) -> ModeDef {
        toml::from_str(&format!("[permissions]\n{perms}")).unwrap()
    }

    #[test]
    fn precedence() {
        let cfg = Config::from_toml_str("[permissions]\nBash = \"allow\"\n\"mise__*\" = \"deny\"")
            .unwrap();
        let m = mode("Bash = \"ask\"\n\"mise__t*\" = \"allow\"");
        assert_eq!(
            resolve("Bash", ToolKind::Execute, &m, &cfg),
            Permission::Ask
        );
        assert_eq!(
            resolve("mise__test", ToolKind::Execute, &m, &cfg),
            Permission::Allow
        );
        assert_eq!(
            resolve("mise__build", ToolKind::Execute, &m, &cfg),
            Permission::Deny
        );
        let none = mode("");
        assert_eq!(
            resolve("Bash", ToolKind::Execute, &none, &cfg),
            Permission::Allow
        );
    }

    #[test]
    fn defaults() {
        let cfg = Config::default();
        let m = mode("");
        assert_eq!(resolve("Read", ToolKind::Read, &m, &cfg), Permission::Allow);
        assert_eq!(resolve("Write", ToolKind::Edit, &m, &cfg), Permission::Ask);
        assert_eq!(
            resolve("TodoWrite", ToolKind::Other, &m, &cfg),
            Permission::Allow
        );
        assert_eq!(
            resolve("AskUserQuestion", ToolKind::Other, &m, &cfg),
            Permission::Allow
        );
    }

    #[test]
    fn mcp_globs() {
        let cfg = Config::from_toml_str("[permissions]\n\"mcp__github__*\" = \"allow\"").unwrap();
        let m = mode("\"mcp__github__delete*\" = \"deny\"");
        let r = |t, k| resolve(t, k, &m, &cfg);
        assert_eq!(r("mcp__github__search", ToolKind::Other), Permission::Allow);
        assert_eq!(
            r("mcp__github__delete_repo", ToolKind::Other),
            Permission::Deny
        );
        // Unlisted MCP tools ask unless annotated read-only.
        assert_eq!(r("mcp__other__x", ToolKind::Other), Permission::Ask);
        assert_eq!(r("mcp__other__x", ToolKind::Read), Permission::Allow);
    }
}
