//! Lifecycle hooks: user-configured shell commands run on agent events.

use globset::Glob;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

const MAX_OUTPUT: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    PostEdit,
    ModeChange,
    Stop,
}

impl HookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session_start",
            HookEvent::UserPromptSubmit => "user_prompt_submit",
            HookEvent::PreToolUse => "pre_tool_use",
            HookEvent::PostToolUse => "post_tool_use",
            HookEvent::PostEdit => "post_edit",
            HookEvent::ModeChange => "mode_change",
            HookEvent::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnFailure {
    #[default]
    Ignore,
    Block,
    FixTurn,
}

fn default_max_fix_turns() -> u32 {
    2
}

fn default_timeout() -> u64 {
    120
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookSpec {
    /// Shell command run via `sh -c` in the project root.
    pub run: String,
    /// post_edit: only when an edited file matches (also matched against the file name).
    #[serde(default)]
    pub glob: Option<String>,
    /// pre/post_tool_use: anchored regex on the tool name.
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub on_failure: OnFailure,
    #[serde(default = "default_max_fix_turns")]
    pub max_fix_turns: u32,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

/// TOML shape: `[[post_edit]] run = "..."`; used for global and per-mode hook tables.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub session_start: Vec<HookSpec>,
    #[serde(default)]
    pub user_prompt_submit: Vec<HookSpec>,
    #[serde(default)]
    pub pre_tool_use: Vec<HookSpec>,
    #[serde(default)]
    pub post_tool_use: Vec<HookSpec>,
    #[serde(default)]
    pub post_edit: Vec<HookSpec>,
    #[serde(default)]
    pub mode_change: Vec<HookSpec>,
    #[serde(default)]
    pub stop: Vec<HookSpec>,
}

impl HooksConfig {
    pub fn get(&self, ev: HookEvent) -> &[HookSpec] {
        match ev {
            HookEvent::SessionStart => &self.session_start,
            HookEvent::UserPromptSubmit => &self.user_prompt_submit,
            HookEvent::PreToolUse => &self.pre_tool_use,
            HookEvent::PostToolUse => &self.post_tool_use,
            HookEvent::PostEdit => &self.post_edit,
            HookEvent::ModeChange => &self.mode_change,
            HookEvent::Stop => &self.stop,
        }
    }

    /// Concatenate (self first, then other).
    pub fn merged(&self, other: &HooksConfig) -> HooksConfig {
        let cat = |a: &[HookSpec], b: &[HookSpec]| [a, b].concat();
        HooksConfig {
            session_start: cat(&self.session_start, &other.session_start),
            user_prompt_submit: cat(&self.user_prompt_submit, &other.user_prompt_submit),
            pre_tool_use: cat(&self.pre_tool_use, &other.pre_tool_use),
            post_tool_use: cat(&self.post_tool_use, &other.post_tool_use),
            post_edit: cat(&self.post_edit, &other.post_edit),
            mode_change: cat(&self.mode_change, &other.mode_change),
            stop: cat(&self.stop, &other.stop),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == HooksConfig::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HookOutcome {
    pub spec: HookSpec,
    pub success: bool,
    /// Combined stdout+stderr, truncated keeping head and tail.
    pub output: String,
    pub exit_code: Option<i32>,
    /// Wall-clock time the hook took, in milliseconds.
    pub duration_ms: u64,
}

impl HookOutcome {
    pub fn on_failure(&self) -> OnFailure {
        self.spec.on_failure
    }
}

pub struct HookRunner {
    cwd: PathBuf,
}

impl HookRunner {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self { cwd: cwd.into() }
    }

    /// Run all hooks for `event` whose matcher matches `tool_name`, sequentially.
    /// `payload` is written as JSON to stdin.
    pub async fn run(
        &self,
        hooks: &HooksConfig,
        event: HookEvent,
        tool_name: Option<&str>,
        payload: &Value,
    ) -> Vec<HookOutcome> {
        let session_id = payload.get("session_id").and_then(Value::as_str);
        let stdin = payload.to_string();
        let mut out = Vec::new();
        for spec in hooks.get(event) {
            if let Some(m) = &spec.matcher {
                let Some(name) = tool_name else { continue };
                match Regex::new(&format!("^(?:{m})$")) {
                    Ok(re) if re.is_match(name) => {}
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::warn!("invalid hook matcher {m:?}: {e}");
                        continue;
                    }
                }
            }
            out.push(
                self.exec(spec, spec.run.clone(), event, session_id, &stdin, None)
                    .await,
            );
        }
        out
    }

    /// Run post_edit hooks whose glob matches any of `files`, once each, with `{files}` substituted.
    pub async fn run_post_edit(
        &self,
        hooks: &HooksConfig,
        session_id: &str,
        files: &[PathBuf],
    ) -> Vec<HookOutcome> {
        let rel: Vec<String> = files
            .iter()
            .map(|f| {
                f.strip_prefix(&self.cwd)
                    .unwrap_or(f)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        let mut out = Vec::new();
        for spec in &hooks.post_edit {
            let matching: Vec<&String> = match &spec.glob {
                None => rel.iter().collect(),
                Some(g) => match Glob::new(g) {
                    Ok(g) => {
                        let m = g.compile_matcher();
                        rel.iter()
                            .filter(|f| {
                                m.is_match(f.as_str())
                                    || Path::new(f.as_str())
                                        .file_name()
                                        .is_some_and(|n| m.is_match(n))
                            })
                            .collect()
                    }
                    Err(e) => {
                        tracing::warn!("invalid hook glob {g:?}: {e}");
                        continue;
                    }
                },
            };
            if matching.is_empty() {
                continue;
            }
            let quoted = matching
                .iter()
                .map(|f| shell_quote(f))
                .collect::<Vec<_>>()
                .join(" ");
            let cmd = spec.run.replace("{files}", &quoted);
            let list: Vec<&str> = matching.iter().map(|s| s.as_str()).collect();
            let payload = json!({"event": "post_edit", "session_id": session_id, "files": list});
            let env_files = list.join("\n");
            out.push(
                self.exec(
                    spec,
                    cmd,
                    HookEvent::PostEdit,
                    Some(session_id),
                    &payload.to_string(),
                    Some(env_files),
                )
                .await,
            );
        }
        out
    }

    async fn exec(
        &self,
        spec: &HookSpec,
        cmd: String,
        event: HookEvent,
        session_id: Option<&str>,
        stdin: &str,
        files_env: Option<String>,
    ) -> HookOutcome {
        let t = std::time::Instant::now();
        let mut o = self
            .exec_inner(spec, cmd, event, session_id, stdin, files_env)
            .await;
        o.duration_ms = t.elapsed().as_millis() as u64;
        o
    }

    async fn exec_inner(
        &self,
        spec: &HookSpec,
        cmd: String,
        event: HookEvent,
        session_id: Option<&str>,
        stdin: &str,
        files_env: Option<String>,
    ) -> HookOutcome {
        let fail = |output: String| HookOutcome {
            spec: spec.clone(),
            success: false,
            output,
            exit_code: None,
            duration_ms: 0,
        };
        let mut c = Command::new("sh");
        c.arg("-c")
            .arg(&cmd)
            .current_dir(&self.cwd)
            .env("EZ_EVENT", event.as_str())
            .env("EZ_PROJECT_DIR", &self.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(s) = session_id {
            c.env("EZ_SESSION_ID", s);
        }
        if let Some(f) = files_env {
            c.env("EZ_FILES", f);
        }
        let mut child = match c.spawn() {
            Ok(c) => c,
            Err(e) => return fail(format!("failed to spawn hook: {e}")),
        };
        if let Some(mut si) = child.stdin.take() {
            let data = stdin.as_bytes().to_vec();
            tokio::spawn(async move {
                let _ = si.write_all(&data).await;
            });
        }
        let mut so = child.stdout.take();
        let mut se = child.stderr.take();
        let read = |r: Option<tokio::process::ChildStdout>| async move {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b).await;
            }
            b
        };
        let read_err = |r: Option<tokio::process::ChildStderr>| async move {
            let mut b = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut b).await;
            }
            b
        };
        let work = async {
            let (o, e, st) = tokio::join!(read(so.take()), read_err(se.take()), child.wait());
            (o, e, st)
        };
        match tokio::time::timeout(Duration::from_secs(spec.timeout_secs), work).await {
            Err(_) => fail(format!("hook timed out after {}s", spec.timeout_secs)),
            Ok((o, e, st)) => {
                let mut text = String::from_utf8_lossy(&o).into_owned();
                text.push_str(&String::from_utf8_lossy(&e));
                match st {
                    Ok(st) => HookOutcome {
                        spec: spec.clone(),
                        success: st.success(),
                        output: truncate_middle(&text, MAX_OUTPUT),
                        exit_code: st.code(),
                        duration_ms: 0,
                    },
                    Err(e) => fail(format!("failed to wait for hook: {e}")),
                }
            }
        }
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn truncate_middle(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head: String = chars[..half].iter().collect();
    let tail: String = chars[chars.len() - half..].iter().collect();
    format!(
        "{head}\n... [{} chars truncated] ...\n{tail}",
        chars.len() - 2 * half
    )
}

/// Build the synthetic user message for a fix turn from failed FixTurn outcomes.
pub fn fix_turn_prompt(outcomes: &[HookOutcome]) -> Option<String> {
    let failed: Vec<&HookOutcome> = outcomes
        .iter()
        .filter(|o| !o.success && o.on_failure() == OnFailure::FixTurn)
        .collect();
    if failed.is_empty() {
        return None;
    }
    let mut s = String::from("The following hook commands failed after your changes:\n");
    for o in failed {
        let code = o
            .exit_code
            .map_or("no exit code".to_string(), |c| format!("exit code {c}"));
        s.push_str(&format!(
            "\n$ {} ({code})\n```\n{}\n```\n",
            o.spec.run,
            o.output.trim_end()
        ));
    }
    s.push_str("\nPlease fix these problems.");
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(run: &str) -> HookSpec {
        HookSpec {
            run: run.into(),
            glob: None,
            matcher: None,
            on_failure: OnFailure::Ignore,
            max_fix_turns: 2,
            timeout_secs: 10,
        }
    }

    #[test]
    fn parses_toml_with_defaults() {
        let cfg: HooksConfig = toml::from_str(
            r#"
            [[post_edit]]
            run = "cargo fmt"
            glob = "*.rs"
            on_failure = "fix_turn"
            [[pre_tool_use]]
            run = "echo hi"
            matcher = "Bash|Edit"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.post_edit[0].on_failure, OnFailure::FixTurn);
        assert_eq!(cfg.post_edit[0].max_fix_turns, 2);
        assert_eq!(cfg.post_edit[0].timeout_secs, 120);
        assert_eq!(cfg.pre_tool_use[0].on_failure, OnFailure::Ignore);
        assert!(!cfg.is_empty());
        assert!(HooksConfig::default().is_empty());
        let m = cfg.merged(&cfg);
        assert_eq!(m.get(HookEvent::PostEdit).len(), 2);
    }

    #[tokio::test]
    async fn matcher_filtering() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = spec("echo a");
        a.matcher = Some("Bash".into());
        let mut b = spec("echo b");
        b.matcher = Some("Edit|Write".into());
        let cfg = HooksConfig {
            pre_tool_use: vec![a, b, spec("echo c")],
            ..Default::default()
        };
        let r = HookRunner::new(dir.path());
        let p = json!({"session_id": "s1"});
        let o = r.run(&cfg, HookEvent::PreToolUse, Some("Bash"), &p).await;
        let outs: Vec<_> = o.iter().map(|o| o.output.trim().to_string()).collect();
        assert_eq!(outs, ["a", "c"]);
        let o = r
            .run(&cfg, HookEvent::PreToolUse, Some("Bashful"), &p)
            .await;
        assert_eq!(o.len(), 1);
        let o = r.run(&cfg, HookEvent::PreToolUse, None, &p).await;
        assert_eq!(o.len(), 1);
    }

    #[tokio::test]
    async fn env_and_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = HooksConfig {
            stop: vec![spec("cat; echo $EZ_EVENT $EZ_SESSION_ID")],
            ..Default::default()
        };
        let r = HookRunner::new(dir.path());
        let o = r
            .run(&cfg, HookEvent::Stop, None, &json!({"session_id": "abc"}))
            .await;
        assert!(o[0].output.contains("stop abc"));
        assert!(o[0].output.contains("\"session_id\""));
    }

    #[tokio::test]
    async fn glob_and_files_substitution() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("echo {files}; echo \"$EZ_FILES\"");
        s.glob = Some("*.rs".into());
        let cfg = HooksConfig {
            post_edit: vec![s],
            ..Default::default()
        };
        let r = HookRunner::new(dir.path());
        let files = vec![
            dir.path().join("src/a.rs"),
            dir.path().join("b.txt"),
            PathBuf::from("it's.rs"),
        ];
        let o = r.run_post_edit(&cfg, "s", &files).await;
        assert_eq!(o.len(), 1);
        assert!(o[0].success);
        assert!(
            o[0].output
                .starts_with("src/a.rs it's.rs\nsrc/a.rs\nit's.rs")
        );
        let o = r
            .run_post_edit(&cfg, "s", &[dir.path().join("x.txt")])
            .await;
        assert!(o.is_empty());
    }

    #[tokio::test]
    async fn failure_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = HooksConfig {
            stop: vec![spec("echo oops >&2; exit 3")],
            ..Default::default()
        };
        let r = HookRunner::new(dir.path());
        let o = r.run(&cfg, HookEvent::Stop, None, &json!({})).await;
        assert!(!o[0].success);
        assert_eq!(o[0].exit_code, Some(3));
        assert_eq!(o[0].output.trim(), "oops");
    }

    #[tokio::test]
    async fn timeout_is_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = spec("sleep 5");
        s.timeout_secs = 1;
        let cfg = HooksConfig {
            stop: vec![s],
            ..Default::default()
        };
        let o = HookRunner::new(dir.path())
            .run(&cfg, HookEvent::Stop, None, &json!({}))
            .await;
        assert!(!o[0].success);
        assert!(o[0].output.contains("timed out"));
    }

    #[test]
    fn fix_prompt() {
        let mut s = spec("cargo clippy");
        s.on_failure = OnFailure::FixTurn;
        let bad = HookOutcome {
            spec: s,
            success: false,
            output: "error: boom".into(),
            exit_code: Some(1),
            duration_ms: 0,
        };
        let ignored = HookOutcome {
            spec: spec("other"),
            success: false,
            output: "x".into(),
            exit_code: Some(1),
            duration_ms: 0,
        };
        assert!(fix_turn_prompt(std::slice::from_ref(&ignored)).is_none());
        let p = fix_turn_prompt(&[bad, ignored]).unwrap();
        assert!(p.contains("cargo clippy") && p.contains("error: boom") && !p.contains("other"));
    }

    #[test]
    fn truncation_keeps_ends() {
        let s = format!("{}{}", "a".repeat(30_000), "z".repeat(30_000));
        let t = truncate_middle(&s, MAX_OUTPUT);
        assert!(t.starts_with('a') && t.ends_with('z') && t.len() < 21_000);
    }
}
