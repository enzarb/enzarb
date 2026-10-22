//! System prompt assembly.

use std::path::Path;

use crate::modes::ModeSet;

const BASE: &str = "\
You are ez, an interactive coding agent running in the user's terminal or editor. You help \
with software engineering tasks by reading, searching, editing and running code with the tools \
provided.

- Be concise and direct. Do the work rather than describing it; do not narrate each step.
- Prefer the dedicated tools (Read, Glob, Grep, Edit, Write) over shell commands for files.
- Read code before changing it and match the style of the surrounding code.
- Tools named `mise__*` (or MiseRun) run the project's own mise tasks; prefer them for builds, \
tests, linting and other project workflows.
- Use TodoWrite to plan and track multi-step work.
- Use AskUserQuestion when you are blocked on a decision only the user can make.
- Work happens in lifecycle modes. When the current phase's work is done, use SwitchMode to \
propose moving to the next phase; the user may need to approve.
- Do not commit, push or publish unless explicitly asked.";

const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", ".ez/INSTRUCTIONS.md"];

async fn git_branch(cwd: &Path) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

pub async fn system_prompt(cwd: &Path, modes: &ModeSet, mode_id: &str) -> String {
    let mut s = String::from(BASE);
    s.push_str("\n\n## Environment\n");
    s.push_str(&format!("- Working directory: {}\n", cwd.display()));
    s.push_str(&format!("- Platform: {}\n", std::env::consts::OS));
    s.push_str(&format!(
        "- Today's date: {}\n",
        chrono::Utc::now().format("%Y-%m-%d")
    ));
    if let Some(b) = git_branch(cwd).await {
        s.push_str(&format!("- Git branch: {b}\n"));
    }
    for f in INSTRUCTION_FILES {
        if let Ok(text) = std::fs::read_to_string(cwd.join(f)) {
            s.push_str(&format!(
                "\n## Project instructions ({f})\n{}\n",
                text.trim()
            ));
        }
    }
    if let Some(mode) = modes.get(mode_id) {
        s.push_str(&format!(
            "\n## Current mode: {}\n{}\n{}\n",
            modes.display_name(mode_id),
            mode.description,
            mode.prompt
        ));
    }
    if modes.modes.len() > 1 {
        s.push_str("\n## Modes\n");
        for (id, m) in &modes.modes {
            s.push_str(&format!("- {id}: {}\n", m.description));
        }
    }
    s
}
