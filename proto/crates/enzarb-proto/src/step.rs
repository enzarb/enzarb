//! Step frames (ADR-0022, `docs/workflows/contracts-v2.md`): the engine runs one
//! step per `step_start` in a task's persistent workspace. The sandbox knows
//! nothing about workflows: it gets a rendered prompt or command, the runtime to
//! launch, workspaces with tokens, secrets, declared outputs and an optional
//! result schema, and reports exit, outputs, result and git facts (which the
//! engine re-verifies).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ctrl::{Secret, Usage};

/// Lifecycle of a step in the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepPhase {
    Running,
    /// Interrupted by a sandbox restart; `step_resume` continues it.
    Interrupted,
    Finished,
    /// The sandbox does not know the run.
    Unknown,
}

/// `read` or `write`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Read,
    Write,
}

/// A repository checked out for the step under `/workspace/tasks/<task_id>/<owner>/<repo>`.
/// The first workspace is the primary one (the step's working directory).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StepWorkspace {
    /// `owner/repo`.
    pub repo: String,
    /// Clone URL (https).
    pub url: String,
    pub mode: Mode,
    /// Branch the workspace works on (created from `base` when missing remotely).
    pub branch: String,
    /// Branch to start from when `branch` does not exist yet.
    pub base: String,
    /// Commit and push `branch` after the step (write workspaces only), with a
    /// lease on the remote head seen when the step started.
    #[serde(default)]
    pub push: bool,
    /// Short-lived token for fetching and pushing this repo.
    pub token: Secret,
}

impl std::fmt::Debug for StepWorkspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepWorkspace")
            .field("repo", &self.repo)
            .field("mode", &self.mode)
            .field("branch", &self.branch)
            .field("base", &self.base)
            .field("push", &self.push)
            .finish_non_exhaustive()
    }
}

/// An agent runtime, resolved by the engine from the library's runtime file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Runtime {
    pub name: String,
    /// mise tool specs to install (`npm:@agentclientprotocol/claude-agent-acp@0.5`).
    #[serde(default)]
    pub install: Vec<String>,
    /// `acp` or `stdio-json`.
    pub protocol: String,
    /// argv of the agent process (run with `mise exec`).
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// `{sessions, resume, steering, interactive, structured_output}`.
    #[serde(default)]
    pub capabilities: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepExec {
    Agent {
        runtime: Runtime,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        prompt: String,
        /// JSON Schema of the `submit_result` MCP tool; offered only when set.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_schema: Option<Value>,
    },
    Command {
        run: String,
        /// mise tool specs installed before the command.
        #[serde(default)]
        tools: Vec<String>,
    },
}

/// A declared output file, read from the primary workspace after the step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepOutput {
    pub file: String,
    /// `markdown`, `json` (parsed) or `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default)]
    pub optional: bool,
}

/// Body of `step_start`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct StepStart {
    pub run_id: String,
    pub attempt: u32,
    /// Task id; workspaces live under `/workspace/tasks/<task_id>/`.
    pub task_id: String,
    pub workspaces: Vec<StepWorkspace>,
    pub exec: StepExec,
    #[serde(default)]
    pub outputs: BTreeMap<String, StepOutput>,
    /// Named secrets as environment variables (injected into this step's process only).
    #[serde(default)]
    pub env: BTreeMap<String, Secret>,
    /// Short-lived GitHub token with exactly the step's requested permissions (`GH_TOKEN`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_token: Option<Secret>,
}

impl std::fmt::Debug for StepStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StepStart")
            .field("run_id", &self.run_id)
            .field("attempt", &self.attempt)
            .field("task_id", &self.task_id)
            .field("workspaces", &self.workspaces)
            .field("exec", &self.exec)
            .field("outputs", &self.outputs)
            .field("env", &self.env.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl StepStart {
    /// Every secret value the step got, for redaction.
    pub fn secret_values(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.env.values().map(Secret::expose).collect();
        v.extend(self.workspaces.iter().map(|w| w.token.expose()));
        v.extend(self.github_token.iter().map(Secret::expose));
        v.retain(|s| !s.is_empty());
        v
    }
}

/// Session event kinds (an open set; receivers tolerate others).
pub mod session_kind {
    pub const PROMPT: &str = "prompt";
    pub const USER: &str = "user";
    pub const ASSISTANT: &str = "assistant";
    pub const THINKING: &str = "thinking";
    pub const TOOL_CALL: &str = "tool_call";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const USAGE: &str = "usage";
    pub const STEER: &str = "steer";
    pub const SYSTEM: &str = "system";
    pub const STDOUT: &str = "stdout";
    pub const STDERR: &str = "stderr";
}

/// One captured, redacted session event. `seq` starts at 1 and is dense per run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub run_id: String,
    pub seq: u64,
    /// RFC 3339.
    pub at: String,
    pub kind: String,
    pub payload: Value,
    #[serde(default)]
    pub truncated: bool,
}

/// Git state of one write workspace after the step, as the sandbox saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepGit {
    pub repo: String,
    pub branch: String,
    pub head_sha: String,
    pub pushed: bool,
}

/// Body of `step_finished` (and `step_state.result`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepFinished {
    pub run_id: String,
    /// `success`, `failure` or `interrupted` (see [`exit`]).
    pub exit: String,
    #[serde(default)]
    pub outputs: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// One entry per write workspace, in `step_start.workspaces` order.
    #[serde(default)]
    pub git: Vec<StepGit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Why the step failed, for humans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `step_finished.exit` values.
pub mod exit {
    pub const SUCCESS: &str = "success";
    pub const FAILURE: &str = "failure";
    pub const INTERRUPTED: &str = "interrupted";
}
