//! Expose `mise` tasks as agent tools.

use async_trait::async_trait;
use ez_tools::{Tool, ToolContext, ToolKind, ToolOutput, truncate_middle};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

const DISCOVER_TIMEOUT: Duration = Duration::from_secs(15);
const RUN_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_OUTPUT: usize = 30_000;
const MAX_TOOL_NAME: usize = 64;

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct MiseTask {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub hide: bool,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub usage: String,
}

/// Parse `mise tasks ls --json` output, dropping hidden tasks.
pub fn parse_tasks(json: &str) -> anyhow::Result<Vec<MiseTask>> {
    let tasks: Vec<MiseTask> = serde_json::from_str(json)?;
    Ok(tasks.into_iter().filter(|t| !t.hide).collect())
}

fn mise_bin() -> String {
    std::env::var("EZ_MISE_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mise".to_string())
}

/// Raw process result.
struct Exec {
    output: String,
    code: i32,
}

enum ExecError {
    Timeout,
    Cancelled,
    Io(std::io::Error),
}

/// Run `program args...` (no shell) capturing combined stdout/stderr.
async fn exec(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<Exec, ExecError> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(ExecError::Io)?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let read = async {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let _ = tokio::join!(stdout.read_to_end(&mut o), stderr.read_to_end(&mut e));
        (o, e)
    };
    let cancelled = async {
        match cancel {
            Some(c) => c.cancelled().await,
            None => std::future::pending().await,
        }
    };
    let ((o, e), status) = tokio::select! {
        r = async { let out = read.await; (out, child.wait().await) } => r,
        _ = tokio::time::sleep(timeout) => return Err(ExecError::Timeout),
        _ = cancelled => return Err(ExecError::Cancelled),
    };
    let status = status.map_err(ExecError::Io)?;
    let mut output = String::from_utf8_lossy(&o).into_owned();
    if !e.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&String::from_utf8_lossy(&e));
    }
    Ok(Exec {
        output,
        code: status.code().unwrap_or(-1),
    })
}

/// Runs `mise tasks ls --json` in cwd. Returns an empty list if mise is missing or fails.
pub async fn discover(cwd: &Path) -> Vec<MiseTask> {
    let args: Vec<String> = ["tasks", "ls", "--json"].map(String::from).to_vec();
    match exec(&mise_bin(), &args, cwd, DISCOVER_TIMEOUT, None).await {
        Ok(r) if r.code == 0 => match parse_tasks(&r.output) {
            Ok(t) => t,
            Err(e) => {
                tracing::debug!("failed to parse mise task list: {e}");
                vec![]
            }
        },
        Ok(r) => {
            tracing::debug!("mise tasks ls exited with {}: {}", r.code, r.output.trim());
            vec![]
        }
        Err(ExecError::Io(e)) => {
            tracing::debug!("mise unavailable: {e}");
            vec![]
        }
        Err(_) => {
            tracing::debug!("mise tasks ls timed out");
            vec![]
        }
    }
}

/// Glob-style selectors over task names (`*` matches any chars including `:`).
pub struct TaskSelector {
    set: GlobSet,
}

impl TaskSelector {
    pub fn new(patterns: &[String]) -> anyhow::Result<Self> {
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            b.add(Glob::new(p)?);
        }
        Ok(Self { set: b.build()? })
    }

    pub fn matches(&self, task: &str) -> bool {
        self.set.is_match(task)
    }

    pub fn select<'a>(&self, tasks: &'a [MiseTask]) -> Vec<&'a MiseTask> {
        tasks.iter().filter(|t| self.matches(&t.name)).collect()
    }
}

/// Tool-name-safe identifier for a task.
pub fn tool_name(task: &str) -> String {
    let mut s = String::from("mise__");
    s.extend(task.chars().map(|c| {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            c
        } else {
            '_'
        }
    }));
    s.truncate(MAX_TOOL_NAME);
    s
}

fn args_schema() -> Value {
    json!({
        "type": "array",
        "items": {"type": "string"},
        "description": "Extra arguments passed to the task after `--`"
    })
}

fn string_args(input: &Value) -> anyhow::Result<Vec<String>> {
    match input.get("args") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow::anyhow!("`args` must be an array of strings"))
            })
            .collect(),
        Some(_) => anyhow::bail!("`args` must be an array of strings"),
    }
}

async fn run_task(task: &str, args: &[String], ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
    let mut argv = vec!["run".to_string(), task.to_string()];
    if !args.is_empty() {
        argv.push("--".into());
        argv.extend(args.iter().cloned());
    }
    match exec(&mise_bin(), &argv, &ctx.cwd, RUN_TIMEOUT, Some(&ctx.cancel)).await {
        Ok(r) => {
            let text = truncate_middle(&r.output, MAX_OUTPUT);
            if r.code == 0 {
                Ok(ToolOutput::ok(if text.is_empty() {
                    "(no output)".to_string()
                } else {
                    text
                }))
            } else {
                Ok(ToolOutput::error(format!("{text}\n[exit code {}]", r.code)))
            }
        }
        Err(ExecError::Timeout) => Ok(ToolOutput::error(format!(
            "mise task timed out after {}s",
            RUN_TIMEOUT.as_secs()
        ))),
        Err(ExecError::Cancelled) => Ok(ToolOutput::error("mise task cancelled")),
        Err(ExecError::Io(e)) => Ok(ToolOutput::error(format!(
            "failed to run mise in {}: {e}",
            ctx.cwd.display()
        ))),
    }
}

fn describe(task: &MiseTask) -> String {
    let mut d = task.description.clone();
    if !task.usage.trim().is_empty() {
        d.push_str("\nUsage spec:\n");
        d.push_str(&task.usage);
    }
    d
}

/// A single mise task exposed as a tool.
pub struct MiseTaskTool {
    pub task: MiseTask,
    name: String,
}

impl MiseTaskTool {
    pub fn new(task: MiseTask) -> Self {
        let name = tool_name(&task.name);
        Self { task, name }
    }
}

#[async_trait]
impl Tool for MiseTaskTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> String {
        format!(
            "{}\n(Runs `mise run {}`.)",
            describe(&self.task),
            self.task.name
        )
        .trim_start()
        .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {"args": args_schema()}})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn title(&self, input: &Value) -> String {
        let args = string_args(input).unwrap_or_default();
        if args.is_empty() {
            format!("mise run {}", self.task.name)
        } else {
            format!("mise run {} -- {}", self.task.name, args.join(" "))
        }
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        run_task(&self.task.name, &string_args(&input)?, ctx).await
    }
}

/// Lazy-loading meta tool: list the allowed tasks.
pub struct MiseListTasks {
    pub tasks: Vec<MiseTask>,
}

#[async_trait]
impl Tool for MiseListTasks {
    fn name(&self) -> &str {
        "MiseListTasks"
    }

    fn description(&self) -> String {
        "List the mise tasks available in this project. Run one with MiseRun.".into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Search
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, _: Value, _: &ToolContext) -> anyhow::Result<ToolOutput> {
        if self.tasks.is_empty() {
            return Ok(ToolOutput::ok("(no mise tasks available)"));
        }
        let lines: Vec<String> = self
            .tasks
            .iter()
            .map(|t| {
                if t.description.is_empty() {
                    t.name.clone()
                } else {
                    format!("{} — {}", t.name, t.description)
                }
            })
            .collect();
        Ok(ToolOutput::ok(lines.join("\n")))
    }
}

/// Lazy-loading meta tool: run an allowed task by name.
pub struct MiseRun {
    pub tasks: Vec<MiseTask>,
}

#[async_trait]
impl Tool for MiseRun {
    fn name(&self) -> &str {
        "MiseRun"
    }

    fn description(&self) -> String {
        "Run a mise task (`mise run <task> -- <args>`). Use MiseListTasks to see available tasks."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "Task name"},
                "args": args_schema()
            },
            "required": ["task"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn title(&self, input: &Value) -> String {
        format!("mise run {}", input["task"].as_str().unwrap_or("?"))
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let task = input
            .get("task")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing required string parameter `task`"))?;
        let Some(found) = self
            .tasks
            .iter()
            .find(|t| t.name == task || t.aliases.iter().any(|a| a == task))
        else {
            return Ok(ToolOutput::error(format!(
                "unknown or disallowed mise task `{task}`; use MiseListTasks to see available tasks"
            )));
        };
        run_task(&found.name, &string_args(&input)?, ctx).await
    }
}

/// Build the tools to expose: one per task, or lazy meta tools above the threshold.
pub fn tools_for(selected: Vec<MiseTask>, lazy_threshold: usize) -> Vec<Arc<dyn Tool>> {
    if selected.len() <= lazy_threshold {
        selected
            .into_iter()
            .map(|t| Arc::new(MiseTaskTool::new(t)) as Arc<dyn Tool>)
            .collect()
    } else {
        vec![
            Arc::new(MiseListTasks {
                tasks: selected.clone(),
            }),
            Arc::new(MiseRun { tasks: selected }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ez_tools::{Answer, Question, UserBridge};
    use tokio_util::sync::CancellationToken;

    struct NoUser;

    #[async_trait]
    impl UserBridge for NoUser {
        async fn ask_user(&self, _: &str, _: Vec<Question>) -> anyhow::Result<Vec<Answer>> {
            anyhow::bail!("no user")
        }
    }

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            session_id: "t".into(),
            user: Arc::new(NoUser),
            cancel: CancellationToken::new(),
        }
    }

    fn task(name: &str) -> MiseTask {
        MiseTask {
            name: name.into(),
            description: format!("desc {name}"),
            aliases: vec![],
            hide: false,
            source: None,
            usage: String::new(),
        }
    }

    const FIXTURE: &str = r#"[
      {"name":"build","aliases":["b"],"description":"Build it","source":"/repo/mise.toml","depends":[],"hide":false,"dir":null,"run":["cargo build"],"usage":""},
      {"name":"secret","hide":true},
      {"name":"test:unit","usage":"arg \"<f>\""}
    ]"#;

    #[test]
    fn parses_and_filters_hidden() {
        let t = parse_tasks(FIXTURE).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].aliases, vec!["b"]);
        assert_eq!(t[0].source.as_deref(), Some("/repo/mise.toml"));
        assert_eq!(t[1].description, "");
        assert!(parse_tasks("nope").is_err());
    }

    #[test]
    fn selectors() {
        let tasks = vec![
            task("test"),
            task("test:unit"),
            task("lint"),
            task("lint-fix"),
        ];
        let sel = |p: &[&str]| {
            let p: Vec<String> = p.iter().map(|s| s.to_string()).collect();
            let s = TaskSelector::new(&p).unwrap();
            s.select(&tasks)
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(sel(&["test"]), vec!["test"]);
        assert_eq!(sel(&["test:*"]), vec!["test:unit"]);
        assert_eq!(sel(&["lint*"]), vec!["lint", "lint-fix"]);
        assert_eq!(sel(&["*"]).len(), 4);
        assert!(sel(&[]).is_empty());
    }

    #[test]
    fn tool_names() {
        assert_eq!(tool_name("test:unit"), "mise__test_unit");
        assert_eq!(tool_name("a b/c.d-e_f"), "mise__a_b_c_d-e_f");
        assert_eq!(tool_name(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn lazy_threshold() {
        let ts = vec![task("a"), task("b")];
        let eager = tools_for(ts.clone(), 2);
        assert_eq!(eager.len(), 2);
        assert_eq!(eager[0].name(), "mise__a");
        assert!(eager[0].description().contains("mise run a"));
        let lazy = tools_for(ts, 1);
        let names: Vec<_> = lazy.iter().map(|t| t.name().to_string()).collect();
        assert_eq!(names, vec!["MiseListTasks", "MiseRun"]);
    }

    #[tokio::test]
    async fn run_rejects_unknown_task() {
        let dir = tempfile::tempdir().unwrap();
        let run = MiseRun {
            tasks: vec![task("a")],
        };
        let out = run
            .call(json!({"task": "zzz"}), &ctx(dir.path()))
            .await
            .unwrap();
        assert!(out.is_error);
        let list = MiseListTasks {
            tasks: vec![task("a")],
        };
        let out = list.call(json!({}), &ctx(dir.path())).await.unwrap();
        assert!(out.content.contains("a — desc a"));
    }

    // Single test fn: it mutates EZ_MISE_BIN.
    #[cfg(unix)]
    #[tokio::test]
    async fn executes_via_fake_mise() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("fake-mise");
        std::fs::write(
            &bin,
            "#!/bin/sh\nif [ \"$1\" = tasks ]; then echo '[{\"name\":\"hi\"},{\"name\":\"h\",\"hide\":true}]'; exit 0; fi\n\
             echo \"args: $*\"; if [ \"$2\" = fail ]; then echo oops >&2; exit 3; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        // SAFETY: test-only; set before or while no other thread reads this variable.
        unsafe {
            std::env::set_var("EZ_MISE_BIN", &bin);
        }

        let found = discover(dir.path()).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "hi");

        let c = ctx(dir.path());
        let tool = MiseTaskTool::new(task("hi"));
        let out = tool
            .call(json!({"args": ["a b", "$HOME"]}), &c)
            .await
            .unwrap();
        assert!(!out.is_error);
        assert_eq!(out.content.trim(), "args: run hi -- a b $HOME");

        let out = MiseTaskTool::new(task("fail"))
            .call(json!({}), &c)
            .await
            .unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("oops") && out.content.contains("[exit code 3]"));

        // SAFETY: test-only; set before or while no other thread reads this variable.
        unsafe {
            std::env::set_var("EZ_MISE_BIN", dir.path().join("missing"));
        }
        assert!(discover(dir.path()).await.is_empty());
        let out = tool.call(json!({}), &c).await.unwrap();
        assert!(out.is_error);
        // SAFETY: test-only; set before or while no other thread reads this variable.
        unsafe {
            std::env::remove_var("EZ_MISE_BIN");
        }
    }
}
