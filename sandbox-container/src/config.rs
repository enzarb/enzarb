//! Environment configuration.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use crate::docker_prune::PruneConfig;
use crate::dockerd::DockerdConfig;

/// Runtime configuration of sandbox-init.
#[derive(Debug, Clone)]
pub struct Config {
    /// `ENZARB_LISTEN`, default `0.0.0.0:7070`.
    pub listen: SocketAddr,
    /// `ENZARB_AUTH_TOKEN_FILE`, default `/var/run/enzarb/auth/token`.
    pub auth_token_file: PathBuf,
    /// `ENZARB_WORKSPACE`, default `/workspace`: working directory of PTYs.
    pub workspace: PathBuf,
    /// `ENZARB_SHELL`, then `$SHELL`, then `/bin/bash`: the login shell.
    pub shell: String,
    /// `ENZARB_GRACE_SECS`, default 25: how long children get after SIGTERM before SIGKILL.
    pub grace: Duration,
    /// `ENZARB_ACP_AGENT_BIN`, default `claude-agent-acp`: the chat ACP agent
    /// (stdio JSON-RPC). Used as is when it is on `PATH`, else through
    /// `mise exec <chat_runtime_tool> --`.
    pub acp_bin: String,
    /// `ENZARB_CHAT_RUNTIME_TOOL`, default
    /// `npm:@agentclientprotocol/claude-agent-acp`: the mise tool that provides
    /// `acp_bin` when it is not installed in the image. Empty disables.
    pub chat_runtime_tool: String,
    /// `ENZARB_CANCEL_GRACE_SECS`, default 10: how long the agent gets to end
    /// its turn after `session/cancel` (cancel or timeout) before it is killed.
    pub cancel_grace: Duration,
    /// `ENZARB_CLAUDE_TOKEN_FILE`, default `<workspace>/.enzarb/claude-token`.
    pub claude_token_file: PathBuf,
    /// `ENZARB_GIT_BIN`, default `git`.
    pub git_bin: String,
    /// `ENZARB_MISE_BIN` (the image sets `mise`); empty disables: steps then
    /// run commands and agents directly and ignore `tools`/`runtime.install`.
    pub mise_bin: String,
    /// `ENZARB_GIT_NAME` / `ENZARB_GIT_EMAIL`: identity for commits made in runs.
    pub git_name: String,
    pub git_email: String,
    /// `ENZARB_NET_RETRY_DELAYS_MS`, default `10000,30000`: comma-separated
    /// pauses before each retry of a clone/fetch/push that failed on a
    /// transient network error. The attempts are one more than the delays
    /// (at most `netgit::NET_ATTEMPTS`).
    pub net_retry_delays: Vec<Duration>,
    /// `ENZARB_PUSH_ATTEMPT_SECS`, default 120: wall-clock cap of one push attempt.
    pub push_attempt_cap: Duration,
    /// `ENZARB_FETCH_ATTEMPT_SECS`, default 180: wall-clock cap of one clone or
    /// fetch attempt, so a dead host fails early instead of after the kernel's
    /// connect timeout (about 300 s). A transfer slower than this is retried,
    /// then fails: raise it for very large repos.
    pub fetch_attempt_cap: Duration,
    /// `ENZARB_MAX_CHATS`, default 1: live chat (`acp`) channels at once; `0`
    /// disables chat (no `acp` feature, opens get `chat_disabled`). Chats do
    /// not use run slots.
    pub max_chats: usize,
    /// `ENZARB_CHAT_IDLE_SECS`, default 1800: a chat with no message in either
    /// direction for this long is closed with `idle`.
    pub chat_idle: Duration,
    /// `ENZARB_CHAT_MAX_SECS`, default 28800 (8 h): hard lifetime of one chat
    /// channel (closed with `limit`).
    pub chat_max: Duration,
    /// `ENZARB_CHAT_RETENTION_DAYS`, default 7: chat records (transcripts)
    /// untouched for this long are deleted at startup.
    pub chat_retention: Duration,
    /// `ENZARB_CHAT_PERMISSION_MODE`: the permission mode every chat session
    /// starts in (`bypassPermissions`, `default` or `plan`). Default
    /// `bypassPermissions`, or `default` when bypass is not allowed.
    pub chat_permission_mode: ChatMode,
    /// `ENZARB_CHAT_ALLOW_BYPASS`, default true: chats may run in (or switch
    /// to) `bypassPermissions`. False keeps them in `default`/`plan`.
    pub chat_allow_bypass: bool,
    /// `ENZARB_CGROUP_ROOT`, default `/sys/fs/cgroup`: the cgroup v2 directory
    /// read for `stats_request` (the container's own cgroup).
    pub cgroup_root: PathBuf,
    /// `ENZARB_DOCKERD*`: the supervised Docker daemon (ADR-0016).
    pub dockerd: DockerdConfig,
    /// `ENZARB_DOCKER_PRUNE*`, `ENZARB_DOCKER_BIN`: automatic Docker pruning (ADR-0016).
    pub docker_prune: PruneConfig,
}

/// A chat permission mode sandbox-init lets a chat be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMode {
    /// No permission prompts: tool calls run without asking the browser.
    Bypass,
    /// Every tool call that needs approval asks the browser.
    Default,
    /// Read-only planning.
    Plan,
}

impl ChatMode {
    /// The ACP mode id.
    pub fn as_str(self) -> &'static str {
        match self {
            ChatMode::Bypass => "bypassPermissions",
            ChatMode::Default => "default",
            ChatMode::Plan => "plan",
        }
    }

    /// Parse an ACP mode id (exact spelling).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bypassPermissions" => Some(ChatMode::Bypass),
            "default" => Some(ChatMode::Default),
            "plan" => Some(ChatMode::Plan),
            _ => None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([0, 0, 0, 0], 7070).into(),
            auth_token_file: "/var/run/enzarb/auth/token".into(),
            workspace: "/workspace".into(),
            shell: "/bin/bash".into(),
            grace: Duration::from_secs(25),
            acp_bin: "claude-agent-acp".into(),
            chat_runtime_tool: "npm:@agentclientprotocol/claude-agent-acp".into(),
            cancel_grace: Duration::from_secs(10),
            claude_token_file: "/workspace/.enzarb/claude-token".into(),
            git_bin: "git".into(),
            mise_bin: String::new(),
            git_name: "enzarb-agent".into(),
            git_email: "agent@enzarb.dev".into(),
            net_retry_delays: crate::netgit::RETRY_DELAYS.to_vec(),
            push_attempt_cap: crate::netgit::PUSH_ATTEMPT_CAP,
            fetch_attempt_cap: crate::netgit::FETCH_ATTEMPT_CAP,
            max_chats: 1,
            chat_idle: Duration::from_secs(1800),
            chat_max: Duration::from_secs(8 * 3600),
            chat_retention: Duration::from_secs(7 * 86400),
            chat_permission_mode: ChatMode::Bypass,
            chat_allow_bypass: true,
            cgroup_root: "/sys/fs/cgroup".into(),
            dockerd: DockerdConfig::default(),
            docker_prune: PruneConfig::default(),
        }
    }
}

impl Config {
    /// Read configuration from the environment.
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Like [`from_env`](Self::from_env) with an injectable lookup (for tests).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let mut c = Self::default();
        if let Some(v) = get("ENZARB_LISTEN") {
            c.listen = v
                .parse()
                .map_err(|e| anyhow::anyhow!("ENZARB_LISTEN={v:?}: {e}"))?;
        }
        if let Some(v) = get("ENZARB_AUTH_TOKEN_FILE") {
            c.auth_token_file = v.into();
        }
        if let Some(v) = get("ENZARB_WORKSPACE") {
            c.workspace = v.into();
        }
        c.claude_token_file = c.workspace.join(".enzarb/claude-token");
        if let Some(v) = get("ENZARB_CLAUDE_TOKEN_FILE") {
            c.claude_token_file = v.into();
        }
        if let Some(v) = get("ENZARB_CGROUP_ROOT").filter(|s| !s.is_empty()) {
            c.cgroup_root = v.into();
        }
        if let Some(v) = get("ENZARB_ACP_AGENT_BIN").filter(|s| !s.is_empty()) {
            c.acp_bin = v;
        }
        if let Some(v) = get("ENZARB_CHAT_RUNTIME_TOOL") {
            c.chat_runtime_tool = v;
        }
        if let Some(v) = get("ENZARB_CANCEL_GRACE_SECS") {
            c.cancel_grace = Duration::from_secs(
                v.parse()
                    .map_err(|e| anyhow::anyhow!("ENZARB_CANCEL_GRACE_SECS={v:?}: {e}"))?,
            );
        }
        // `ENZARB_PERMISSION_POLICY` is ignored: there is no permission
        // policy any more (the sandbox is the security boundary).
        if let Some(v) = get("ENZARB_GIT_BIN").filter(|s| !s.is_empty()) {
            c.git_bin = v;
        }
        if let Some(v) = get("ENZARB_MISE_BIN") {
            c.mise_bin = v;
        }
        if let Some(v) = get("ENZARB_GIT_NAME").filter(|s| !s.is_empty()) {
            c.git_name = v;
        }
        if let Some(v) = get("ENZARB_GIT_EMAIL").filter(|s| !s.is_empty()) {
            c.git_email = v;
        }
        if let Some(v) = get("ENZARB_NET_RETRY_DELAYS_MS") {
            c.net_retry_delays = v
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse()
                        .map(Duration::from_millis)
                        .map_err(|e| anyhow::anyhow!("ENZARB_NET_RETRY_DELAYS_MS={v:?}: {e}"))
                })
                .collect::<anyhow::Result<_>>()?;
        }
        if let Some(v) = get("ENZARB_PUSH_ATTEMPT_SECS") {
            c.push_attempt_cap = Duration::from_secs(
                v.parse::<u64>()
                    .map_err(|e| anyhow::anyhow!("ENZARB_PUSH_ATTEMPT_SECS={v:?}: {e}"))?
                    .max(1),
            );
        }
        if let Some(v) = get("ENZARB_FETCH_ATTEMPT_SECS") {
            c.fetch_attempt_cap = Duration::from_secs(
                v.parse::<u64>()
                    .map_err(|e| anyhow::anyhow!("ENZARB_FETCH_ATTEMPT_SECS={v:?}: {e}"))?
                    .max(1),
            );
        }
        if let Some(v) = get("ENZARB_MAX_CHATS") {
            c.max_chats = v
                .parse()
                .map_err(|e| anyhow::anyhow!("ENZARB_MAX_CHATS={v:?}: {e}"))?;
        }
        let secs = |k: &str| -> anyhow::Result<Option<u64>> {
            match get(k) {
                None => Ok(None),
                Some(v) => match v.parse::<u64>() {
                    Ok(n) if n > 0 => Ok(Some(n)),
                    Ok(_) => Err(anyhow::anyhow!("{k}={v:?}: must be at least 1")),
                    Err(e) => Err(anyhow::anyhow!("{k}={v:?}: {e}")),
                },
            }
        };
        if let Some(n) = secs("ENZARB_CHAT_IDLE_SECS")? {
            c.chat_idle = Duration::from_secs(n);
        }
        if let Some(n) = secs("ENZARB_CHAT_MAX_SECS")? {
            c.chat_max = Duration::from_secs(n);
        }
        if let Some(n) = secs("ENZARB_CHAT_RETENTION_DAYS")? {
            c.chat_retention = Duration::from_secs(n.saturating_mul(86400));
        }
        // The values are not echoed in errors: a misplaced secret must not end
        // up in the logs.
        if let Some(v) = get("ENZARB_CHAT_ALLOW_BYPASS") {
            c.chat_allow_bypass = match v.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => anyhow::bail!("ENZARB_CHAT_ALLOW_BYPASS: must be true or false"),
            };
        }
        c.chat_permission_mode = match get("ENZARB_CHAT_PERMISSION_MODE") {
            Some(v) => ChatMode::parse(v.trim()).ok_or_else(|| {
                anyhow::anyhow!(
                    "ENZARB_CHAT_PERMISSION_MODE: must be bypassPermissions, default or plan"
                )
            })?,
            None if c.chat_allow_bypass => ChatMode::Bypass,
            None => ChatMode::Default,
        };
        if c.chat_permission_mode == ChatMode::Bypass && !c.chat_allow_bypass {
            anyhow::bail!(
                "ENZARB_CHAT_PERMISSION_MODE=bypassPermissions needs ENZARB_CHAT_ALLOW_BYPASS=true"
            );
        }
        if let Some(v) = get("ENZARB_SHELL")
            .or_else(|| get("SHELL"))
            .filter(|s| !s.is_empty())
        {
            c.shell = v;
        }
        if let Some(v) = get("ENZARB_GRACE_SECS") {
            let secs: u64 = v
                .parse()
                .map_err(|e| anyhow::anyhow!("ENZARB_GRACE_SECS={v:?}: {e}"))?;
            c.grace = Duration::from_secs(secs);
        }
        c.dockerd = DockerdConfig::from_lookup(&get)?;
        c.docker_prune = PruneConfig::from_lookup(&get, &c.dockerd)?;
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert_eq!(c.listen.port(), 7070);
        assert_eq!(
            c.auth_token_file,
            PathBuf::from("/var/run/enzarb/auth/token")
        );
        let c = Config::from_lookup(|k| match k {
            "ENZARB_AUTH_TOKEN_FILE" => Some("/t".into()),
            "ENZARB_GRACE_SECS" => Some("3".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.auth_token_file, PathBuf::from("/t"));
        assert_eq!(c.grace, Duration::from_secs(3));
        assert!(Config::from_lookup(|_| Some("x".into())).is_err());
    }

    #[test]
    fn step_settings() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert_eq!(
            c.chat_runtime_tool,
            "npm:@agentclientprotocol/claude-agent-acp"
        );
        let c = Config::from_lookup(|k| match k {
            "ENZARB_CHAT_RUNTIME_TOOL" => Some(String::new()),
            _ => None,
        })
        .unwrap();
        assert!(c.chat_runtime_tool.is_empty());
    }

    #[test]
    fn chat_settings() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert_eq!(c.max_chats, 1);
        assert_eq!(c.chat_idle, Duration::from_secs(1800));
        assert_eq!(c.chat_max, Duration::from_secs(8 * 3600));
        assert_eq!(c.chat_retention, Duration::from_secs(7 * 86400));
        let c = Config::from_lookup(|k| match k {
            "ENZARB_MAX_CHATS" => Some("0".into()),
            "ENZARB_CHAT_IDLE_SECS" => Some("5".into()),
            "ENZARB_CHAT_MAX_SECS" => Some("60".into()),
            "ENZARB_CHAT_RETENTION_DAYS" => Some("1".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.max_chats, 0);
        assert_eq!(c.chat_idle, Duration::from_secs(5));
        assert_eq!(c.chat_max, Duration::from_secs(60));
        assert_eq!(c.chat_retention, Duration::from_secs(86400));
        for k in [
            "ENZARB_CHAT_IDLE_SECS",
            "ENZARB_CHAT_MAX_SECS",
            "ENZARB_MAX_CHATS",
        ] {
            assert!(Config::from_lookup(|q| (q == k).then(|| "x".into())).is_err());
        }
        assert!(
            Config::from_lookup(|q| (q == "ENZARB_CHAT_IDLE_SECS").then(|| "0".into())).is_err()
        );
    }

    #[test]
    fn chat_permission_mode() {
        let mode = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            Config::from_lookup(move |k| vars.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone()))
                .map(|c| (c.chat_permission_mode, c.chat_allow_bypass))
        };
        assert_eq!(mode(&[]).unwrap(), (ChatMode::Bypass, true));
        let m = "ENZARB_CHAT_PERMISSION_MODE";
        let a = "ENZARB_CHAT_ALLOW_BYPASS";
        assert_eq!(mode(&[(m, "default")]).unwrap(), (ChatMode::Default, true));
        assert_eq!(mode(&[(m, "plan")]).unwrap(), (ChatMode::Plan, true));
        assert_eq!(
            mode(&[(m, "bypassPermissions")]).unwrap(),
            (ChatMode::Bypass, true)
        );
        assert_eq!(mode(&[(a, "false")]).unwrap(), (ChatMode::Default, false));
        assert_eq!(
            mode(&[(a, "false"), (m, "plan")]).unwrap(),
            (ChatMode::Plan, false)
        );
        assert!(mode(&[(a, "false"), (m, "bypassPermissions")]).is_err());
        assert!(mode(&[(a, "maybe")]).is_err());
        for bad in [
            "acceptEdits",
            "auto",
            "dontAsk",
            "bypass",
            "BYPASSPERMISSIONS",
            "",
        ] {
            assert!(mode(&[(m, bad)]).is_err(), "{bad}");
        }
        // the error does not echo the value
        let e = mode(&[(m, "sk-ant-oat01-secret")]).unwrap_err().to_string();
        assert!(!e.contains("secret"), "{e}");
    }

    #[test]
    fn docker_prune_settings() {
        let c = Config::from_lookup(|_| None).unwrap().docker_prune;
        assert!(c.enabled);
        assert_eq!(c.age, "24h");
        assert_eq!(c.min_free_percent, 15);
        let c = Config::from_lookup(|k| match k {
            "ENZARB_DOCKER_PRUNE_AGE" => Some("6h".into()),
            "ENZARB_DOCKER_PRUNE_MIN_FREE" => Some("25%".into()),
            "ENZARB_DOCKERD_DATA_ROOT" => Some("/data/docker".into()),
            _ => None,
        })
        .unwrap()
        .docker_prune;
        assert_eq!((c.age.as_str(), c.min_free_percent), ("6h", 25));
        assert_eq!(c.data_root, std::path::PathBuf::from("/data/docker"));
        assert!(
            Config::from_lookup(|k| (k == "ENZARB_DOCKER_PRUNE_AGE").then(|| "soon".into()))
                .is_err()
        );
    }

    #[test]
    fn dockerd_settings() {
        let c = Config::from_lookup(|_| None).unwrap().dockerd;
        assert!(c.enabled);
        assert_eq!(c.bin, "dockerd");
        let a = c.command_args();
        assert_eq!(a[0], "--host=unix:///var/run/docker.sock");
        assert!(a.contains(&"--group=1000".to_string()));
        assert!(a.contains(&"--data-root=/var/lib/docker".to_string()));
        assert!(a.contains(&"--iptables=false".to_string()));
        assert!(a.contains(&"--storage-driver=overlay2".to_string()));
        assert!(!a.contains(&"--storage-driver=vfs".to_string()));
        let c = Config::from_lookup(|k| match k {
            "ENZARB_DOCKERD" => Some("off".into()),
            "ENZARB_DOCKERD_ARGS" => Some("--storage-driver=overlay2  --debug".into()),
            "ENZARB_DOCKERD_SOCKET" => Some("/run/d.sock".into()),
            _ => None,
        })
        .unwrap()
        .dockerd;
        assert!(!c.enabled);
        let a = c.command_args();
        assert_eq!(a[0], "--host=unix:///run/d.sock");
        assert_eq!(&a[a.len() - 2..], ["--storage-driver=overlay2", "--debug"]);
        assert!(!a.contains(&"--iptables=false".to_string()));
        assert!(Config::from_lookup(|k| (k == "ENZARB_DOCKERD").then(|| "maybe".into())).is_err());
    }

    #[test]
    fn acp_settings() {
        let c = Config::from_lookup(|_| None).unwrap();
        assert_eq!(c.acp_bin, "claude-agent-acp");
        assert_eq!(c.cancel_grace, Duration::from_secs(10));
        let c = Config::from_lookup(|k| match k {
            "ENZARB_ACP_AGENT_BIN" => Some("/opt/acp".into()),
            "ENZARB_CANCEL_GRACE_SECS" => Some("3".into()),
            "ENZARB_PERMISSION_POLICY" => Some("/nope.yaml".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.acp_bin, "/opt/acp");
        assert_eq!(c.cancel_grace, Duration::from_secs(3));
    }
}
