//! Effect builder: deterministic idempotency keys, progress and agent job helpers.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use enzarb_connector::{ChangeRequestSpec, Progress, ProgressTarget, RepoKey};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::prompt::{PromptContext, render_prompt};
use super::{Ctx, Effect, EngineEvent, JobKind};

/// Acknowledgements land within seconds: above everything else.
pub const PRIORITY_PROGRESS: i32 = 100;
/// `Working` is queued after `Received` so the latter is never the last marker set
/// when both come from one event.
pub const PRIORITY_WORKING: i32 = 90;
pub const PRIORITY_COMMENT: i32 = 10;
pub const PRIORITY_AGENT: i32 = 0;

/// `jobs.input` of every `agent_*` job.
///
/// `prompt` is rendered from `context` with the data the machine knows. The
/// machine cannot fetch the thread, so the executor should call
/// `Connector::fetch_thread`, set `context.thread`, and re-render with
/// [`render_prompt`] before `run_start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentJobInput {
    pub kind: JobKind,
    pub repo: RepoKey,
    /// `run_start.base_ref`: where the worktree starts.
    pub base_ref: String,
    /// `run_start.branch`.
    pub branch: String,
    /// `run_start.push`.
    pub push: bool,
    pub timeout_secs: u64,
    pub context: PromptContext,
    pub prompt: String,
}

pub(crate) struct AgentSpec {
    pub base_ref: String,
    pub branch: String,
    pub push: bool,
}

/// Collects effects for one transition.
pub(crate) struct Fx<'a> {
    ctx: &'a Ctx,
    event_id: &'a str,
    seen: HashMap<JobKind, u32>,
    pub effects: Vec<Effect>,
}

impl<'a> Fx<'a> {
    pub fn new(ctx: &'a Ctx, ev: &'a EngineEvent) -> Self {
        Self {
            ctx,
            event_id: &ev.id,
            seen: HashMap::new(),
            effects: Vec::new(),
        }
    }

    /// `<work_item>:<event_id>:<kind>`; a second job of the same kind from the same
    /// event gets `#2`, and so on.
    fn key(&mut self, kind: JobKind) -> String {
        let n = self.seen.entry(kind).or_insert(0);
        *n += 1;
        let base = format!("{}:{}:{}", self.ctx.work_item, self.event_id, kind.as_str());
        if *n == 1 { base } else { format!("{base}#{n}") }
    }

    fn job(&mut self, kind: JobKind, priority: i32, input: serde_json::Value) {
        let idempotency_key = self.key(kind);
        self.effects.push(Effect::Job {
            kind,
            priority,
            idempotency_key,
            input,
        });
    }

    fn progress(&mut self, target: &ProgressTarget, p: Progress, priority: i32) {
        self.job(
            JobKind::SetProgress,
            priority,
            json!({ "target": target, "progress": p }),
        );
    }

    pub fn received(&mut self, target: &ProgressTarget) {
        self.progress(target, Progress::Received, PRIORITY_PROGRESS);
    }

    pub fn done(&mut self, target: &ProgressTarget) {
        self.progress(target, Progress::Done, PRIORITY_PROGRESS);
    }

    pub fn failed(&mut self, target: &ProgressTarget) {
        self.progress(target, Progress::Failed, PRIORITY_PROGRESS);
    }

    pub fn comment(&mut self, body: impl Into<String>) {
        let input = json!({ "item": self.ctx.work_item, "body": body.into() });
        self.job(JobKind::PostComment, PRIORITY_COMMENT, input);
    }

    pub fn open_change_request(&mut self, spec: &ChangeRequestSpec) {
        self.job(
            JobKind::OpenChangeRequest,
            PRIORITY_COMMENT,
            json!({ "spec": spec }),
        );
    }

    pub fn cancel_jobs(&mut self) {
        self.effects.push(Effect::CancelJobs);
    }

    pub fn wake_at(&mut self, at: Option<DateTime<Utc>>) {
        self.effects.push(Effect::SetWakeAt { at });
    }

    /// Queue an agent job and its `Working` marker on `target`.
    pub fn agent(
        &mut self,
        kind: JobKind,
        target: &ProgressTarget,
        spec: AgentSpec,
        context: PromptContext,
    ) {
        debug_assert!(kind.is_agent());
        self.progress(target, Progress::Working, PRIORITY_WORKING);
        let input = AgentJobInput {
            kind,
            repo: self.ctx.work_item.repo.clone(),
            base_ref: spec.base_ref,
            branch: spec.branch,
            push: spec.push,
            timeout_secs: self.ctx.config.agent_timeout_secs,
            prompt: render_prompt(kind, &context),
            context,
        };
        self.job(
            kind,
            PRIORITY_AGENT,
            serde_json::to_value(input).expect("agent input serializes"),
        );
    }

    pub fn finish(self) -> Vec<Effect> {
        self.effects
    }
}
