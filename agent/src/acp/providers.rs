//! Static registry of supported ACP agent providers. Each entry maps a
//! provider id (chosen by the frontend/API caller when creating a session)
//! to the shell command used to spawn that provider's ACP-speaking CLI.

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Passed verbatim to `AcpAgent::from_str`, which shell-splits it and
    /// treats leading `NAME=value` tokens as env vars.
    pub spawn_command: &'static str,
}

pub const DEFAULT_PROVIDER: &str = "claude";

pub const PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        id: "claude",
        display_name: "Claude",
        spawn_command: "claude-agent-acp",
    },
    ProviderSpec {
        id: "gemini",
        display_name: "Gemini",
        spawn_command: "gemini --experimental-acp",
    },
    ProviderSpec {
        id: "copilot",
        display_name: "Copilot",
        spawn_command: "copilot --acp",
    },
    ProviderSpec {
        id: "codex",
        display_name: "Codex",
        spawn_command: "codex acp",
    },
    ProviderSpec {
        id: "opencode",
        display_name: "OpenCode",
        spawn_command: "opencode acp",
    },
];

pub fn lookup(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|p| p.id == id)
}
