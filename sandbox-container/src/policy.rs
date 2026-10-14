//! Permission policy for agent tool calls (`session/request_permission`).
//!
//! The policy is data: a [`Policy`] with a built-in default, overridable by a
//! YAML or JSON file (`ENZARB_PERMISSION_POLICY`). [`Policy::decide`] turns a
//! permission request's `toolCall` (kind, title, `rawInput`, locations) into an
//! allow/deny [`Decision`]; everything that is not explicitly allowed is
//! denied.
//!
//! * file reads, edits and searches must stay inside `read_roots` /
//!   `edit_roots` (`{worktree}` stands for the run's worktree, `{workspace}`
//!   for the workspace root, derived as `<workspace>/worktrees/<run>`'s
//!   grandparent); paths are normalised lexically and through symlinks of
//!   their existing prefix. Reads may also reach the Project's other repo
//!   checkouts and worktrees; writes never leave the run's worktree. Paths
//!   containing a `deny_read` component sequence (secrets) are never readable;
//! * shell commands are parsed (quotes, `;`, `&&`, `||`, `|`, `&`, newlines,
//!   redirections) and every simple command must match a [`CommandRule`];
//!   command substitution, variable expansion, subshells and heredocs are
//!   rejected because their effect cannot be judged statically;
//! * `fetch` tools are allowed only for hosts in `fetch_allow_hosts`.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod hard;
pub use hard::{Classification, HardRule, Hint, Opinion};

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
    /// Component sequences (e.g. `.enzarb`, `.config/git`) that make a path
    /// unreadable even inside a read root.
    pub deny_read: Vec<String>,
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
            // Docker talks to the dockerd that sandbox-init runs in this container (ADR-0016). Extra checks in `check_docker`.
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
            // Repo check scripts run through an interpreter: the script must
            // sit in the edit roots (see `check_script_interpreter`).
            CommandRule::new("bash"),
            CommandRule::new("sh"),
            // Python comes from the repo's mise.toml; same script-only rule.
            CommandRule::new("python3"),
            CommandRule::new("shellcheck").paths(Read).deny_args(&[
                "-x",
                "--external-sources",
                "-P",
                "--source-path",
                "--rcfile",
            ]),
            // `mise exec`/`x` is unwrapped in `decide_shell` and its command judged like a
            // top-level one; `use`, `set`, `settings`, `plugins` and the rest are refused.
            CommandRule::new("mise")
                .allow_sub(&["install", "exec", "x", "ls", "which", "run"])
                .global(&["--version", "-v", "--help", "-h"]),
            // Hooks come from the repo's `lefthook.yml`: same trust as `mise run`.
            // `install` writes into `.git/hooks`, so it is not listed.
            CommandRule::new("lefthook")
                .allow_sub(&["validate", "run"])
                .global(&["--version", "-v", "--help", "-h"]),
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
            read_roots: vec![
                "{worktree}".into(),
                "{workspace}/repos".into(),
                "{workspace}/worktrees".into(),
            ],
            deny_read: [
                ".enzarb",
                ".git-credentials",
                ".gitconfig",
                ".netrc",
                ".config/git/credentials",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
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
                "PS4",
                "PROMPT_COMMAND",
                "SHELLOPTS",
                "BASHOPTS",
                "BASH_XTRACEFD",
                "LD_*",
                "DYLD_*",
                "GIT_*",
                "NODE_*",
                "NPM_*",
                "PYTHON*",
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
                let ws = worktree.parent().and_then(Path::parent).unwrap_or(worktree);
                let r = r
                    .replace("{worktree}", &worktree.to_string_lossy())
                    .replace("{workspace}", &ws.to_string_lossy());
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
                    if !self.read_ok(&read, &cwd, p) {
                        return Decision::deny(
                            "read",
                            format!("{p} is outside the read roots or is a secret"),
                        );
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
                let ok = if r.input {
                    self.read_ok(roots, &cwd, &r.target)
                } else {
                    inside(roots, &cwd, &r.target)
                };
                if !ok {
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
            let mut prog = prog.as_str();
            let mut args = &seg.words[1..];
            // `timeout` is transparent: classify the inner command exactly
            // like a top-level one.
            // `mise exec|x [tool@version ...] -- <cmd>` is transparent the same way.
            let mut wrapped = 0;
            loop {
                let (wrapper, unwrapped) = if prog == "timeout" {
                    ("timeout", unwrap_timeout(args))
                } else if prog == "mise"
                    && matches!(args.first().map(String::as_str), Some("exec" | "x"))
                {
                    ("mise", unwrap_mise_exec(&args[1..]))
                } else {
                    break;
                };
                wrapped += 1;
                if wrapped > MAX_TIMEOUT_NESTING {
                    return Decision::deny("shell", format!("{wrapper}: nested too deeply"));
                }
                match unwrapped {
                    Ok(inner) => {
                        prog = inner[0].as_str();
                        args = &inner[1..];
                    }
                    Err(why) => return Decision::deny("shell", format!("{wrapper}: {why}")),
                }
            }
            if wrapped > 0 && prog == "cd" {
                return Decision::deny("shell", "cd cannot be wrapped");
            }
            if prog == "cd" {
                let piped = seg.piped_before || seg.piped_after;
                let [dir] = args else {
                    return Decision::deny("shell", "cd needs exactly one directory");
                };
                if piped || !inside(edit, &cwd, dir) {
                    return Decision::deny("shell", format!("cd {dir} leaves the worktree"));
                }
                cwd = resolve(&cwd, dir).unwrap_or(cwd);
                continue;
            }
            if prog.contains('/') {
                // A script inside the run's own worktree, like `make` or
                // `mise run` (which already execute repo code).
                if !inside(edit, &cwd, prog) {
                    return Decision::deny(
                        "shell",
                        format!("{prog}: scripts must be inside the worktree"),
                    );
                }
                if let Err(why) = self.check_script_file(prog, &cwd, edit) {
                    return Decision::deny("shell", why);
                }
                if let Err(why) = self.script_args(args, &cwd, read) {
                    return Decision::deny("shell", format!("{prog}: {why}"));
                }
                continue;
            }
            let Some(rule) = self.commands.iter().find(|c| c.name == prog) else {
                return Decision::deny("shell", format!("{prog} is not an allowed command"));
            };
            if let Err(why) = self.check_args(rule, args, &cwd, read, edit) {
                return Decision::deny("shell", format!("{prog}: {why}"));
            }
        }
        Decision::allow("shell", "every command matches the allowlist")
    }

    /// A script run from the worktree: a regular file (not a symlink, so it
    /// cannot be re-pointed within a compound command) inside the edit roots.
    fn check_script_file(&self, script: &str, cwd: &Path, edit: &[PathBuf]) -> Result<(), String> {
        let p = resolve(cwd, script).ok_or_else(|| format!("{script}: unresolvable path"))?;
        if !edit.iter().any(|r| p.starts_with(r)) {
            return Err(format!("{script}: scripts must be inside the worktree"));
        }
        let raw = if Path::new(script).is_absolute() {
            PathBuf::from(script)
        } else {
            cwd.join(script)
        };
        let is_link = std::fs::symlink_metadata(&raw).is_ok_and(|m| m.file_type().is_symlink());
        if is_link || !p.is_file() {
            return Err(format!("{script}: not a regular script file"));
        }
        Ok(())
    }

    /// Script arguments that look like paths must be readable by the policy.
    fn script_args(&self, args: &[String], cwd: &Path, read: &[PathBuf]) -> Result<(), String> {
        for a in args {
            let v = if a.starts_with('-') {
                a.split_once('=').map_or("", |(_, v)| v)
            } else {
                a.as_str()
            };
            if path_like(v) && !self.read_ok(read, cwd, v) {
                return Err(format!("path {v} is outside the allowed roots"));
            }
        }
        Ok(())
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
        if matches!(rule.name.as_str(), "bash" | "sh" | "python3") {
            let n = check_script_interpreter(rest, &rule.name, cwd, edit)?;
            self.check_script_file(&rest[n], cwd, edit)?;
            self.script_args(&rest[n + 1..], cwd, read)?;
        }
        if rule.name == "git" {
            rest = self.git_dash_c(rest, cwd, read)?;
            self.git_args(rest, cwd, edit)?;
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
        if rule.name == "mise" {
            check_mise(rest)?;
        }
        if rule.name == "docker" {
            // Bind mounts are writable: only the edit roots qualify.
            check_docker(rest, cwd, edit)?;
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
            let ok = if rule.paths == PathMode::Read {
                self.read_ok(roots, cwd, candidate)
            } else {
                inside(roots, cwd, candidate)
            };
            if path_like(candidate) && !ok {
                return Err(format!("path {candidate} is outside the allowed roots"));
            }
        }
        Ok(())
    }

    /// Strip leading `-C <dir>` pairs from git's arguments. They are accepted
    /// only for read-only subcommands and directories in the read roots.
    fn git_dash_c<'a>(
        &self,
        args: &'a [String],
        cwd: &Path,
        read: &[PathBuf],
    ) -> Result<&'a [String], String> {
        let mut i = 0;
        let mut dirs = Vec::new();
        while let Some(a) = args.get(i) {
            if a == "-C" {
                let Some(dir) = args.get(i + 1) else {
                    return Err("-C needs a directory".into());
                };
                dirs.push(dir);
                i += 2;
            } else if matches!(a.as_str(), "--no-pager" | "-P" | "--no-optional-locks") {
                i += 1;
            } else {
                break;
            }
        }
        if dirs.is_empty() {
            return Ok(args);
        }
        let rest = &args[i..];
        for d in dirs {
            if !self.read_ok(read, cwd, d) {
                return Err(format!("-C {d} is outside the read roots or is a secret"));
            }
        }
        let Some(sub) = rest.first() else {
            return Err("-C needs a read-only subcommand".into());
        };
        if !GIT_READ_ONLY.contains(&sub.as_str()) {
            return Err(format!(
                "git -C only allows read-only subcommands, not {sub}; run git from your \
                 worktree directory without -C, for example `git add -A && git commit -m ...`"
            ));
        }
        if sub == "branch" {
            let lists = rest[1..]
                .iter()
                .any(|a| matches!(a.as_str(), "--list" | "-l" | "--show-current"));
            let mutates = rest[1..].iter().any(|a| {
                a.starts_with("--set-upstream")
                    || a.starts_with("--unset-upstream")
                    || a.starts_with("--edit-description")
                    || matches!(
                        a.as_str(),
                        "-d" | "-D"
                            | "-m"
                            | "-M"
                            | "-c"
                            | "-C"
                            | "-f"
                            | "-u"
                            | "--delete"
                            | "--move"
                            | "--copy"
                            | "--force"
                            | "--track"
                            | "-t"
                    )
            });
            if !lists || mutates {
                return Err("git -C branch only allows --list".into());
            }
        }
        Ok(rest)
    }

    /// Argument checks for every git invocation: options that run programs,
    /// write files or read arbitrary files, and subcommands that create
    /// files at a path given in their arguments.
    fn git_args(&self, rest: &[String], cwd: &Path, edit: &[PathBuf]) -> Result<(), String> {
        let sub = rest
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);
        if let Some(sub) = sub
            && matches!(
                sub,
                "archive" | "bundle" | "checkout-index" | "format-patch"
            )
        {
            return Err(format!("git {sub} writes files and is not allowed"));
        }
        let writes_path = matches!(sub, Some("worktree" | "init" | "clone"));
        for a in rest {
            if a == "--" {
                break;
            }
            if let Some(long) = a.strip_prefix("--") {
                let name = long.split_once('=').map_or(long, |(n, _)| n);
                // git accepts unambiguous abbreviations of long options.
                if name.len() >= 2
                    && GIT_DENY_LONG
                        .iter()
                        .any(|d| d.strip_prefix("--").is_some_and(|d| d.starts_with(name)))
                {
                    return Err(format!("argument {a} is not allowed"));
                }
            } else if let Some(short) = a.strip_prefix('-') {
                let bad = match sub {
                    Some("grep") => "fO",
                    Some("ls-files") => "XO",
                    _ => "O",
                };
                if short.chars().any(|c| bad.contains(c)) && !short.starts_with('-') {
                    return Err(format!("argument {a} is not allowed"));
                }
            }
            let tail = a.rsplit(':').next().unwrap_or(a);
            if self.secret_path(Path::new(a)) || self.secret_path(Path::new(tail)) {
                return Err(format!("argument {a} names a secret"));
            }
            if writes_path && path_like(a) && !a.starts_with('-') && !inside(edit, cwd, a) {
                return Err(format!("path {a} is outside the edit roots"));
            }
        }
        Ok(())
    }

    /// `arg` is inside the read roots and does not name a secret.
    fn read_ok(&self, roots: &[PathBuf], cwd: &Path, arg: &str) -> bool {
        let Some(p) = resolve(cwd, arg) else {
            return false;
        };
        roots.iter().any(|r| p.starts_with(r)) && !self.secret_path(&p)
    }

    fn secret_path(&self, p: &Path) -> bool {
        let comps: Vec<_> = p.components().map(|c| c.as_os_str()).collect();
        self.deny_read.iter().any(|d| {
            let seq: Vec<_> = Path::new(d).components().map(|c| c.as_os_str()).collect();
            !seq.is_empty() && comps.windows(seq.len()).any(|w| w == seq.as_slice())
        })
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

/// Git subcommands usable with `-C` (read-only).
const GIT_READ_ONLY: &[&str] = &[
    "status",
    "log",
    "show",
    "diff",
    "rev-parse",
    "ls-files",
    "ls-tree",
    "grep",
    "blame",
    "branch",
    "cat-file",
];
/// Long options refused for git: they write files, run programs or read
/// files outside the repository.
const GIT_DENY_LONG: &[&str] = &[
    "--output",
    "--output-indicator-new",
    "--no-index",
    "--ext-diff",
    "--textconv",
    "--open-files-in-pager",
    "--git-dir",
    "--work-tree",
    "--exec-path",
    "--contents",
    "--exclude-from",
    "--orderfile",
    "--file",
    "--prefix",
    "--upload-pack",
    "--receive-pack",
    "--exec",
];

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

/// Maximum depth of nested `timeout` wrappers.
const MAX_TIMEOUT_NESTING: usize = 3;

/// `^\d+(\.\d+)?[smhd]?$`
fn valid_duration(s: &str) -> bool {
    let s = s.strip_suffix(['s', 'm', 'h', 'd']).unwrap_or(s);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// Digits, or an upper-case signal name with an optional `SIG` prefix.
fn valid_signal(s: &str) -> bool {
    let name = s.strip_prefix("SIG").unwrap_or(s);
    !name.is_empty()
        && (name.bytes().all(|b| b.is_ascii_digit())
            || name
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
}

/// Validate the arguments of `timeout` and return the inner command
/// (program first). Stricter than GNU `timeout`: no `-v`, no `--`.
fn unwrap_timeout(args: &[String]) -> Result<&[String], String> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if !a.starts_with('-') || a.len() == 1 {
            break;
        }
        i += 1;
        let (kind, val): (&str, Option<&str>) = match a.as_str() {
            "--foreground" | "--preserve-status" => continue,
            "-s" | "-k" => (&a[1..], None),
            s if s.starts_with("--signal=") => ("s", Some(&s[9..])),
            s if s.starts_with("--kill-after=") => ("k", Some(&s[13..])),
            s if s.starts_with("-s") => ("s", Some(&s[2..])),
            s if s.starts_with("-k") => ("k", Some(&s[2..])),
            _ => return Err(format!("flag {a} is not allowed")),
        };
        let val = match val {
            Some(v) => v,
            None => {
                let v = args.get(i).ok_or("missing flag value")?;
                i += 1;
                v.as_str()
            }
        };
        let ok = if kind == "s" {
            valid_signal(val)
        } else {
            valid_duration(val)
        };
        if !ok {
            return Err(format!("invalid value {val:?} for -{kind}"));
        }
    }
    let dur = args.get(i).ok_or("missing duration")?;
    if !valid_duration(dur) {
        return Err(format!("invalid duration {dur:?}"));
    }
    let inner = &args[i + 1..];
    if inner.is_empty() {
        return Err("missing command".to_string());
    }
    Ok(inner)
}

/// A plain `tool` or `tool@version` as `mise install` and `mise exec` take them. Backends
/// (`npm:x`, `pipx:x`, `ubi:x`, ...) and URLs or paths are refused: they fetch arbitrary code.
fn valid_tool_spec(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'))
}

/// `mise exec|x [tool@version ...] -- <cmd>` (the arguments after `exec`): the command
/// after `--` is returned to be judged like any other.
fn unwrap_mise_exec(args: &[String]) -> Result<&[String], String> {
    let dd = args
        .iter()
        .position(|a| a == "--")
        .ok_or("the command must follow `--`")?;
    if let Some(bad) = args[..dd].iter().find(|a| !valid_tool_spec(a)) {
        return Err(format!("{bad:?} is not a plain tool@version"));
    }
    let inner = &args[dd + 1..];
    if inner.is_empty() {
        return Err("missing command".to_string());
    }
    Ok(inner)
}

/// `mise install|ls|which` take plain tool specs and a few read-only flags; `run` runs the
/// repo's own tasks. (`exec`/`x` never get here: they are unwrapped.)
fn check_mise(rest: &[String]) -> Result<(), String> {
    let Some(i) = rest.iter().position(|a| !a.starts_with('-')) else {
        return Ok(());
    };
    if !matches!(rest[i].as_str(), "install" | "ls" | "which") {
        return Ok(());
    }
    for a in &rest[i + 1..] {
        let flag = a.starts_with('-')
            && matches!(
                a.as_str(),
                "--json" | "--installed" | "--current" | "--help" | "-h"
            );
        if !flag && !valid_tool_spec(a) {
            return Err(format!("argument {a} is not allowed"));
        }
    }
    Ok(())
}

/// `bash|sh [-e -x -u ...] <script> [args]`: only a script file inside the
/// edit roots may run; `-c`, stdin and rc-file options are refused.
/// `python3 [-u -B ...] <script> [args]` follows the same rule; `-c`, `-m`,
/// `-`, `-i`, `-W`, `-X` and long options are refused.
fn check_script_interpreter(
    args: &[String],
    name: &str,
    cwd: &Path,
    edit: &[PathBuf],
) -> Result<usize, String> {
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if a == "--" {
            i += 1;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            break;
        }
        let python = name == "python3";
        let ok = if a.starts_with("--") {
            !python && matches!(a.as_str(), "--posix" | "--noprofile" | "--norc")
        } else {
            let short = if python { "uBEsSIOq" } else { "eux" };
            a[1..].chars().all(|c| short.contains(c))
        };
        if !ok {
            return Err(format!("option {a} is not allowed for {name}"));
        }
        i += 1;
    }
    let Some(script) = args.get(i) else {
        return Err(format!("{name} needs a script file inside the worktree"));
    };
    if !inside(edit, cwd, script) {
        return Err(format!("script {script} is outside the worktree"));
    }
    Ok(i)
}

/// Extra rules for `docker`, on top of the allowlist/subcommand checks. The
/// daemon is the sandbox container's own dockerd (inside gVisor), so `--network=host`
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

/// Split a command line into tokens. Strict mode (`lenient == false`, used
/// by [`Policy::decide`]) refuses every construct it cannot judge. Lenient
/// mode (used by the hard-deny scan) never fails: substitutions, groups,
/// heredocs and process substitution become separators, so their inner
/// commands are scanned as ordinary segments, and `$` stays in the word.
fn tokenize(s: &str, lenient: bool) -> Result<Vec<Tok>, String> {
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
                    None if lenient => {}
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
                        None if lenient => break,
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
                        Some(&n @ ('$' | '`')) if lenient => cur.push(n),
                        Some('$' | '`') => {
                            return Err("shell expansion ($ or backtick) is not allowed".into());
                        }
                        Some(&n) => cur.push(n),
                        None if lenient => break,
                        None => return Err("unterminated quote".into()),
                    }
                    i += 1;
                }
            }
            '$' if lenient && chars.get(i + 1) != Some(&'(') => {
                cur.push('$');
                has_word = true;
            }
            '$' | '`' | '(' | ')' if lenient => {
                flush!();
                toks.push(Tok::Sep);
            }
            '{' | '}' if lenient && !has_word => {
                flush!();
                toks.push(Tok::Sep);
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
                if lenient && chars.get(i + 1) == Some(&'(') {
                    // `>(cmd)`: the inner command is its own segment.
                    toks.push(Tok::Sep);
                } else if chars.get(i + 1) == Some(&'&') {
                    // `>&2`, `>&-`
                    i += 1;
                    let mut j = i + 1;
                    while chars
                        .get(j)
                        .is_some_and(|c| c.is_ascii_digit() || *c == '-')
                    {
                        j += 1;
                    }
                    if j == i + 1 && lenient {
                        // `>&file`: like `&>file`.
                        toks.push(Tok::Out);
                        i += 1;
                        continue;
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
                if lenient && matches!(chars.get(i + 1), Some('<' | '(' | '&')) {
                    // Heredoc, here-string, `<(cmd)` or `<&fd`: what follows
                    // is scanned as its own segment.
                    flush!();
                    while matches!(chars.get(i + 1), Some('<' | '&' | '-')) {
                        i += 1;
                    }
                    toks.push(Tok::Sep);
                    i += 1;
                    continue;
                }
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
    parse_shell_mode(s, false)
}

/// [`parse_shell`] in lenient mode: never fails (see [`tokenize`]).
fn parse_shell_lenient(s: &str) -> Vec<Segment> {
    parse_shell_mode(s, true).unwrap_or_default()
}

fn parse_shell_mode(s: &str, lenient: bool) -> Result<Vec<Segment>, String> {
    let toks = tokenize(s, lenient)?;
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
                    if !lenient {
                        return Err("redirection without a target".into());
                    }
                    pending = None;
                }
                let is_pipe = t == Tok::Pipe;
                cur.piped_before = piped_before;
                cur.piped_after = is_pipe;
                piped_before = is_pipe;
                finish(&mut cur, &mut segs);
            }
        }
    }
    if pending.is_some() && !lenient {
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
            "mise install node@24",
            "mise ls",
            "mise ls --json",
            "mise which cargo",
            "mise x node@24 -- npm test",
            "mise exec -- cargo test",
            "mise exec rust@1.99.0 node@24 -- cargo fmt --check",
            "timeout 60 mise install",
            "lefthook validate",
            "lefthook run pre-commit",
            "lefthook --version",
            "cargo fmt --check &&cargo clippy --all-targets -- -D warnings && cargo test",
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
    fn docker_in_the_sandbox_container_workflows_are_allowed() {
        // The daemon runs in this container (ADR-0016): the worktree path is the same for the
        // agent and for bind mounts.
        let (_d, wt) = setup();
        let w = wt.display();
        for c in [
            "docker version".to_string(),
            "docker version --format '{{.Server.Version}}'".to_string(),
            "docker info".to_string(),
            "docker run --rm hello-world".to_string(),
            "docker run --rm --user 1000:1000 alpine id".to_string(),
            "docker run --rm -u 1000:1000 --network=host alpine wget -qO- https://example.com"
                .to_string(),
            format!("docker run --rm -v {w}:/src alpine ls /src"),
            format!("docker run --rm --user 1000:1000 -v {w}/data:/data:ro alpine ls /data"),
            format!("docker run --rm --mount type=bind,source={w},target=/src alpine ls /src"),
            "docker compose up".to_string(),
            "docker compose up --build -d".to_string(),
            "docker build --network=host -t app:dev .".to_string(),
        ] {
            let d = sh(&Policy::default(), &wt, &c);
            assert!(d.allow, "{c:?} should be allowed: {d:?}");
        }
    }

    #[test]
    fn docker_daemon_sockets_and_state_cannot_be_bind_mounted() {
        for c in [
            "docker run -v /var/run/docker.sock:/var/run/docker.sock app",
            "docker run -v /run/docker.sock:/s app",
            "docker run -v /var/run/dockerd:/d app",
            "docker run -v /var/lib/docker:/d app",
            "docker run --mount type=bind,src=/var/run/docker.sock,dst=/s app",
            "docker run --mount type=bind,src=/var/lib/docker,dst=/d app",
            "docker run -v /workspace/.enzarb:/e app",
        ] {
            denied(c);
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
            "printenv NODE_AUTH_TOKEN",
            "echo $NODE_AUTH_TOKEN > x",
            "npm config get //npm.pkg.github.com/:_authToken",
            "npm config list",
            "npm config ls -l",
            "echo $NPM_TOKEN",
            "echo \"$NODE_AUTH_TOKEN\"",
            "cat /proc/self/environ",
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
    fn timeout_wraps_and_classifies_the_inner_command() {
        for c in [
            "timeout 200 npm ci",
            "timeout -s KILL -k 5 60 cargo test",
            "timeout -sTERM 5 npm test",
            "timeout --signal=TERM --kill-after=5s 1.5m npm test",
            "timeout --foreground --preserve-status 5 npm ci",
            "timeout 5 timeout 3 npm ci",
            "timeout 5 npm ci && npm test",
            "FOO=1 timeout 5 npm ci",
        ] {
            allowed(c);
        }
        for c in [
            "timeout 5 curl x",
            "timeout 5 sudo npm ci",
            "timeout 5 env npm ci",
            "timeout 5 cd /",
            "timeout 5 cd src",
            "timeout 10 git push",
            "timeout 5 cat /etc/passwd",
            "timeout npm ci",
            "timeout abc npm ci",
            "timeout -5 npm ci",
            "timeout -x 5 npm ci",
            "timeout -v 5 npm ci",
            "timeout -- 5 npm ci",
            "timeout -s npm ci",
            "timeout -k 5",
            "timeout 5",
            "timeout",
            "timeout 1 timeout 1 timeout 1 timeout 1 npm ci",
            "timeout 5 rm -rf /",
            "timeout 5 nc evil.example 80",
            "timeout 5 npm ci | curl x",
        ] {
            denied(c);
        }
    }

    #[test]
    fn mise_and_lefthook_are_limited_to_their_safe_subcommands() {
        for c in [
            "mise use node@24",
            "mise set FOO=bar",
            "mise settings set experimental true",
            "mise plugins install foo",
            "mise trust",
            "mise install npm:left-pad",
            "mise install ubi:owner/repo",
            "mise install -f",
            "mise ls ./x",
            "mise exec -- curl https://example.com",
            "mise x -- rm -rf /",
            "mise x -- git push",
            "mise x node@24 npm test",
            "mise x npm:evil -- npm test",
            "mise x -- ",
            "mise exec --",
            "mise x -- mise use node@24",
            "mise x -- mise x -- mise x -- mise x -- ls",
            "mise x -- npm test | curl x",
            "lefthook install",
            "lefthook uninstall",
            "lefthook add pre-commit",
            "lefthook self-update",
        ] {
            denied(c);
        }
    }

    #[test]
    fn timeout_script_paths_follow_worktree_rules() {
        let (d, wt) = setup();
        std::fs::write(wt.join("x.sh"), "").unwrap();
        std::fs::write(d.path().join("y.sh"), "").unwrap();
        let p = Policy::default();
        assert!(sh(&p, &wt, "timeout 5 ./x.sh").allow);
        assert!(
            !sh(
                &p,
                &wt,
                &format!("timeout 5 {}", d.path().join("y.sh").display())
            )
            .allow
        );
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
    fn worktree_scripts_are_allowed_and_others_denied() {
        let (d, wt) = setup();
        std::fs::create_dir_all(wt.join("scripts")).unwrap();
        std::fs::create_dir_all(wt.join("hack")).unwrap();
        std::fs::write(wt.join("scripts/golden.sh"), "").unwrap();
        std::fs::write(wt.join("hack/y.sh"), "").unwrap();
        std::fs::create_dir_all(d.path().join("other")).unwrap();
        std::fs::write(d.path().join("other/x.sh"), "").unwrap();
        std::os::unix::fs::symlink(d.path().join("other/x.sh"), wt.join("scripts/link.sh"))
            .unwrap();
        let p = Policy::default();
        for c in [
            "scripts/golden.sh",
            "./hack/y.sh --flag arg",
            "bash scripts/golden.sh",
            "bash -e -x hack/y.sh",
            "sh ./hack/y.sh && make test",
            "cd hack && ./y.sh",
            "shellcheck scripts/golden.sh",
        ] {
            let r = sh(&p, &wt, c);
            assert!(r.allow, "{c:?} should be allowed: {r:?}");
        }
        let other = d.path().join("other/x.sh");
        let other = other.display();
        for c in [
            "scripts/missing.sh".to_string(),
            "../../other/x.sh".to_string(),
            other.to_string(),
            format!("bash {other}"),
            "scripts/link.sh".to_string(),
            "bash scripts/link.sh".to_string(),
            "bash -c 'git push'".to_string(),
            "bash".to_string(),
            "bash -s".to_string(),
            "sh --rcfile x scripts/golden.sh".to_string(),
            "/bin/sh scripts/golden.sh".to_string(),
            "~/x.sh".to_string(),
            "bash ~/x.sh".to_string(),
            "bash < scripts/golden.sh".to_string(),
            "git push".to_string(),
            "scripts/golden.sh /etc/shadow".to_string(),
            "bash scripts/golden.sh ~/.enzarb/token".to_string(),
            "PS4=x bash -x scripts/golden.sh".to_string(),
            "BASH_ENV=x bash scripts/golden.sh".to_string(),
            "bash -eo pipefail scripts/golden.sh".to_string(),
            "shellcheck /etc/passwd".to_string(),
            "shellcheck -x scripts/golden.sh".to_string(),
            "shellcheck -P /etc scripts/golden.sh".to_string(),
            "shellcheck --source-path=/etc scripts/golden.sh".to_string(),
            "shellcheck --rcfile /etc/x scripts/golden.sh".to_string(),
            "./hack".to_string(),
        ] {
            let r = sh(&p, &wt, &c);
            assert!(!r.allow, "{c:?} should be denied: {r:?}");
        }
    }

    #[test]
    fn python_runs_worktree_scripts_only() {
        let (d, wt) = setup();
        std::fs::create_dir_all(wt.join("scripts")).unwrap();
        std::fs::write(wt.join("scripts/x.py"), "").unwrap();
        std::fs::write(wt.join("x.py"), "").unwrap();
        std::fs::create_dir_all(d.path().join("other")).unwrap();
        std::fs::write(d.path().join("other/x.py"), "").unwrap();
        std::os::unix::fs::symlink(d.path().join("other/x.py"), wt.join("scripts/link.py"))
            .unwrap();
        let p = Policy::default();
        for c in [
            "python3 scripts/x.py",
            "python3 -u scripts/x.py arg",
            "python3 -uB x.py",
            "python3 -- x.py",
            "cd scripts && python3 x.py",
        ] {
            let r = sh(&p, &wt, c);
            assert!(r.allow, "{c:?} should be allowed: {r:?}");
        }
        let other = d.path().join("other/x.py");
        let other = other.display();
        for c in [
            "python3 -c 'print(1)'".to_string(),
            "python3 -".to_string(),
            "python3 < x.py".to_string(),
            "python3 -m http.server".to_string(),
            "python3 -i x.py".to_string(),
            "python3 -X importtime x.py".to_string(),
            "python3 -W error x.py".to_string(),
            "python3 -V".to_string(),
            "python3 --help".to_string(),
            "python3".to_string(),
            format!("python3 {other}"),
            "python3 ../../other/x.py".to_string(),
            "python3 scripts/link.py".to_string(),
            "python3 scripts/missing.py".to_string(),
            "/usr/bin/python3 x.py".to_string(),
            "PYTHONSTARTUP=x python3 x.py".to_string(),
            "PYTHONPATH=. python3 x.py".to_string(),
            "python3 x.py /etc/shadow".to_string(),
        ] {
            let r = sh(&p, &wt, &c);
            assert!(!r.allow, "{c:?} should be denied: {r:?}");
        }
    }

    #[test]
    fn allowed_python_script_actually_runs() {
        let ok = std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipping: python3 is not on PATH");
            return;
        }
        let (_d, wt) = setup();
        std::fs::create_dir_all(wt.join("scripts")).unwrap();
        std::fs::write(wt.join("scripts/check.py"), "print('py-ok', 6 * 7)\n").unwrap();
        let p = Policy::default();
        let r = sh(&p, &wt, "python3 scripts/check.py");
        assert!(r.allow, "{r:?}");
        let out = std::process::Command::new("python3")
            .arg("scripts/check.py")
            .current_dir(&wt)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "py-ok 42");
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

    fn ws_path(wt: &Path, rel: &str) -> String {
        wt.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(rel)
            .to_string_lossy()
            .into_owned()
    }

    fn cross_setup() -> (tempfile::TempDir, PathBuf) {
        let (d, wt) = setup();
        std::fs::create_dir_all(d.path().join("repos/other-abc")).unwrap();
        std::fs::write(d.path().join("repos/other-abc/README.md"), "x").unwrap();
        std::fs::create_dir_all(d.path().join("worktrees/run-2")).unwrap();
        std::fs::write(d.path().join(".git-credentials"), "x").unwrap();
        (d, wt)
    }

    #[test]
    fn cross_repo_reads_are_allowed() {
        let (_d, wt) = cross_setup();
        let p = Policy::default();
        let readme = ws_path(&wt, "repos/other-abc/README.md");
        for c in [
            format!("cat {readme}"),
            format!("ls {}", ws_path(&wt, "repos")),
            format!("grep -rn x {}", ws_path(&wt, "worktrees/run-2")),
            format!("head -n 3 {readme}"),
        ] {
            let d = sh(&p, &wt, &c);
            assert!(d.allow, "{c}: {d:?}");
        }
        for kind in ["read", "search"] {
            let d = p.decide(
                &json!({"kind": kind, "rawInput": {"file_path": readme, "path": readme, "pattern": "x"}}),
                &wt,
            );
            assert!(d.allow, "{kind}: {d:?}");
        }
        let glob = p.decide(
            &json!({"kind": "search", "rawInput": {"pattern": format!("{}/**/*.md", ws_path(&wt, "repos"))}}),
            &wt,
        );
        assert!(glob.allow, "{glob:?}");
    }

    #[test]
    fn cross_repo_writes_are_denied() {
        let (_d, wt) = cross_setup();
        let p = Policy::default();
        let readme = ws_path(&wt, "repos/other-abc/README.md");
        for c in [
            format!("rm {readme}"),
            format!("touch {}", ws_path(&wt, "repos/other-abc/new")),
            format!("cp src/lib.rs {readme}"),
            format!("echo x > {readme}"),
            format!("tee {readme}"),
            format!("cd {} && touch x", ws_path(&wt, "repos/other-abc")),
            format!("cd {} && git commit -m x", ws_path(&wt, "repos/other-abc")),
            format!(
                "docker run -v {}:/r alpine ls",
                ws_path(&wt, "repos/other-abc")
            ),
        ] {
            let d = sh(&p, &wt, &c);
            assert!(!d.allow, "{c}: {d:?}");
        }
        for kind in ["edit", "delete"] {
            let d = p.decide(
                &json!({"kind": kind, "rawInput": {"file_path": readme}}),
                &wt,
            );
            assert!(!d.allow, "{kind}: {d:?}");
        }
        let d = p.decide(
            &json!({"kind": "move", "rawInput": {"old_path": "src/lib.rs", "new_path": readme}}),
            &wt,
        );
        assert!(!d.allow);
    }

    #[test]
    fn git_dash_c_is_read_only() {
        let (_d, wt) = cross_setup();
        let p = Policy::default();
        let repo = ws_path(&wt, "repos/other-abc");
        for sub in [
            "status",
            "log --oneline -5",
            "show HEAD",
            "diff HEAD~1",
            "rev-parse HEAD",
            "ls-files",
            "ls-tree HEAD",
            "grep -n foo",
            "blame README.md",
            "branch --list",
            "cat-file -p HEAD",
        ] {
            let c = format!("git -C {repo} {sub}");
            let d = sh(&p, &wt, &c);
            assert!(d.allow, "{c}: {d:?}");
        }
        let c = format!("git --no-pager -C {} log", wt.display());
        assert!(sh(&p, &wt, &c).allow);
        for sub in [
            "commit -m x",
            "checkout main",
            "reset --hard",
            "fetch",
            "pull",
            "clean -fd",
            "branch -D main",
            "branch newbranch",
            "branch --list -m a b",
            "push",
            "config user.name x",
            "diff --output=/tmp/x",
            "diff --no-index /etc/passwd x",
            "grep -O foo",
            "worktree remove x",
            "update-ref -d HEAD",
            "",
        ] {
            let c = format!("git -C {repo} {sub}");
            let d = sh(&p, &wt, &c);
            assert!(!d.allow, "{c}: {d:?}");
        }
        for c in [
            "git -C /etc status",
            "git -C / log",
            "git -C ../.. status",
            "git -C",
        ] {
            assert!(!sh(&p, &wt, c).allow, "{c}");
        }
    }

    #[test]
    fn git_dash_c_write_denial_hints_at_plain_git() {
        let (_d, wt) = cross_setup();
        let p = Policy::default();
        let repo = ws_path(&wt, "repos/other-abc");
        for dir in [wt.display().to_string(), repo] {
            let c = format!("git -C {dir} commit -m x");
            let d = sh(&p, &wt, &c);
            assert!(!d.allow, "{c}: {d:?}");
            assert_eq!(d.rule, "shell");
            assert!(d.reason.contains("without -C"), "{d:?}");
            assert!(d.reason.contains("git add -A && git commit"), "{d:?}");
        }
    }

    #[test]
    fn git_argument_escapes_are_denied() {
        let (_d, wt) = cross_setup();
        let p = Policy::default();
        let repo = ws_path(&wt, "repos/other-abc");
        let tok = ws_path(&wt, ".enzarb/claude-token");
        for c in [
            "git grep -O'touch x;' hello".to_string(),
            "git grep -Otouch hello".to_string(),
            "git grep --open-files-in-pager=sh hello".to_string(),
            "git grep -nf /etc/passwd".to_string(),
            "git blame --contents /etc/passwd README.md".to_string(),
            format!("git blame --contents {tok} README.md"),
            format!("git diff --no-index {tok} /dev/null"),
            format!("git log -1 --output={repo}/README.md"),
            format!("git log -1 --out={repo}/README.md"),
            format!("git diff --output {repo}/README.md"),
            "git ls-files -X /etc/passwd".to_string(),
            "git ls-files --exclude-from=/etc/passwd".to_string(),
            format!("git archive -o {repo}/a.tar HEAD"),
            format!("git format-patch -o {repo} HEAD~1"),
            format!("git worktree add {repo}/wt"),
            format!("git init {repo}/sub"),
            format!("git clone . {repo}/c"),
            format!("git -C {repo} show HEAD:.git-credentials"),
            "git show HEAD:.enzarb/claude-token".to_string(),
        ] {
            let d = sh(&p, &wt, &c);
            assert!(!d.allow, "{c}: {d:?}");
        }
        for c in [
            "git log --oneline origin/main",
            "git diff --stat HEAD~1",
            "git grep -n foo",
            "git show HEAD:src/lib.rs",
            "git worktree list",
            "git ls-files -o",
        ] {
            let d = sh(&p, &wt, c);
            assert!(d.allow, "{c}: {d:?}");
        }
    }

    #[test]
    fn secrets_are_unreadable_under_the_new_roots() {
        let (d, wt) = cross_setup();
        let p = Policy::default();
        let token = ws_path(&wt, ".enzarb/claude-token");
        let creds = ws_path(&wt, ".git-credentials");
        // secrets planted inside a readable checkout
        let repo = d.path().join("repos/other-abc");
        std::fs::create_dir_all(repo.join(".config/git")).unwrap();
        std::fs::write(repo.join(".git-credentials"), "x").unwrap();
        std::fs::write(repo.join(".config/git/credentials"), "x").unwrap();
        let nested = format!("{}/.git-credentials", repo.display());
        let helper = format!("{}/.config/git/credentials", repo.display());
        let enz = ws_path(&wt, "repos/other-abc/.enzarb/x");
        for path in [&token, &creds, &nested, &helper, &enz] {
            for c in [
                format!("cat {path}"),
                format!("grep x {path}"),
                format!("wc -l < {path}"),
            ] {
                assert!(!sh(&p, &wt, &c).allow, "{c}");
            }
            for kind in ["read", "search"] {
                let r = p.decide(
                    &json!({"kind": kind, "rawInput": {"file_path": path, "path": path, "pattern": "x"}}),
                    &wt,
                );
                assert!(!r.allow, "{kind} {path}");
            }
        }
        // a repo's own .claude/ and .config/ are project docs, not secrets
        let repo = d.path().join("repos/other-abc");
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(repo.join(".claude/settings.json"), "{}").unwrap();
        let settings = format!("{}/.claude/settings.json", repo.display());
        assert!(sh(&p, &wt, &format!("cat {settings}")).allow);
        let r = p.decide(
            &json!({"kind": "read", "rawInput": {"file_path": settings}}),
            &wt,
        );
        assert!(r.allow, "{r:?}");
        let c = format!("git -C {} show HEAD", ws_path(&wt, ".enzarb"));
        assert!(!sh(&p, &wt, &c).allow);
        assert!(!sh(&p, &wt, &format!("ls {}", ws_path(&wt, ""))).allow);
        assert!(!sh(&p, &wt, &format!("cat {}/.gitconfig", wt.display())).allow);
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
