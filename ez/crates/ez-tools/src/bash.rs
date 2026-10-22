use crate::{Tool, ToolContext, ToolKind, ToolOutput, str_arg, truncate_middle};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT: usize = 30_000;

pub struct Bash;

#[async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> String {
        "Run a shell command in the project root and return combined stdout/stderr and the exit code. \
         Prefer dedicated tools (Read, Grep, Glob, mise tasks) when they fit."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "description": {"type": "string", "description": "Short description of what the command does"},
                "timeout": {"type": "integer", "description": "Timeout in milliseconds (max 600000)"}
            },
            "required": ["command"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Execute
    }

    fn title(&self, input: &Value) -> String {
        input["description"]
            .as_str()
            .or_else(|| input["command"].as_str())
            .unwrap_or("Bash")
            .to_string()
    }

    fn locations(&self, _: &Value) -> Vec<String> {
        vec![]
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let command = str_arg(&input, "command")?;
        let timeout = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let r = run_shell(command, &ctx.cwd, Duration::from_millis(timeout), ctx).await?;
        Ok(r)
    }
}

/// Run `sh -c command`, honoring cancellation and a timeout.
pub async fn run_shell(
    command: &str,
    cwd: &std::path::Path,
    timeout: Duration,
    ctx: &ToolContext,
) -> anyhow::Result<ToolOutput> {
    let mut child = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let read = async {
        let (mut o, mut e) = (Vec::new(), Vec::new());
        let _ = tokio::join!(stdout.read_to_end(&mut o), stderr.read_to_end(&mut e));
        (o, e)
    };
    let result = tokio::select! {
        r = async { let out = read.await; (out, child.wait().await) } => r,
        _ = tokio::time::sleep(timeout) => {
            return Ok(ToolOutput::error(format!("command timed out after {}ms", timeout.as_millis())));
        }
        _ = ctx.cancel.cancelled() => {
            return Ok(ToolOutput::error("command cancelled"));
        }
    };
    let ((o, e), status) = result;
    let status = status?;
    let mut text = String::from_utf8_lossy(&o).into_owned();
    if !e.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&e));
    }
    let text = truncate_middle(&text, MAX_OUTPUT);
    let code = status.code().unwrap_or(-1);
    if status.success() {
        Ok(ToolOutput::ok(if text.is_empty() {
            "(no output)".to_string()
        } else {
            text
        }))
    } else {
        Ok(ToolOutput::error(format!("{text}\n[exit code {code}]")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::ctx;

    #[tokio::test]
    async fn runs_and_reports_exit() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let ok = Bash.call(json!({"command": "echo hi"}), &c).await.unwrap();
        assert_eq!(ok.content.trim(), "hi");
        let bad = Bash
            .call(json!({"command": "echo no >&2; exit 3"}), &c)
            .await
            .unwrap();
        assert!(bad.is_error);
        assert!(bad.content.contains("no") && bad.content.contains("exit code 3"));
    }
}
