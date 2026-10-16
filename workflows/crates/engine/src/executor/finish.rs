//! Ingesting a run's final [`RunFinished`], however it arrived: a `run_finished` push on the
//! executor's mux session ([`results`](super::results)), a `run_state.result` read by the
//! results sweep or the deadline poll, or (legacy sandboxes) the HTTP callback.
//!
//! Every path ends in [`Executor::ingest_run_finished`], which is idempotent: results of runs
//! that are not the job's current run, or of jobs that are no longer `running`, are dropped,
//! usage is counted once per run, the recovery claim and the session-limit and restart
//! outcomes are guarded by the job's status and attempt, and the job event's dedupe key is
//! `job:<id>:<attempt>`.
//!
//! # Push facts are not trusted (#192)
//! The sandbox is agent-writable, so for jobs that push (`agent_implement`,
//! `agent_address_comments`, `agent_resolve_conflicts`, `agent_segment` with `push`) the
//! reported `branch`, `head_sha` and `pushed` are replaced by what the remote says
//! ([`Executor::verify_push`]): `branch` is the job's own, and `pushed` is true iff the remote
//! branch exists at the reported `head_sha`, or, when `head_sha` is not a commit sha, the
//! remote branch is ahead of `base_ref` (then `head_sha` becomes the remote head). A job that
//! does not push reports `pushed: false`.
//!
//! A recovery run (`-r1`, started with `push: false` to get a well-formed result) never
//! changes the push facts: the original run's `branch`, `head_sha` and `pushed` are kept in
//! `input.recovery_push` when the recovery is claimed, and the recovery's result only brings
//! the decision, title and body (its `summary`). The remote check then decides.

use enzarb_proto::{RunFinished, RunOutcome};
use serde_json::{Value, json};

use super::callback::{current_run_id, recovery_used};
use super::restart::RestartOutcome;
use super::session_limit::LimitOutcome;
use super::{Executor, JobError};
use crate::machine::{AgentJobInput, AgentOutput, JobKind, OutputError};
use crate::results::{JobResult, ingest_job_result, record_job_usage};

/// Why a result could not be ingested (nothing was recorded but possibly usage; retry later).
#[derive(Debug)]
pub(crate) enum IngestError {
    /// `run_id` is not `job-<id>[...]`.
    BadRunId,
    /// No agent or retarget job with that id (in that Project).
    UnknownJob,
    /// Database trouble.
    Db(String),
    /// The push could not be checked against the remote (connector error).
    Unverified(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRunId => f.write_str("unrecognized run_id"),
            Self::UnknownJob => f.write_str("unknown job"),
            Self::Db(e) => write!(f, "database error: {e}"),
            Self::Unverified(e) => write!(f, "cannot verify the push: {e}"),
        }
    }
}

impl From<sqlx::Error> for IngestError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e.to_string())
    }
}

impl From<crate::error::Error> for IngestError {
    fn from(e: crate::error::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// The job id in `job-<id>`, `job-<id>-<n>`, `job-<id>[-<n>]-r1`.
pub(crate) fn job_id_of(run_id: &str) -> Option<i64> {
    run_id
        .strip_prefix("job-")
        .map(|n| n.split_once('-').map_or(n, |(id, _)| id))
        .and_then(|n| n.parse::<i64>().ok())
}

/// The columns ingestion reads (`generation` = session-limit waits + restarts).
#[derive(sqlx::FromRow)]
struct FinishJob {
    status: String,
    attempt: i32,
    generation: i32,
    project_id: Option<String>,
    input: Value,
    kind: String,
}

/// Job kinds whose run pushes the job's branch.
fn pushes(kind: JobKind) -> bool {
    matches!(
        kind,
        JobKind::AgentImplement
            | JobKind::AgentAddressComments
            | JobKind::AgentResolveConflicts
            | JobKind::AgentSegment
    )
}

/// A full commit sha (SHA-1 or SHA-256), lowercase hex.
fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The push facts of the original run, stored when its recovery is claimed.
fn push_facts(body: &RunFinished) -> Value {
    json!({ "branch": body.branch, "head_sha": body.head_sha, "pushed": body.pushed })
}

/// Put the original run's push facts (`input.recovery_push`) back on a recovery result.
fn merge_recovery_push(input: &Value, body: &mut RunFinished) {
    let Some(p) = input.get("recovery_push") else {
        return;
    };
    if let Some(b) = p.get("branch").and_then(Value::as_str) {
        body.branch = b.to_string();
    }
    if let Some(s) = p.get("head_sha").and_then(Value::as_str) {
        body.head_sha = s.to_string();
    }
    if let Some(x) = p.get("pushed").and_then(Value::as_bool) {
        body.pushed = x;
    }
}

impl Executor {
    /// Record a run's final result (see the module docs). `project`, when given, is the
    /// Project whose sandbox delivered it: a job of another Project is [`IngestError::UnknownJob`].
    /// Returns whether it changed the job (`false`: stale, already recorded, or another run).
    pub(crate) async fn ingest_run_finished(
        &self,
        project: Option<&str>,
        mut body: RunFinished,
    ) -> Result<bool, IngestError> {
        let run_id = body.run_id.clone();
        let job_id = job_id_of(&run_id).ok_or(IngestError::BadRunId)?;
        let row: Option<FinishJob> = sqlx::query_as(
            "SELECT status, attempt, limit_wait_count + restart_retries AS generation, project_id,
                    input, kind
             FROM jobs WHERE id = $1 AND (kind LIKE 'agent\\_%' OR kind = 'retarget_change_request')",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(FinishJob {
            status,
            attempt,
            generation,
            project_id,
            input,
            kind,
        }) = row
        else {
            return Err(IngestError::UnknownJob);
        };
        if project.is_some_and(|p| project_id.as_deref() != Some(p)) {
            tracing::warn!(
                job_id,
                ?project,
                "a sandbox reported a run of another Project's job"
            );
            return Err(IngestError::UnknownJob);
        }
        // The spend counts for any outcome and even for a stale result, once per run.
        if let Some(usage) = &body.usage {
            record_job_usage(&self.pool, job_id, &run_id, usage).await?;
        }
        if status != "running" {
            tracing::debug!(job_id, %status, "result for a job that is not running; ignored");
            return Ok(false);
        }
        let recovery_used = recovery_used(&input);
        if run_id != current_run_id(job_id, generation, recovery_used) {
            tracing::debug!(job_id, %run_id, "result of an earlier run of this job; ignored");
            return Ok(false);
        }

        if kind == JobKind::RetargetChangeRequest.as_str() {
            let result = self.finish_retarget(&input, &body).await;
            return Ok(ingest_job_result(&self.pool, job_id, attempt, result)
                .await?
                .is_some());
        }

        if recovery_used {
            merge_recovery_push(&input, &mut body);
        }
        let result = match body.outcome {
            RunOutcome::Succeeded => {
                let output = serde_json::to_value(&body).unwrap_or(Value::Null);
                match unparseable_output(&input, &output) {
                    None => {
                        self.verify_push(&input, &mut body)
                            .await
                            .map_err(|e| IngestError::Unverified(e.message))?;
                        JobResult::Succeeded {
                            output: serde_json::to_value(&body).unwrap_or(Value::Null),
                        }
                    }
                    Some(e) => {
                        let error = format!(
                            "unusable agent output: {e}; tail of output: {}",
                            tail(&body.summary, TAIL_BYTES)
                        );
                        if recovery_used {
                            JobResult::Failed { error }
                        } else {
                            match self
                                .recover(
                                    job_id,
                                    attempt,
                                    generation,
                                    project_id.as_deref(),
                                    &input,
                                    &body,
                                )
                                .await?
                            {
                                Recovery::Started => return Ok(true),
                                Recovery::Stale => return Ok(false),
                                Recovery::CouldNotStart => JobResult::Failed { error },
                            }
                        }
                    }
                }
            }
            RunOutcome::Failed => {
                let error = format!(
                    "the agent run failed (exit code {}): {}",
                    body.exit_code.map_or("none".into(), |c| c.to_string()),
                    truncate(&body.summary, 2000)
                );
                match self
                    .session_limit_outcome(job_id, attempt, &body.summary, &error)
                    .await?
                {
                    LimitOutcome::Waiting => return Ok(true),
                    LimitOutcome::Stale => return Ok(false),
                    LimitOutcome::NotALimit => JobResult::Failed { error },
                    LimitOutcome::GaveUp(error) => JobResult::Failed { error },
                }
            }
            RunOutcome::Cancelled => {
                // The job is still `running`, so no transition cancelled it: the sandbox restarted.
                let reason = "the agent stopped with cancelled";
                match self.restart_outcome(job_id, attempt, reason).await? {
                    RestartOutcome::Requeued => return Ok(true),
                    RestartOutcome::Stale => return Ok(false),
                    RestartOutcome::GaveUp(error) => JobResult::Failed { error },
                }
            }
        };
        Ok(ingest_job_result(&self.pool, job_id, attempt, result)
            .await?
            .is_some())
    }

    /// Replace the sandbox-reported push facts of `body` with the remote's (module docs).
    /// Connector errors are transient: nothing is recorded and the result is retried.
    pub(crate) async fn verify_push(
        &self,
        input: &Value,
        body: &mut RunFinished,
    ) -> Result<(), JobError> {
        let Ok(input) = serde_json::from_value::<AgentJobInput>(input.clone()) else {
            body.pushed = false;
            return Ok(());
        };
        if !pushes(input.kind) {
            return Ok(());
        }
        let reported = body.pushed;
        body.branch = input.branch.clone();
        if !input.push {
            body.pushed = false;
        } else {
            let connector = self.connector_for(&input.repo.connector)?;
            let remote = connector.branch_head(&input.repo, &input.branch).await?;
            body.pushed = match remote {
                None => false,
                Some(r) if is_sha(&body.head_sha) => r == body.head_sha,
                Some(r) => {
                    let ahead = connector.ahead_by(&input.repo, &input.base_ref, &r).await?;
                    let pushed = ahead.is_some_and(|n| n > 0);
                    if pushed {
                        body.head_sha = r;
                    }
                    pushed
                }
            };
        }
        if body.pushed != reported {
            tracing::info!(branch = %body.branch, head_sha = %body.head_sha, reported,
                verified = body.pushed, "push facts corrected from the remote");
        }
        // A segment's agent nodes report `pushed` too; none of them pushed if the branch is
        // not on the remote at the segment's head.
        if !body.pushed
            && let Some(g) = body.graph.as_mut()
        {
            for out in g.outputs.values_mut() {
                if let Some(o) = out.as_object_mut()
                    && o.get("pushed").and_then(Value::as_bool) == Some(true)
                {
                    o.insert("pushed".into(), Value::Bool(false));
                }
            }
        }
        Ok(())
    }

    /// Claim the job's single recovery (keeping the original run's push facts) and start the
    /// recovery run.
    #[allow(clippy::too_many_arguments)]
    async fn recover(
        &self,
        job_id: i64,
        attempt: i32,
        generation: i32,
        project: Option<&str>,
        input: &Value,
        original: &RunFinished,
    ) -> Result<Recovery, sqlx::Error> {
        let claimed = sqlx::query(
            "UPDATE jobs SET input = jsonb_set(jsonb_set(input, '{recovery_used}', 'true'),
                                               '{recovery_push}', $3),
                    updated_at = now()
             WHERE id = $1 AND status = 'running' AND attempt = $2
               AND NOT COALESCE((input->>'recovery_used')::bool, false)",
        )
        .bind(job_id)
        .bind(attempt)
        .bind(push_facts(original))
        .execute(&self.pool)
        .await?;
        if claimed.rows_affected() == 0 {
            return Ok(Recovery::Stale);
        }
        let (Some(project), Ok(input)) = (
            project,
            serde_json::from_value::<AgentJobInput>(input.clone()),
        ) else {
            return Ok(Recovery::CouldNotStart);
        };
        match self
            .start_recovery(job_id, attempt, generation, project, &input)
            .await
        {
            Ok(()) => Ok(Recovery::Started),
            Err(e) => {
                tracing::warn!(job_id, error = %e.message, "could not start the recovery run");
                Ok(Recovery::CouldNotStart)
            }
        }
    }
}

/// How much of the end of an unusable output goes into the error.
const TAIL_BYTES: usize = 2048;

enum Recovery {
    /// The recovery run was started; the job stays `running`.
    Started,
    /// The attempt is no longer running, or another delivery already claimed the recovery.
    Stale,
    /// The run could not be started; fail the job.
    CouldNotStart,
}

/// The parse error if `output` of the agent job `input` has no usable `json` block (missing, or
/// not valid JSON). Other problems (unknown decision, empty body) are not recoverable by
/// asking again for the block.
fn unparseable_output(input: &Value, output: &Value) -> Option<OutputError> {
    let kind = serde_json::from_value::<JobKind>(input.get("kind")?.clone()).ok()?;
    // A segment's output is the graph result; the kernel reads it, not a `json` block.
    if kind == JobKind::AgentSegment {
        return None;
    }
    match AgentOutput::from_job_output(kind, output) {
        Err(e @ (OutputError::NoJsonBlock | OutputError::BadJson(_))) => Some(e),
        _ => None,
    }
}

/// The last `max` bytes of `s`, cut on a char boundary and prefixed with `…` when cut.
fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_ids_from_run_ids() {
        assert_eq!(job_id_of("job-7"), Some(7));
        assert_eq!(job_id_of("job-7-2"), Some(7));
        assert_eq!(job_id_of("job-7-2-r1"), Some(7));
        assert_eq!(job_id_of("job-7-r1"), Some(7));
        assert_eq!(job_id_of("run-7"), None);
        assert_eq!(job_id_of("job-x"), None);
    }

    #[test]
    fn shas() {
        assert!(is_sha(&"a".repeat(40)));
        assert!(is_sha(&"0".repeat(64)));
        assert!(!is_sha("abc123"));
        assert!(!is_sha(&"A".repeat(40)));
        assert!(!is_sha(""));
    }

    #[test]
    fn recovery_keeps_the_original_push_facts() {
        let original = RunFinished {
            run_id: "job-5830".into(),
            outcome: RunOutcome::Succeeded,
            exit_code: Some(0),
            summary: "no block".into(),
            branch: "enzarb/issue-164".into(),
            head_sha: "b5a9ea3".repeat(6)[..40].to_string(),
            pushed: true,
            usage: None,
            graph: None,
        };
        let input = json!({ "recovery_used": true, "recovery_push": push_facts(&original) });
        let mut rec = RunFinished {
            run_id: "job-5830-r1".into(),
            summary: "```json\n{\"decision\":\"done\",\"body\":\"b\"}\n```".into(),
            branch: "other".into(),
            head_sha: String::new(),
            pushed: false,
            ..original.clone()
        };
        merge_recovery_push(&input, &mut rec);
        assert_eq!(rec.branch, original.branch);
        assert_eq!(rec.head_sha, original.head_sha);
        assert!(rec.pushed);
        assert!(rec.summary.contains("done"));
        // Without stored facts (a recovery claimed by an older executor) nothing changes.
        let mut plain = rec.clone();
        plain.pushed = false;
        merge_recovery_push(&json!({}), &mut plain);
        assert!(!plain.pushed);
    }

    #[test]
    fn tail_keeps_the_end_on_char_boundaries() {
        assert_eq!(tail("short", 10), "short");
        assert_eq!(tail("abcdef", 3), "…def");
        // "é" is two bytes: cutting inside it moves forward to the next boundary.
        assert_eq!(tail("aéb", 2), "…b");
        assert_eq!(tail("aéb", 3), "…éb");
        assert_eq!(tail("", 3), "");
    }

    #[test]
    fn only_missing_or_bad_blocks_are_unparseable() {
        let input = json!({ "kind": "agent_triage" });
        let out = |s: &str| json!({ "summary": s });
        assert!(unparseable_output(&input, &out("no block")).is_some());
        assert!(unparseable_output(&input, &out("```json\n{\"decision\": 1}\n```")).is_some());
        let ok = "```json\n{\"decision\":\"ready\",\"body\":\"b\"}\n```";
        assert!(unparseable_output(&input, &out(ok)).is_none());
        let unknown = "```json\n{\"decision\":\"nope\",\"body\":\"b\"}\n```";
        assert!(unparseable_output(&input, &out(unknown)).is_none());
        assert!(unparseable_output(&input, &json!({})).is_none());
        assert!(unparseable_output(&json!({}), &out("no block")).is_none());
    }
}
