//! Effect builder: deterministic idempotency keys, progress and agent job helpers.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use enzarb_connector::{ChangeRequestSpec, Progress, ProgressTarget, RepoKey};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::prompt::{PromptContext, SubIssue, render_prompt};
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
    /// Set by the executor once the one recovery run (a re-run asking only for the final
    /// `json` block) has been started for an unparseable result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub recovery_used: bool,
    /// `agent_segment` jobs: the graph segment to run (`RunStart.graph`). The executor
    /// re-renders its `prompts` once it has the thread, like `prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<enzarb_proto::GraphSegment>,
    /// `agent_segment` jobs: the pinned prompt files (path → text) the executor renders
    /// file prompts from.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub prompt_files: std::collections::BTreeMap<String, String>,
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

    /// Ask the connector for the head's aggregate CI and mergeability; the result is
    /// applied only if the head is still `head_sha`.
    pub fn check_merge_readiness(&mut self, number: u64, head_sha: &str) {
        let input = json!({
            "item": self.ctx.work_item,
            "number": number,
            "head_sha": head_sha,
        });
        self.job(JobKind::CheckMergeReadiness, PRIORITY_COMMENT, input);
    }

    /// Merge the base into the change request's branch (`Connector::update_branch`: never a
    /// rebase or force-push), pinned to `expected_head_sha`.
    pub fn update_branch(&mut self, number: u64, expected_head_sha: &str) {
        let input = json!({
            "item": self.ctx.work_item,
            "number": number,
            "expected_head_sha": expected_head_sha,
        });
        self.job(JobKind::UpdateBranch, PRIORITY_COMMENT, input);
    }

    /// The human-triggered squash merge. Only the UI `merge` handler may call this.
    pub fn merge_change_request(
        &mut self,
        number: u64,
        title: &str,
        expected_head_sha: &str,
        actor: &str,
        delete_branch: Option<&str>,
    ) {
        let mut input = json!({
            "item": self.ctx.work_item,
            "number": number,
            "title": title,
            "expected_head_sha": expected_head_sha,
            "actor": actor,
        });
        // `delete_branch` is the head branch to delete once merged, only set on request.
        if let (Some(branch), Some(o)) = (delete_branch, input.as_object_mut()) {
            o.insert("delete_branch".into(), json!(branch));
        }
        self.job(JobKind::MergeChangeRequest, PRIORITY_COMMENT, input);
    }

    /// Rebase enzarb's own stacked child branch `head_ref` (expected at `expected_sha`) from
    /// `old_base` onto `onto`, push it with a lease and set the change request's base.
    /// Queued only by the change request machine, for a child whose parent was merged.
    pub fn retarget_change_request(
        &mut self,
        number: u64,
        head_ref: &str,
        expected_sha: &str,
        old_base: &str,
        onto: &str,
    ) {
        let input = json!({
            "item": self.ctx.work_item,
            "number": number,
            "head_ref": head_ref,
            "expected_sha": expected_sha,
            "old_base": old_base,
            "onto": onto,
        });
        self.job(JobKind::RetargetChangeRequest, PRIORITY_COMMENT, input);
    }

    /// Create `subs` (already in creation order) as sub-issues of this epic. `depth` is the
    /// children's nesting depth (the epic's own plus one).
    pub fn create_sub_issues(&mut self, depth: u32, subs: &[SubIssue]) {
        let input = json!({
            "item": self.ctx.work_item,
            "depth": depth,
            "sub_issues": subs,
        });
        self.job(JobKind::CreateSubIssues, PRIORITY_COMMENT, input);
    }

    /// Close this work item's issue with `reason`. Runs after the comments queued with it
    /// (lower priority), so the summary is posted before the issue closes.
    pub fn close_issue(&mut self, reason: enzarb_connector::CloseReason) {
        let input = json!({ "item": self.ctx.work_item, "reason": reason });
        self.job(JobKind::CloseIssue, PRIORITY_COMMENT - 1, input);
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
            recovery_used: false,
            graph: None,
            prompt_files: Default::default(),
        };
        self.job(
            kind,
            PRIORITY_AGENT,
            serde_json::to_value(input).expect("agent input serializes"),
        );
    }

    /// Queue a graph segment run (`agent_segment`) and its `Working` marker on `target`.
    pub fn segment_with_files(
        &mut self,
        target: &ProgressTarget,
        spec: AgentSpec,
        context: PromptContext,
        segment: enzarb_proto::GraphSegment,
        prompt_files: std::collections::BTreeMap<String, String>,
    ) {
        self.progress(target, Progress::Working, PRIORITY_WORKING);
        let input = AgentJobInput {
            kind: JobKind::AgentSegment,
            repo: self.ctx.work_item.repo.clone(),
            base_ref: spec.base_ref,
            branch: spec.branch,
            push: spec.push,
            timeout_secs: self.ctx.config.agent_timeout_secs,
            prompt: String::new(),
            context,
            recovery_used: false,
            graph: Some(segment),
            prompt_files,
        };
        self.job(
            JobKind::AgentSegment,
            PRIORITY_AGENT,
            serde_json::to_value(input).expect("agent input serializes"),
        );
    }

    pub fn finish(self) -> Vec<Effect> {
        self.effects
    }
}
