//! Permission policy for agent tool calls (`session/request_permission`).
//!
//! The policy is data: a [`Policy`] with a built-in default, overridable by a
//! YAML or JSON file (`ENZARB_PERMISSION_POLICY`). [`Policy::decide`] turns a
//! permission request's `toolCall` (kind, title, `rawInput`, locations) into an
//! allow/deny [`Decision`]; everything that is not explicitly allowed is
//! denied.
//!
//! * file reads, edits and searches must stay inside `read_roots` /
//!   `edit_roots` (`{worktree}` stands for the run's worktree); paths are
//!   normalised lexically and through symlinks of their existing prefix;
//! * shell commands are parsed (quotes, `;`, `&&`, `||`, `|`, `&`, newlines,
//!   redirections) and every simple command must match a [`CommandRule`];
//!   command substitution, variable expansion, subshells and heredocs are
//!   rejected because their effect cannot be judged statically;
//! * `fetch` tools are allowed only for hosts in `fetch_allow_hosts`.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How the non-flag arguments of a command are treated.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathMode {
    /// Arguments are not paths.
    #[default]
    None,
    /// Path-like arguments must be inside `read_roots`.
    Read,
    /// Path-like arguments must be inside `edit_roots`.
    Write,
}

/// One allowed program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRule {
    /// Program name as typed (bare word, no `/`).
    pub name: String,
    #[serde(default)]
    pub paths: PathMode,
    /// If set, the subcommand (first argument) must be one of these.
    #[serde(default)]
    pub allow_subcommands: Option<Vec<String>>,
    /// Subcommands that are refused.
    #[serde(default)]
    pub deny_subcommands: Vec<String>,
    /// Arguments that are refused (`--flag` also matches `--flag=value`).
    #[serde(default)]
    pub deny_args: Vec<String>,
    /// Options accepted before the subcommand (e.g. git's `--no-pager`).
    #[serde(default)]
    pub global_flags: Vec<String>,
}

impl CommandRule {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            paths: PathMode::None,
            allow_subcommands: None,
            deny_subcommands: Vec::new(),
            deny_args: Vec::new(),
            global_flags: Vec::new(),
        }
    }
    fn paths(mut self, m: PathMode) -> Self {
        self.paths = m;
        self
    }
    fn deny_sub(mut self, subs: &[&str]) -> Self {
        self.deny_subcommands = subs.iter().map(|s| s.to_string()).collect();
        self
    }
    fn deny_args(mut self, args: &[&str]) -> Self {
        self.deny_args = args.iter().map(|s| s.to_string()).collect();
        self
    }
    fn allow_sub(mut self, subs: &[&str]) -> Self {
        self.allow_subcommands = Some(subs.iter().map(|s| s.to_string()).collect());
        self
    }
    fn global(mut self, flags: &[&str]) -> Self {
        self.global_flags = flags.iter().map(|s| s.to_string()).collect();
        self
    }
}

/// The permission policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// Directories the agent may read (`{worktree}` = the run's worktree).
    pub read_roots: Vec<String>,
    /// Directories the agent may modify, by edit tools and shell commands.
    pub edit_roots: Vec<String>,
    /// Tool kinds allowed without further checks (`think`: subagents, todos).
    pub allow_kinds: Vec<String>,
    /// Hosts the `fetch` tools may reach (exact or `*.suffix`). Empty: none.
    pub fetch_allow_hosts: Vec<String>,
    /// Allowed programs in shell commands.
    pub commands: Vec<CommandRule>,
    /// Environment variable names (or `PREFIX_*`) a command may not set inline.
    pub deny_env: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        use PathMode::{Read, Write};
        let mut commands = vec![
            CommandRule::new("git")
                .deny_sub(&[
                    "push",
                    "send-pack",
                    "config",
                    "credential",
                    "credential-store",
                    "daemon",
                    "upload-pack",
                    "receive-pack",
                ])
                .deny_args(&["--upload-pack", "--receive-pack", "--exec"])
                .global(&["--no-pager", "-P", "--no-optional-locks", "--version"]),
            CommandRule::new("cargo")
                .deny_sub(&["publish", "login", "logout", "owner", "yank", "token"])
                .global(&["--version", "-V", "--help", "-h", "--list"]),
            CommandRule::new("npm")
                .deny_sub(&[
                    "publish",
                    "unpublish",
                    "login",
                    "logout",
                    "adduser",
                    "token",
                    "owner",
                    "access",
                    "config",
                    "set",
                    "deprecate",
                    "dist-tag",
                    "team",
                    "org",
                    "profile",
                    "hook",
                    "star",
                ])
                .global(&["--version", "-v", "--help", "-h"]),
            CommandRule::new("npx").global(&["--version", "-v", "--help", "-h"]),
            CommandRule::new("node").global(&["--version", "-v", "--help", "-h"]),
            CommandRule::new("rustc").global(&["--version", "-V", "--help", "-h"]),
            // Docker talks to the sandbox's own dockerd sidecar (ADR-0012). Extra checks in `check_docker`.
            CommandRule::new("docker")
                .allow_sub(&[
                    "run",
                    "create",
                    "exec",
                    "start",
                    "stop",
                    "restart",
                    "kill",
                    "rm",
                    "ps",
                    "logs",
                    "inspect",
                    "top",
                    "stats",
                    "port",
                    "wait",
                    "cp",
                    "diff",
                    "commit",
                    "attach",
                    "pause",
                    "unpause",
                    "rename",
                    "update",
                    "build",
                    "buildx",
                    "builder",
                    "compose",
                    "images",
                    "image",
                    "container",
                    "network",
                    "volume",
                    "pull",
                    "rmi",
                    "tag",
                    "history",
                    "version",
                    "info",
                    "events",
                    "system",
                ])
                .global(&["--version", "-v", "--help", "-h"]),
            // Offline chart/manifest checks only: nothing that talks to a cluster (ADR-0013).
            CommandRule::new("helm")
                .paths(Write)
                .allow_sub(&[
                    "lint",
                    "template",
                    "dependency",
                    "show",
                    "package",
                    "version",
                ])
                .deny_args(&[
                    "--kubeconfig",
                    "--kube-context",
                    "--kube-apiserver",
                    "--kube-token",
                    "--kube-as-user",
                    "--kube-as-group",
                    "--post-renderer",
                    "--set-file",
                    "--repo",
                    "--keyring",
                    "--key",
                    "--validate",
                    "--dry-run",
                ])
                .global(&["--version", "-h", "--help"]),
            CommandRule::new("kubectl")
                .paths(Write)
                .allow_sub(&["kustomize"])
                .deny_args(&[
                    "--kubeconfig",
                    "--enable-helm",
                    "--enable-alpha-plugins",
                    "--helm-command",
                    "--load-restrictor",
                    "--mount",
                    "--network",
                    "--network-name",
                    "--as-current-user",
                ]),
            CommandRule::new("make"),
            CommandRule::new("mise").global(&["--version", "-v", "--help", "-h"]),
            // Path-restricted file tools.
            CommandRule::new("find").paths(Read).deny_args(&[
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprint0", "-fprintf",
                "-fls",
            ]),
            CommandRule::new("rg").paths(Read).deny_args(&["--pre"]),
            CommandRule::new("tee").paths(Write),
        ];
        for n in [
            "ls", "cat", "head", "tail", "wc", "grep", "egrep", "fgrep", "sort", "uniq", "diff",
            "stat", "file", "du", "tree", "realpath", "readlink", "basename", "dirname", "cut",
            "tr", "nl", "column", "jq",
        ] {
            commands.push(CommandRule::new(n).paths(Read));
        }
        for n in ["mkdir", "touch", "cp", "mv", "rm", "rmdir"] {
            commands.push(CommandRule::new(n).paths(Write));
        }
        for n in [
            "pwd", "echo", "printf", "true", "false", "test", "[", "which", "date", "whoami",
            "uname", "sleep", "seq",
        ] {
            commands.push(CommandRule::new(n));
        }
        Self {
            read_roots: vec!["{worktree}".into()],
            edit_roots: vec!["{worktree}".into()],
            allow_kinds: vec!["think".into()],
            fetch_allow_hosts: Vec::new(),
            commands,
            deny_env: [
                "PATH",
                "HOME",
                "SHELL",
                "IFS",
                "ENV",
                "BASH_ENV",
                "LD_*",
                "DYLD_*",
                "GIT_*",
                "NODE_*",
                "NPM_*",
                "CARGO_*",
                "RUSTC_*",
                "RUSTFLAGS",
                "HELM_*",
                "KUBE*",
                "KUSTOMIZE_*",
                "XDG_*",
                "MISE_*",
                "CLAUDE_*",
                "ENZARB_*",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        }
    }
}

/// The verdict on one tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    pub allow: bool,
    /// Short stable name of the rule that decided (`shell`, `edit`, ...).
    pub rule: String,
    pub reason: String,
}

impl Decision {
    fn allow(rule: &str, reason: impl Into<String>) -> Self {
        Self {
            allow: true,
            rule: rule.into(),
            reason: reason.into(),
        }
    }
    fn deny(rule: &str, reason: impl Into<String>) -> Self {
        Self {
            allow: false,
            rule: rule.into(),
            reason: reason.into(),
        }
    }
}

impl Policy {
    /// Load the policy from `path` (`.json` is JSON, anything else YAML), or
    /// the built-in default when `path` is `None`. A file replaces only the
    /// fields it mentions.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("permission policy {}: {e}", path.display()))?;
        let policy = if path.extension().is_some_and(|e| e == "json") {
            serde_json::from_str(&text).map_err(anyhow::Error::from)
        } else {
            serde_yaml_ng::from_str(&text).map_err(anyhow::Error::from)
        };
        policy.map_err(|e| anyhow::anyhow!("permission policy {}: {e}", path.display()))
    }

    fn roots(list: &[String], worktree: &Path) -> Vec<PathBuf> {
        list.iter()
            .map(|r| {
                let r = r.replace("{worktree}", &worktree.to_string_lossy());
                canonical(Path::new(&r))
            })
            .collect()
    }

    /// Decide on a permission request's `toolCall` object (as JSON).
    pub fn decide(&self, tool_call: &Value, worktree: &Path) -> Decision {
        let kind = tool_call
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("other");
        if self.allow_kinds.iter().any(|k| k == kind) {
            return Decision::allow("kind", format!("{kind} tools are allowed"));
        }
        let input = tool_call.get("rawInput").unwrap_or(&Value::Null);
        let read = Self::roots(&self.read_roots, worktree);
        let edit = Self::roots(&self.edit_roots, worktree);
        let cwd = canonical(worktree);
        let locations: Vec<&str> = tool_call
            .get("locations")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.get("path").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        match kind {
            "execute" => {
                let Some(cmd) = input.get("command").and_then(Value::as_str) else {
                    return Decision::deny("shell", "no command in the tool input");
                };
                self.decide_shell(cmd, &cwd, &read, &edit)
            }
            "read" | "search" => {
                let mut paths = input_paths(input);
                paths.extend(locations.iter().map(|s| s.to_string()));
                if kind == "search" {
                    // Glob patterns may be absolute.
                    if let Some(p) = input.get("pattern").and_then(Value::as_str)
                        && p.starts_with('/')
                    {
                        paths.push(literal_prefix(p));
                    }
                    if paths.is_empty() {
                        return Decision::allow("search", "search in the worktree");
                    }
                } else if paths.is_empty() {
                    return Decision::deny("read", "no path in the tool input");
                }
                for p in &paths {
                    if !inside(&read, &cwd, p) {
                        return Decision::deny("read", format!("{p} is outside the read roots"));
                    }
                }
                Decision::allow(kind, "path inside the read roots")
            }
            "edit" | "delete" | "move" => {
                let mut paths = input_paths(input);
                paths.extend(locations.iter().map(|s| s.to_string()));
                if paths.is_empty() {
                    return Decision::deny("edit", "no path in the tool input");
                }
                for p in &paths {
                    if !inside(&edit, &cwd, p) {
                        return Decision::deny("edit", format!("{p} is outside the edit roots"));
                    }
                }
                Decision::allow("edit", "path inside the edit roots")
            }
            "fetch" => {
                let url = input.get("url").and_then(Value::as_str).unwrap_or_default();
                match url_host(url) {
                    Some(h) if host_allowed(&self.fetch_allow_hosts, &h) => {
                        Decision::allow("fetch", format!("host {h} is allowed"))
                    }
                    Some(h) => Decision::deny("fetch", format!("host {h} is not allowed")),
                    None => Decision::deny("fetch", "network fetch is not allowed"),
                }
            }
            other => Decision::deny("kind", format!("tool kind {other:?} is not allowed")),
        }
    }

    /// Decide on a shell command line.
    pub fn decide_shell(
        &self,
        command: &str,
        cwd: &Path,
        read: &[PathBuf],
        edit: &[PathBuf],
    ) -> Decision {
        let segments = match parse_shell(command) {
            Ok(s) => s,
            Err(e) => return Decision::deny("shell", e),
        };
        if segments.is_empty() {
            return Decision::deny("shell", "empty command");
        }
        let mut cwd = cwd.to_path_buf();
        for seg in &segments {
            for (name, _) in &seg.env {
                if self.env_denied(name) {
                    return Decision::deny("shell", format!("setting {name} is not allowed"));
                }
            }
            for r in &seg.redirs {
                let roots = if r.input { read } else { edit };
                if r.target == "/dev/null" {
                    continue;
                }
                if !inside(roots, &cwd, &r.target) {
                    return Decision::deny(
                        "shell",
                        format!("redirection to {} is outside the allowed roots", r.target),
                    );
                }
            }
            let Some(prog) = seg.words.first() else {
                if seg.env.is_empty() {
                    continue;
                }
                return Decision::deny("shell", "bare variable assignment is not allowed");
            };
            let args = &seg.words[1..];
            if prog == "cd" {
                let piped = seg.piped_before || seg.piped_after;
                let [dir] = args else {
                    return Decision::deny("shell", "cd needs exactly one directory");
                };
                if piped || !(inside(read, &cwd, dir) || inside(edit, &cwd, dir)) {
                    return Decision::deny("shell", format!("cd {dir} leaves the worktree"));
                }
                cwd = resolve(&cwd, dir).unwrap_or(cwd);
                continue;
            }
            if prog.contains('/') {
                return Decision::deny(
                    "shell",
                    format!("{prog}: programs must be named without a path"),
                );
            }
            let Some(rule) = self.commands.iter().find(|c| &c.name == prog) else {
                return Decision::deny("shell", format!("{prog} is not an allowed command"));
            };
            if let Err(why) = self.check_args(rule, args, &cwd, read, edit) {
                return Decision::deny("shell", format!("{prog}: {why}"));
            }
        }
        Decision::allow("shell", "every command matches the allowlist")
    }

    fn env_denied(&self, name: &str) -> bool {
        self.deny_env.iter().any(|d| match d.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == d,
        })
    }

    fn check_args(
        &self,
        rule: &CommandRule,
        args: &[String],
        cwd: &Path,
        read: &[PathBuf],
        edit: &[PathBuf],
    ) -> Result<(), String> {
        let mut rest = args;
        // rustup toolchain selector: `cargo +nightly ...`
        if rule.name == "cargo" && rest.first().is_some_and(|a| a.starts_with('+')) {
            rest = &rest[1..];
        }
        if rule.allow_subcommands.is_some() || !rule.deny_subcommands.is_empty() {
            match rest.first() {
                Some(first) if first.starts_with('-') => {
                    if !rule.global_flags.iter().any(|g| g == first) {
                        return Err(format!("option {first} before the subcommand"));
                    }
                    // Skip accepted global flags to find the subcommand.
                    let mut i = 0;
                    while rest.get(i).is_some_and(|a| rule.global_flags.contains(a)) {
                        i += 1;
                    }
                    if let Some(sub) = rest.get(i) {
                        if sub.starts_with('-') {
                            return Err(format!("option {sub} before the subcommand"));
                        }
                        self.check_sub(rule, sub)?;
                    }
                }
                Some(sub) => self.check_sub(rule, sub)?,
                None => {}
            }
        }
        if (rule.name == "helm" || rule.name == "kubectl")
            && let Some(a) = rest.iter().find(|a| a.contains("://"))
        {
            return Err(format!("remote reference {a} is not allowed"));
        }
        if rule.name == "docker" {
            check_docker(rest, cwd, read)?;
        }
        let mut after_dd = false;
        for a in rest {
            if !after_dd && a == "--" {
                after_dd = true;
                continue;
            }
            if !after_dd {
                for d in &rule.deny_args {
                    if a == d
                        || a.strip_prefix(d.as_str())
                            .is_some_and(|r| r.starts_with('='))
                    {
                        return Err(format!("argument {a} is not allowed"));
                    }
                }
            }
            if rule.paths == PathMode::None {
                continue;
            }
            let roots = if rule.paths == PathMode::Read {
                read
            } else {
                edit
            };
            // `--flag=value`: judge the value.
            let candidate = if !after_dd && a.starts_with('-') {
                match a.split_once('=') {
                    Some((_, v)) => v,
                    None => continue,
                }
            } else {
                a.as_str()
            };
            if path_like(candidate) && !inside(roots, cwd, candidate) {
                return Err(format!("path {candidate} is outside the allowed roots"));
            }
        }
        Ok(())
    }

    fn check_sub(&self, rule: &CommandRule, sub: &str) -> Result<(), String> {
        if rule.deny_subcommands.iter().any(|d| d == sub) {
            return Err(format!("subcommand {sub} is not allowed"));
        }
        if let Some(allow) = &rule.allow_subcommands
            && !allow.iter().any(|a| a == sub)
        {
            return Err(format!("subcommand {sub} is not allowed"));
        }
        Ok(())
    }
}

/// Flags that hand the container host privileges. Refused with any value.
const DOCKER_DENY_FLAGS: &[&str] = &[
    "--privileged",
    "--cap-add",
    "--device",
    "--device-cgroup-rule",
    "--security-opt",
    "--allow",
    "--push",
    "--secret",
    "--ssh",
];
/// Namespace flags; only the `host` value escapes.
const DOCKER_HOST_NS_FLAGS: &[&str] = &["--pid", "--ipc", "--uts", "--userns", "--cgroupns"];

/// Extra rules for `docker`, on top of the allowlist/subcommand checks. The
/// daemon is the sandbox's own dockerd (inside gVisor), so `--network=host`
/// is fine; what must not happen is a container reaching the sandbox pod or
/// the node: privileged mode, host namespaces, extra capabilities, devices,
/// security options, and bind mounts from outside the worktree (which includes
/// the docker socket). Registry login/push are refused (no credentials leave).
fn check_docker(args: &[String], cwd: &Path, roots: &[PathBuf]) -> Result<(), String> {
    // The subcommand path: leading non-flag words (`image push`, `compose push`).
    let mut words = args.iter().filter(|a| !a.starts_with('-'));
    let first = words.next().map(String::as_str);
    let second = words.next().map(String::as_str);
    for w in [first, second].into_iter().flatten() {
        if matches!(w, "login" | "logout" | "push") {
            return Err(format!("{w} is not allowed"));
        }
    }
    if matches!(
        first,
        Some("context" | "plugin" | "manifest" | "trust" | "swarm")
    ) {
        return Err(format!("{} is not allowed", first.unwrap_or_default()));
    }
    let is_build = matches!(first, Some("build"))
        || (matches!(first, Some("buildx" | "builder")) && second == Some("build"));
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        // `--flag=value` or `--flag value`.
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if a.starts_with("--") => (f, Some(v)),
            _ => (a, None),
        };
        let mut value = || -> Option<&str> {
            inline.or_else(|| {
                i += 1;
                args.get(i).map(String::as_str)
            })
        };
        if DOCKER_DENY_FLAGS.contains(&flag) {
            return Err(format!("{flag} is not allowed"));
        }
        // Never retarget another daemon or client config.
        if matches!(flag, "-H" | "--host" | "--context" | "--config") {
            return Err(format!("{flag} is not allowed"));
        }
        if DOCKER_HOST_NS_FLAGS.contains(&flag) {
            if value().is_some_and(|v| v == "host") {
                return Err(format!("{flag}=host is not allowed"));
            }
        } else if flag.starts_with("-v") && !flag.starts_with("--") && flag.len() > 2 {
            // `-v/host:/path` glued form.
            return Err("write -v and its value as separate words".into());
        } else if matches!(flag, "-v" | "--volume") {
            if let Some(v) = value() {
                let src = v.split(':').next().unwrap_or_default();
                // `-v /path` alone is an anonymous volume.
                let bound = v.contains(':');
                if bound && (path_like(src) || src.starts_with('.')) && !inside(roots, cwd, src) {
                    return Err(format!("bind mount of {src} is outside the worktree"));
                }
            }
        } else if flag == "--mount" {
            if let Some(v) = value() {
                let mut bind = false;
                let mut src = None;
                for kv in v.split(',') {
                    match kv.split_once('=') {
                        Some(("type", t)) => bind = t == "bind",
                        Some(("source" | "src", s)) => src = Some(s),
                        _ => {}
                    }
                }
                // A bind mount with no source or an outside source is refused.
                if bind && !src.is_some_and(|s| inside(roots, cwd, s)) {
                    return Err("bind mount outside the worktree".into());
                }
            }
        } else if matches!(flag, "-f" | "--file") && (is_build || first == Some("compose")) {
            if let Some(v) = value()
                && v != "-"
                && !inside(roots, cwd, v)
            {
                return Err(format!("file {v} is outside the worktree"));
            }
        } else if matches!(flag, "-o" | "--output") && is_build {
            if let Some(v) = value() {
                let dest = v
                    .split(',')
                    .find_map(|kv| kv.strip_prefix("dest="))
                    .unwrap_or(v);
                if !v.contains("type=registry") && !inside(roots, cwd, dest) {
                    return Err(format!("output {dest} is outside the worktree"));
                }
                if v.contains("type=registry") {
                    return Err("pushing to a registry is not allowed".into());
                }
            }
        } else if is_build
            && !a.starts_with('-')
            && (a.starts_with(['/', '~'])
                || a == "."
                || a == ".."
                || a.starts_with("./")
                || a.starts_with("../"))
            && !inside(roots, cwd, a)
        {
            return Err(format!("build path {a} is outside the worktree"));
        }
        i += 1;
    }
    Ok(())
}

fn input_paths(input: &Value) -> Vec<String> {
    ["file_path", "path", "notebook_path", "old_path", "new_path"]
        .iter()
        .filter_map(|k| input.get(*k).and_then(Value::as_str))
        .map(String::from)
        .collect()
}

/// `/a/b/*.rs` -> `/a/b`: the part of a glob before the first wildcard.
fn literal_prefix(pattern: &str) -> String {
    let cut = pattern.find(['*', '?', '[', '{']).unwrap_or(pattern.len());
    let head = &pattern[..cut];
    match head.rfind('/') {
        Some(0) => "/".into(),
        Some(i) if cut < pattern.len() => head[..i].to_string(),
        _ => head.to_string(),
    }
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = authority.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn host_allowed(allow: &[String], host: &str) -> bool {
    allow.iter().any(|a| match a.strip_prefix("*.") {
        Some(suffix) => host.ends_with(&format!(".{suffix}")),
        None => a.eq_ignore_ascii_case(host),
    })
}

/// Does an argument look like a path (as opposed to a pattern or a name)?
fn path_like(s: &str) -> bool {
    s.contains('/') || s == "." || s == ".." || s.starts_with('~')
}

/// Lexically normalise `p` (no `.`/`..`), without touching the filesystem.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Normalise and resolve symlinks of the longest existing prefix.
fn canonical(p: &Path) -> PathBuf {
    let p = normalize(p);
    let mut tail = Vec::new();
    let mut head = p.as_path();
    loop {
        if let Ok(c) = head.canonicalize() {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            return out;
        }
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                head = parent;
            }
            _ => return p,
        }
    }
}

/// `arg` resolved against `cwd`; `None` for `~` forms.
fn resolve(cwd: &Path, arg: &str) -> Option<PathBuf> {
    if arg.starts_with('~') {
        return None;
    }
    let p = Path::new(arg);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    Some(canonical(&abs))
}

fn inside(roots: &[PathBuf], cwd: &Path, arg: &str) -> bool {
    let Some(p) = resolve(cwd, arg) else {
        return false;
    };
    roots.iter().any(|r| p.starts_with(r))
}

// ---- shell parsing ----

#[derive(Debug, Default, PartialEq, Eq)]
struct Redir {
    target: String,
    input: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Segment {
    env: Vec<(String, String)>,
    words: Vec<String>,
    redirs: Vec<Redir>,
    piped_before: bool,
    piped_after: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Tok {
    Word(String),
    /// `;`, `&&`, `||`, `&`, newline
    Sep,
    Pipe,
    /// `>` / `>>` (the target is the next word)
    Out,
    /// `<`
    In,
    /// `2>&1`, `>&2`: nothing to check
    Dup,
}

fn is_assignment(w: &str) -> Option<(&str, &str)> {
    let (n, v) = w.split_once('=')?;
    let ok = !n.is_empty()
        && !n.starts_with(|c: char| c.is_ascii_digit())
        && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    ok.then_some((n, v))
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = s.chars().collect();
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut has_word = false;
    let mut i = 0;
    macro_rules! flush {
        () => {
            if has_word {
                toks.push(Tok::Word(std::mem::take(&mut cur)));
                has_word = false;
            }
        };
    }
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => {
                flush!();
            }
            '\n' | ';' => {
                flush!();
                toks.push(Tok::Sep);
            }
            '\r' => {}
            '#' if !has_word => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '\\' => {
                i += 1;
                match chars.get(i) {
                    Some('\n') => {}
                    Some(&n) => {
                        cur.push(n);
                        has_word = true;
                    }
                    None => return Err("trailing backslash".into()),
                }
            }
            '\'' => {
                has_word = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('\'') => break,
                        Some(&n) => cur.push(n),
                        None => return Err("unterminated quote".into()),
                    }
                    i += 1;
                }
            }
            '"' => {
                has_word = true;
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('"') => break,
                        Some('\\') => match chars.get(i + 1) {
                            Some(&n @ ('"' | '\\' | '$' | '`')) => {
                                cur.push(n);
                                i += 1;
                            }
                            Some('\n') => i += 1,
                            _ => cur.push('\\'),
                        },
                        Some('$' | '`') => {
                            return Err("shell expansion ($ or backtick) is not allowed".into());
                        }
                        Some(&n) => cur.push(n),
                        None => return Err("unterminated quote".into()),
                    }
                    i += 1;
                }
            }
            '$' | '`' => return Err("shell expansion ($ or backtick) is not allowed".into()),
            '(' | ')' => return Err("subshells and groups are not allowed".into()),
            '{' | '}' if !has_word => {
                return Err("subshells and groups are not allowed".into());
            }
            '|' => {
                flush!();
                if chars.get(i + 1) == Some(&'|') {
                    i += 1;
                    toks.push(Tok::Sep);
                } else {
                    toks.push(Tok::Pipe);
                }
            }
            '&' => {
                if chars.get(i + 1) == Some(&'>') {
                    // `&>file` / `&>>file`
                    flush!();
                    i += 1;
                    if chars.get(i + 1) == Some(&'>') {
                        i += 1;
                    }
                    toks.push(Tok::Out);
                } else {
                    flush!();
                    if chars.get(i + 1) == Some(&'&') {
                        i += 1;
                    }
                    toks.push(Tok::Sep);
                }
            }
            '>' => {
                // `2>`: the fd digits belong to the operator.
                if has_word && cur.chars().all(|c| c.is_ascii_digit()) {
                    cur.clear();
                    has_word = false;
                } else {
                    flush!();
                }
                if chars.get(i + 1) == Some(&'>') {
                    i += 1;
                }
                if chars.get(i + 1) == Some(&'&') {
                    // `>&2`, `>&-`
                    i += 1;
                    let mut j = i + 1;
                    while chars
                        .get(j)
                        .is_some_and(|c| c.is_ascii_digit() || *c == '-')
                    {
                        j += 1;
                    }
                    if j == i + 1 {
                        return Err("unsupported redirection".into());
                    }
                    i = j - 1;
                    toks.push(Tok::Dup);
                } else {
                    if chars.get(i + 1) == Some(&'|') {
                        i += 1;
                    }
                    toks.push(Tok::Out);
                }
            }
            '<' => {
                if matches!(chars.get(i + 1), Some('<' | '(' | '&')) {
                    return Err("heredocs and process substitution are not allowed".into());
                }
                if has_word && cur.chars().all(|c| c.is_ascii_digit()) {
                    cur.clear();
                    has_word = false;
                } else {
                    flush!();
                }
                toks.push(Tok::In);
            }
            other => {
                cur.push(other);
                has_word = true;
            }
        }
        i += 1;
    }
    if has_word {
        toks.push(Tok::Word(cur));
    }
    Ok(toks)
}

fn parse_shell(s: &str) -> Result<Vec<Segment>, String> {
    let toks = tokenize(s)?;
    let mut segs: Vec<Segment> = Vec::new();
    let mut cur = Segment::default();
    let mut pending: Option<bool> = None; // Some(input?) while awaiting a redirect target
    let mut piped_before = false;
    let finish = |cur: &mut Segment, segs: &mut Vec<Segment>| {
        let seg = std::mem::take(cur);
        if !seg.words.is_empty() || !seg.env.is_empty() || !seg.redirs.is_empty() {
            segs.push(seg);
        }
    };
    for t in toks {
        match t {
            Tok::Word(w) => {
                if let Some(input) = pending.take() {
                    cur.redirs.push(Redir { target: w, input });
                } else if cur.words.is_empty()
                    && let Some((n, v)) = is_assignment(&w)
                {
                    cur.env.push((n.to_string(), v.to_string()));
                } else {
                    cur.words.push(w);
                }
            }
            Tok::Out => pending = Some(false),
            Tok::In => pending = Some(true),
            Tok::Dup => {}
            Tok::Sep | Tok::Pipe => {
                if pending.is_some() {
                    return Err("redirection without a target".into());
                }
                let is_pipe = t == Tok::Pipe;
                cur.piped_before = piped_before;
                cur.piped_after = is_pipe;
                piped_before = is_pipe;
                finish(&mut cur, &mut segs);
            }
        }
    }
    if pending.is_some() {
        return Err("redirection without a target".into());
    }
    cur.piped_before = piped_before;
    finish(&mut cur, &mut segs);
    Ok(segs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("worktrees/run-1");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(wt.join("src/lib.rs"), "").unwrap();
        std::fs::create_dir_all(d.path().join(".enzarb")).unwrap();
        std::fs::write(d.path().join(".enzarb/claude-token"), "x").unwrap();
        let wt = wt.canonicalize().unwrap();
        (d, wt)
    }

    fn sh(p: &Policy, wt: &Path, cmd: &str) -> Decision {
        p.decide(
            &json!({"kind": "execute", "rawInput": {"command": cmd}}),
            wt,
        )
    }

    #[track_caller]
    fn allowed(cmd: &str) {
        let (_d, wt) = setup();
        let d = sh(&Policy::default(), &wt, cmd);
        assert!(d.allow, "{cmd:?} should be allowed: {d:?}");
    }

    #[track_caller]
    fn denied(cmd: &str) {
        let (_d, wt) = setup();
        let d = sh(&Policy::default(), &wt, cmd);
        assert!(!d.allow, "{cmd:?} should be denied: {d:?}");
    }

    #[test]
    fn allowlisted_commands() {
        for c in [
            "git status",
            "git diff --stat HEAD~1",
            "git log --oneline -n 5",
            "git add -A && git commit -m \"fix: a b\"",
            "git switch -c enzarb/x",
            "git --no-pager log",
            "git fetch origin",
            "cargo test --all-targets -- --nocapture",
            "cargo +nightly fmt --check",
            "cargo --version",
            "npm ci && npm test",
            "npm run build",
            "npx prettier --check .",
            "node -e 'console.log(1)'",
            "rustc --version",
            "make -j4 check",
            "mise run test",
            "mise install",
            "cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test",
            "npm run check && npm run lint && npm test",
            "helm lint enzarb",
            "helm template enzarb/ --set a=b",
            "kubectl kustomize overlays/kind",
            "ls -la src",
            "cat src/lib.rs | head -n 20 | wc -l",
            "grep -rn 'TODO' src > /dev/null 2>&1",
            "rg --files",
            "find . -name '*.rs'",
            "echo hello; pwd",
            "cd src && ls",
            "mkdir -p target/x && touch target/x/y",
            "rm -rf target",
            "cat src/lib.rs 2>&1",
            "echo hi > out.txt",
            "echo hi >> src/out.txt",
            "cargo build &",
            "git status\ngit diff",
            "ls # a comment",
            "CI=1 npm test",
            "test -f src/lib.rs && echo yes",
            "cp src/lib.rs src/lib2.rs",
            "grep -n foo src/lib.rs",
        ] {
            allowed(c);
        }
    }

    #[test]
    fn cluster_access_via_helm_and_kubectl_is_denied() {
        for c in [
            "helm install x enzarb",
            "helm upgrade x enzarb",
            "helm uninstall x",
            "helm lint enzarb --kubeconfig /etc/k",
            "helm template x enzarb --validate",
            "helm template x enzarb --post-renderer ./evil",
            "helm lint --kube-context prod enzarb",
            "kubectl get pods",
            "kubectl apply -f x.yaml",
            "kubectl exec p -- sh",
            "kubectl kustomize --enable-helm .",
            "kubectl kustomize . --kubeconfig /x",
            "kubectl kustomize --enable-alpha-plugins .",
            "kubectl kustomize /etc",
            "kubectl kustomize https://github.com/x/y",
            "helm template x . --set-file k=/workspace/.enzarb/claude-token",
            "helm template x . -f /etc/passwd",
            "helm template x . --output-dir /tmp/o",
            "helm template oci://example.com/c",
            "KUBECONFIG=/x helm lint .",
            "HELM_KUBEAPISERVER=x helm lint .",
        ] {
            denied(c);
        }
    }

    #[test]
    fn docker_is_allowed_inside_the_worktree() {
        for c in [
            "docker --version",
            "docker ps -a",
            "docker build -t app:dev .",
            "docker build -f Dockerfile.dev -t ghcr.io/x/y .",
            "docker build -o type=local,dest=out .",
            "docker buildx build --platform linux/amd64 -t app .",
            "docker compose up -d",
            "docker compose -f compose.yaml up --build",
            "docker compose logs -f web",
            "docker logs -f web",
            "docker run --rm -it ubuntu:24.04 bash",
            "docker run --rm --network=host app:dev",
            "docker run --rm --network host app:dev",
            "docker run -v ./data:/data app",
            "docker run -v data:/data app",
            "docker run -v /var/lib/x app",
            "docker run -v src:/src:ro app",
            "docker run --mount type=bind,source=./src,target=/src app",
            "docker run --mount type=volume,source=data,target=/d app",
            "docker run --pid=container:abc app",
            "docker exec web ls /etc",
            "docker image ls && docker network ls",
            "docker pull ubuntu:24.04",
            "docker system prune -f",
        ] {
            allowed(c);
        }
    }

    #[test]
    fn docker_host_escapes_are_denied() {
        for c in [
            "docker run --privileged app",
            "docker run --privileged=true app",
            "docker exec --privileged web sh",
            "docker run --pid=host app",
            "docker run --pid host app",
            "docker run --ipc=host app",
            "docker run --uts=host app",
            "docker run --userns=host app",
            "docker run --cgroupns=host app",
            "docker run --cap-add=SYS_ADMIN app",
            "docker run --cap-add SYS_ADMIN app",
            "docker run --device=/dev/fuse app",
            "docker run --device /dev/kvm app",
            "docker run --security-opt seccomp=unconfined app",
            "docker run -v /var/run/docker.sock:/var/run/docker.sock app",
            "docker run --volume=/:/host app",
            "docker run -v /etc:/etc:ro app",
            "docker run -v ../other:/x app",
            "docker run -v~/x:/x app",
            "docker run --mount type=bind,source=/,target=/host app",
            "docker run --mount type=bind,src=/var/run/docker.sock,dst=/s app",
            "docker run --mount type=bind,target=/host app",
            "docker run -H tcp://evil:2375 ps",
            "docker --host=unix:///x ps",
            "docker --context other ps",
            "docker login",
            "docker logout",
            "docker push app:dev",
            "docker image push app:dev",
            "docker compose push",
            "docker build --push -t app .",
            "docker buildx build --push -t app .",
            "docker buildx build -o type=registry,ref=x .",
            "docker build /etc",
            "docker build -f /etc/passwd .",
            "docker build -o /etc .",
            "docker build --secret id=a,src=/etc/shadow .",
            "docker buildx build --allow security.insecure .",
            "docker compose -f /etc/c.yaml up",
            "docker context use x",
            "docker plugin install x",
            "docker swarm init",
            "docker unknown",
            "docker run app $(id)",
        ] {
            denied(c);
        }
    }

    #[test]
    fn git_push_and_dangerous_git_are_denied() {
        for c in [
            "git push",
            "git push origin HEAD",
            "git status && git push",
            "git --no-pager push",
            "git -c core.sshCommand=evil fetch",
            "git -C /etc status",
            "git config user.name x",
            "git fetch --upload-pack=evil",
            "git fetch --upload-pack evil origin",
            "git credential fill",
        ] {
            denied(c);
        }
    }

    #[test]
    fn unlisted_and_network_tools_are_denied() {
        for c in [
            "curl https://example.com",
            "wget http://x",
            "ssh host",
            "nc -l 1",
            "python3 -c 'print(1)'",
            "bash -c 'ls'",
            "sh script.sh",
            "sudo ls",
            "sed -i s/a/b/ f",
            "awk 'BEGIN{system(\"id\")}'",
            "xargs rm",
            "eval ls",
            "source x",
            "./run.sh",
            "/bin/ls",
            "target/debug/x",
            "env FOO=bar ls",
            "env",
            "cargo publish",
            "cargo login",
            "npm publish",
            "npm config set x y",
            "npm --prefix /etc install",
        ] {
            denied(c);
        }
    }

    #[test]
    fn shell_tricks_are_denied() {
        for c in [
            "ls $(id)",
            "ls `id`",
            "echo $HOME",
            "echo \"$HOME\"",
            "cat <(ls)",
            "cat <<EOF\nx\nEOF",
            "(ls)",
            "{ ls; }",
            "ls; curl x",
            "ls && curl x",
            "ls || curl x",
            "ls | curl x",
            "ls & curl x",
            "ls\ncurl x",
            "echo x > /etc/passwd",
            "echo x > ../../escape",
            "echo x >> ~/.bashrc",
            "cat < /etc/passwd",
            "PATH=/tmp ls",
            "LD_PRELOAD=/x.so ls",
            "GIT_SSH_COMMAND=x git fetch",
            "NODE_OPTIONS=--require=x node a.js",
            "FOO=bar",
            "echo 'unterminated",
            "ls >",
            "find . -exec rm {} ;",
            "find . -delete",
            "rg --pre cmd foo",
            "",
            "   ",
        ] {
            denied(c);
        }
    }

    #[test]
    fn quoting_hides_nothing() {
        // Quoted operators are plain text.
        allowed("echo 'a; curl x' \"b && c\"");
        allowed("grep 'foo|bar' src/lib.rs");
        // ... but the program position is still checked after unquoting.
        denied("'curl' x");
        denied("cu\\rl x");
    }

    #[test]
    fn path_containment_for_commands() {
        denied("cat /etc/passwd");
        denied("cat ../../.enzarb/claude-token");
        denied("cat ../../../.enzarb/claude-token");
        denied("ls /");
        denied("ls ~");
        denied("grep -r x /workspace");
        denied("rm -rf /");
        denied("rm ../x");
        denied("touch /tmp/x");
        denied("cp src/lib.rs /tmp/x");
        denied("mv a ../b");
        denied("cd .. && ls");
        denied("cd /etc && cat passwd");
        denied("cat --file=/etc/passwd");
        denied("find / -name x");
        denied("tee /tmp/x");
        denied("tail -n 5 -- /etc/hosts");
        allowed("cat ./src/../src/lib.rs");
        allowed("cd src && cat lib.rs && cd .. && ls");
        allowed("tail -n 5 -- src/lib.rs");
    }

    #[test]
    fn symlink_escape_is_denied() {
        let (d, wt) = setup();
        std::os::unix::fs::symlink(d.path().join(".enzarb"), wt.join("link")).unwrap();
        let p = Policy::default();
        assert!(!sh(&p, &wt, "cat link/claude-token").allow);
        let read = p.decide(
            &json!({"kind": "read", "rawInput": {"file_path": wt.join("link/claude-token")}}),
            &wt,
        );
        assert!(!read.allow, "{read:?}");
        let edit = p.decide(
            &json!({"kind": "edit", "rawInput": {"file_path": wt.join("link/new")}}),
            &wt,
        );
        assert!(!edit.allow, "{edit:?}");
    }

    #[test]
    fn file_tools() {
        let (d, wt) = setup();
        let p = Policy::default();
        let f = |kind: &str, input: Value| p.decide(&json!({"kind": kind, "rawInput": input}), &wt);
        let inside = wt.join("src/lib.rs");
        assert!(f("read", json!({"file_path": inside})).allow);
        assert!(f("read", json!({"file_path": "src/lib.rs"})).allow);
        assert!(f("edit", json!({"file_path": inside, "old_string": "a"})).allow);
        assert!(f("edit", json!({"file_path": wt.join("new/dir/file.rs")})).allow);
        assert!(f("edit", json!({"notebook_path": wt.join("a.ipynb")})).allow);
        // outside
        let outside = d.path().join(".enzarb/claude-token");
        assert!(!f("read", json!({"file_path": outside})).allow);
        assert!(!f("read", json!({"file_path": "/etc/passwd"})).allow);
        assert!(!f("edit", json!({"file_path": outside})).allow);
        assert!(!f("edit", json!({"file_path": "/etc/passwd"})).allow);
        assert!(!f("edit", json!({"file_path": "../../x"})).allow);
        assert!(!f("edit", json!({"file_path": "~/.bashrc"})).allow);
        assert!(!f("edit", json!({})).allow);
        assert!(!f("read", json!({})).allow);
        // sibling dir with the same prefix is not inside
        let sibling = format!("{}-evil/x", wt.display());
        assert!(!f("edit", json!({"file_path": sibling})).allow);
        // delete/move check every path
        assert!(!f("move", json!({"old_path": inside, "new_path": "/tmp/x"})).allow);
        // search
        assert!(f("search", json!({"pattern": "TODO"})).allow);
        assert!(f("search", json!({"pattern": "**/*.rs", "path": wt})).allow);
        assert!(!f("search", json!({"pattern": "x", "path": "/"})).allow);
        assert!(!f("search", json!({"pattern": "/etc/*.conf"})).allow);
        assert!(
            f(
                "search",
                json!({"pattern": format!("{}/src/*.rs", wt.display())})
            )
            .allow
        );
        // locations are checked too
        let d1 = p.decide(
            &json!({"kind": "edit", "rawInput": {"file_path": inside}, "locations": [{"path": "/etc/passwd"}]}),
            &wt,
        );
        assert!(!d1.allow);
    }

    #[test]
    fn fetch_and_other_kinds() {
        let (_d, wt) = setup();
        let p = Policy::default();
        let f = |kind: &str, input: Value| p.decide(&json!({"kind": kind, "rawInput": input}), &wt);
        assert!(!f("fetch", json!({"url": "https://example.com"})).allow);
        assert!(!f("fetch", json!({"query": "x"})).allow);
        assert!(f("think", json!({})).allow);
        assert!(!f("other", json!({})).allow);
        assert!(!f("switch_mode", json!({})).allow);
        assert!(!p.decide(&json!({}), &wt).allow);
        let p = Policy {
            fetch_allow_hosts: vec!["docs.rs".into(), "*.crates.io".into()],
            ..Policy::default()
        };
        let f = |url: &str| p.decide(&json!({"kind": "fetch", "rawInput": {"url": url}}), &wt);
        assert!(f("https://docs.rs/x").allow);
        assert!(f("https://static.crates.io/a").allow);
        assert!(!f("https://crates.io/a").allow);
        assert!(!f("https://docs.rs.evil.com/").allow);
        assert!(!f("https://evil.com/@docs.rs").allow);
        assert!(f("https://user@docs.rs:443/x").allow);
    }

    #[test]
    fn tokenizer_and_parser() {
        let segs = parse_shell("A=1 cmd 'a b' \"c d\" e\\ f 2>&1 > out | tail -n1; x").unwrap();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0].env, vec![("A".into(), "1".into())]);
        assert_eq!(segs[0].words, ["cmd", "a b", "c d", "e f"]);
        assert_eq!(
            segs[0].redirs,
            vec![Redir {
                target: "out".into(),
                input: false
            }]
        );
        assert!(segs[0].piped_after && segs[1].piped_before && !segs[1].piped_after);
        assert!(!segs[2].piped_before);
        assert_eq!(parse_shell("a &> f").unwrap()[0].redirs.len(), 1);
        assert_eq!(parse_shell("a >| f").unwrap()[0].redirs.len(), 1);
        assert!(parse_shell("a 2>&").is_err());
        assert!(parse_shell("a \\").is_err());
        assert!(parse_shell("a \"b").is_err());
    }

    #[test]
    fn glob_prefix() {
        assert_eq!(literal_prefix("/a/b/*.rs"), "/a/b");
        assert_eq!(literal_prefix("/*"), "/");
        assert_eq!(literal_prefix("/a/b"), "/a/b");
        assert_eq!(literal_prefix("/a/b*"), "/a");
    }

    #[test]
    fn load_policy_files() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(Policy::load(None).unwrap(), Policy::default());
        // YAML: partial override, other fields keep their defaults
        let y = d.path().join("p.yaml");
        std::fs::write(
            &y,
            "fetch_allow_hosts: [docs.rs]\ncommands:\n  - name: ls\n  - name: cat\n    paths: read\n",
        )
        .unwrap();
        let p = Policy::load(Some(&y)).unwrap();
        assert_eq!(p.fetch_allow_hosts, ["docs.rs"]);
        assert_eq!(p.commands.len(), 2);
        assert_eq!(p.read_roots, Policy::default().read_roots);
        // JSON
        let j = d.path().join("p.json");
        std::fs::write(&j, r#"{"allow_kinds": ["think", "other"]}"#).unwrap();
        let p = Policy::load(Some(&j)).unwrap();
        assert_eq!(p.allow_kinds, ["think", "other"]);
        assert_eq!(p.commands, Policy::default().commands);
        // errors
        std::fs::write(&y, "bogus_field: 1\n").unwrap();
        assert!(Policy::load(Some(&y)).is_err());
        assert!(Policy::load(Some(&d.path().join("missing.yaml"))).is_err());
        // the default round-trips through YAML
        let text = serde_yaml_ng::to_string(&Policy::default()).unwrap();
        std::fs::write(&y, text).unwrap();
        assert_eq!(Policy::load(Some(&y)).unwrap(), Policy::default());
    }
}
