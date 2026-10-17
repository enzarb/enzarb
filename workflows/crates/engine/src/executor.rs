//! The generic executor: one queue (`engine.jobs`), one backoff, dispatch by kind. Every result
//! comes back as an inbox event for the reducer.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use enzarb_flow::document::GitFacts;
use enzarb_flow::format::Access;
use enzarb_flow::interp::{RunExec, RunStep, StepFinished as FlowFinished, TaskEvent, TaskState};
use enzarb_proto::{
    Mode, Runtime, Secret, SessionEvent, StepExec, StepOutput, StepStart, StepWorkspace,
};
use serde_json::{Value, json};
use sqlx::Row;
use tokio::sync::{Semaphore, mpsc, watch};

use crate::engine::{Engine, repo_key};
use crate::error::Error;
use crate::reducer::ENGINE_PREFIX;
use crate::steps::StepError;

/// Lease length; renewed while a job runs.
const LEASE: Duration = Duration::from_secs(60);
/// Session-event payloads above this are cut to head + tail.
pub const MAX_PAYLOAD: usize = 1 << 20;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub task_id: i64,
    pub project: String,
    pub kind: String,
    pub input: Value,
    pub attempt: i32,
    pub created_at: DateTime<Utc>,
}

/// Outcome of one dispatch.
#[allow(clippy::large_enum_variant)]
enum Done {
    /// Finished; the reducer gets this event (type suffix, payload).
    Event(TaskEvent),
    /// Nothing to report (steer, stop).
    Quiet,
}

impl Engine {
    pub async fn run_executor(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), Error> {
        let slots = Arc::new(Semaphore::new(self.cfg.executor_slots));
        loop {
            if *shutdown.borrow() {
                return Ok(());
            }
            let permit = slots.clone().acquire_owned().await.expect("semaphore");
            match self.claim().await? {
                Some(job) => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        me.execute(job).await;
                        drop(permit);
                    });
                }
                None => {
                    drop(permit);
                    tokio::select! {
                        _ = shutdown.changed() => return Ok(()),
                        _ = tokio::time::sleep(self.cfg.poll) => {}
                    }
                }
            }
        }
    }

    /// Claims the next runnable job (queued and due, or an expired lease). Steps respect the
    /// Project capacity, `concurrency: {group, max}` and sandbox drain holds.
    pub async fn claim(&self) -> Result<Option<Job>, Error> {
        let row = sqlx::query(
            "UPDATE engine.jobs j SET status = 'running', lease_owner = $1,
                    lease_until = now() + make_interval(secs => $2), attempt = j.attempt + 1,
                    updated_at = now()
             WHERE j.id = (
               SELECT c.id FROM engine.jobs c
               WHERE ((c.status = 'queued' AND c.run_after <= now())
                      OR (c.status = 'running' AND c.lease_until < now()))
                 AND (c.kind <> 'run_step' OR (
                   NOT EXISTS (SELECT 1 FROM engine.sandbox_holds h WHERE h.project = c.project)
                   AND (SELECT count(*) FROM engine.jobs r WHERE r.kind = 'run_step' AND r.project = c.project
                          AND r.status = 'running' AND r.lease_until >= now() AND r.id <> c.id) < $3
                   AND (c.concurrency_group IS NULL OR
                        (SELECT count(*) FROM engine.jobs g WHERE g.concurrency_group = c.concurrency_group
                          AND g.status = 'running' AND g.lease_until >= now() AND g.id <> c.id) < c.concurrency_max)))
               ORDER BY c.priority DESC, c.run_after, c.id
               FOR UPDATE SKIP LOCKED LIMIT 1)
             RETURNING j.id, j.task_id, j.project, j.kind, j.input, j.attempt, j.created_at",
        )
        .bind(&self.cfg.worker_id)
        .bind(LEASE.as_secs_f64())
        .bind(self.cfg.max_steps_per_project)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| Job {
            id: r.get("id"),
            task_id: r.get("task_id"),
            project: r.get("project"),
            kind: r.get("kind"),
            input: r.get("input"),
            attempt: r.get("attempt"),
            created_at: r.get("created_at"),
        }))
    }

    /// Runs one job to completion (keeping its lease alive) and records the outcome.
    pub async fn execute(&self, job: Job) {
        let (stop_tx, mut stop_rx) = watch::channel(false);
        let pool = self.pool.clone();
        let id = job.id;
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => return,
                    _ = tokio::time::sleep(LEASE / 3) => {
                        let _ = sqlx::query("UPDATE engine.jobs SET lease_until = now() + make_interval(secs => $2) WHERE id = $1 AND status = 'running'")
                            .bind(id).bind(LEASE.as_secs_f64()).execute(&pool).await;
                    }
                }
            }
        });
        let res = match job.kind.as_str() {
            "run_step" => self.run_step(&job).await,
            "steer" => self.steer(&job).await,
            "stop_step" => self.stop_step(&job).await,
            "call_action" => self.call_action(&job).await,
            "refresh_fact" => self.refresh_fact(&job).await,
            k => Err(StepError::Other(format!("unknown job kind {k}"))),
        };
        let _ = stop_tx.send(true);
        heartbeat.abort();
        if let Err(e) = self.finish(&job, res).await {
            tracing::error!(job = job.id, "recording job outcome: {e}");
        }
    }

    async fn finish(&self, job: &Job, res: Result<Done, StepError>) -> Result<(), Error> {
        let mut tx = self.pool.begin().await?;
        match res {
            Ok(done) => {
                let (status, ev) = match done {
                    Done::Event(e) => ("done", Some(e)),
                    Done::Quiet => ("done", None),
                };
                sqlx::query("UPDATE engine.jobs SET status = $2, lease_owner = NULL, updated_at = now() WHERE id = $1")
                    .bind(job.id)
                    .bind(status)
                    .execute(&mut *tx)
                    .await?;
                if let Some(e) = ev {
                    let kind = serde_json::to_value(&e)
                        .ok()
                        .and_then(|v| v.get("kind").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_default();
                    let dedupe = format!("job:{}:{}:{kind}", job.id, job.attempt);
                    self.insert_event(
                        &mut tx,
                        &dedupe,
                        &job.project,
                        job.task_id,
                        &format!("{ENGINE_PREFIX}{kind}"),
                        serde_json::to_value(&e).unwrap_or_default(),
                    )
                    .await?;
                }
            }
            Err(e) => {
                // The one retry policy: infrastructure errors retry with backoff, forever (no
                // budgets); the error is visible on the job.
                let wait = self.backoff(job.attempt);
                tracing::warn!(job = job.id, kind = %job.kind, "retrying in {wait:?}: {e}");
                sqlx::query(
                    "UPDATE engine.jobs SET status = 'queued', lease_owner = NULL, error = $2,
                            run_after = now() + make_interval(secs => $3), updated_at = now() WHERE id = $1",
                )
                .bind(job.id)
                .bind(e.to_string())
                .bind(wait.as_secs_f64())
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }

    async fn task_state(
        &self,
        task_id: i64,
    ) -> Result<(String, Option<String>, TaskState), StepError> {
        let r = sqlx::query("SELECT repo, started_by, state FROM engine.tasks WHERE id = $1")
            .bind(task_id)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        let state: TaskState =
            serde_json::from_value(r.get("state")).map_err(|e| StepError::Other(e.to_string()))?;
        Ok((r.get("repo"), r.get("started_by"), state))
    }

    // ------------------------------------------------------------ run_step

    async fn run_step(&self, job: &Job) -> Result<Done, StepError> {
        let rs: RunStep = serde_json::from_value(job.input["step"].clone())
            .map_err(|e| StepError::Other(format!("bad run_step input: {e}")))?;
        let (repo, started_by, state) = self.task_state(job.task_id).await?;
        let info = self.index.get(&job.project);
        let default_branch = info
            .as_ref()
            .and_then(|i| i.default_branch(&repo).map(str::to_string))
            .unwrap_or_else(|| "main".into());

        // Secrets: the step's plus its runtime's, by name, most specific scope first.
        let mut names = rs.secrets.clone();
        if let RunExec::Agent { runtime, .. } = &rs.exec {
            names.extend(runtime.file.secrets.iter().map(|s| s.normalized()));
        }
        names.dedup();
        // The task's user is the Project owner (whose Claude token pays for the run); the
        // starter only when the Project is unknown to the index.
        let user = info
            .as_ref()
            .map(|i| i.owner.clone())
            .filter(|o| !o.is_empty())
            .or(started_by);
        let resolved = crate::secrets::resolve(
            &self.pool,
            self.secrets_key.as_ref(),
            self.claude.as_ref(),
            user.as_deref(),
            &job.project,
            &names,
        )
        .await
        .map_err(StepError::Other)?;
        let (env, files, missing) = (resolved.env, resolved.files, resolved.missing);
        if !missing.is_empty() {
            let reason = format!(
                "missing secret(s) {} for step {}: set them, then resume",
                missing.join(", "),
                rs.node
            );
            return self.not_started(&rs, reason).await;
        }

        // Workspaces: the task's repo first, then the step's extra ones.
        let task_branch = format!("enzarb/task-{}", job.task_id);
        let pr_head = state
            .doc
            .subjects
            .pull_requests
            .iter()
            .find(|p| p.repo.eq_ignore_ascii_case(&repo) && p.state == "open")
            .and_then(|p| p.head.clone());
        let writes = rs.github.get("contents") == Some(&Access::Write);
        // A workspace entry for the task's own repo may set its branch and base.
        let own = rs
            .workspaces
            .iter()
            .find(|w| w.repo.eq_ignore_ascii_case(&repo));
        let nonempty = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
        let mut ws = vec![(
            repo.clone(),
            if writes || own.is_some_and(|w| w.mode == Access::Write) {
                Mode::Write
            } else {
                Mode::Read
            },
            own.and_then(|w| nonempty(&w.branch))
                .or(pr_head)
                .unwrap_or_else(|| task_branch.clone()),
            own.and_then(|w| nonempty(&w.base))
                .unwrap_or_else(|| default_branch.clone()),
        )];
        for w in &rs.workspaces {
            if !w.repo.eq_ignore_ascii_case(&repo) {
                let base = nonempty(&w.base).unwrap_or_else(|| {
                    self.index
                        .lookup(&w.repo)
                        .and_then(|i| i.default_branch(&w.repo).map(str::to_string))
                        .unwrap_or_else(|| "main".into())
                });
                ws.push((
                    w.repo.to_ascii_lowercase(),
                    if w.mode == Access::Write {
                        Mode::Write
                    } else {
                        Mode::Read
                    },
                    nonempty(&w.branch).unwrap_or_else(|| task_branch.clone()),
                    base,
                ));
            }
        }
        let mut workspaces = Vec::new();
        for (r, mode, branch, base) in &ws {
            let scope = if *mode == Mode::Write {
                enzarb_connector::CredScope::ReadWrite
            } else {
                enzarb_connector::CredScope::Read
            };
            let cred = match self
                .connector()
                .mint_clone_credential(&repo_key(r), scope, &format!("{}:{r}", rs.run_id))
                .await
            {
                Ok(c) => c,
                Err(e) if refused(&e) => {
                    let reason = format!(
                        "the GitHub App cannot get a {} clone token for {r} needed by step {}: {e}",
                        if *mode == Mode::Write {
                            "write"
                        } else {
                            "read"
                        },
                        rs.node
                    );
                    return self.not_started(&rs, reason).await;
                }
                Err(e) => return Err(StepError::Unavailable(format!("clone token for {r}: {e}"))),
            };
            workspaces.push(StepWorkspace {
                repo: r.clone(),
                url: format!("https://github.com/{r}.git"),
                mode: *mode,
                branch: branch.clone(),
                base: base.clone(),
                push: *mode == Mode::Write,
                token: Secret::new(cred.token),
            });
        }
        // A token with exactly the requested permissions, plus `contents: read` so the agent can
        // fetch its workspaces (the clone token is not left in the workspace). Steps without
        // permissions get that read-only token alone.
        let mut perms: BTreeMap<String, String> = rs
            .github
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    if *v == Access::Write { "write" } else { "read" }.to_string(),
                )
            })
            .collect();
        perms
            .entry("contents".into())
            .or_insert_with(|| "read".into());
        let repos: Vec<_> = ws.iter().map(|(r, ..)| repo_key(r)).collect();
        let github_token = match self.connector().mint_step_token(&repos, &perms).await {
            Ok(t) => Some(Secret::new(t.token)),
            Err(e) if refused(&e) => {
                let asked = perms
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let reason = format!(
                    "the GitHub App installation lacks {asked} requested by step {}; grant them \
                     in the App settings and accept them in the installation settings (a \
                     permission change may be pending acceptance there), or change the step ({e})",
                    rs.node
                );
                return self.not_started(&rs, reason).await;
            }
            Err(e) => return Err(StepError::Unavailable(format!("step token: {e}"))),
        };
        let exec = match &rs.exec {
            RunExec::Agent {
                runtime,
                model,
                prompt,
            } => StepExec::Agent {
                runtime: Box::new(Runtime {
                    name: runtime.name.clone(),
                    install: runtime.file.install.mise.clone(),
                    protocol: match runtime.file.launch.protocol {
                        enzarb_flow::format::Protocol::Acp => "acp".into(),
                        enzarb_flow::format::Protocol::StdioJson => "stdio-json".into(),
                    },
                    command: runtime.file.launch.command.clone(),
                    env: runtime.file.launch.env.clone(),
                    capabilities: serde_json::to_value(runtime.file.capabilities)
                        .unwrap_or_default(),
                }),
                model: model.clone(),
                prompt: prompt.clone(),
                result_schema: rs.result.as_ref().map(|r| r.schema.clone()),
            },
            RunExec::Command { run, tools } => StepExec::Command {
                run: run.clone(),
                tools: tools.mise.clone(),
            },
        };
        let start = StepStart {
            run_id: rs.run_id.clone(),
            attempt: rs.attempt,
            task_id: job.task_id.to_string(),
            workspaces,
            exec,
            outputs: rs
                .outputs
                .iter()
                .map(|(k, o)| {
                    (
                        k.clone(),
                        StepOutput {
                            file: o.file.clone(),
                            format: o.format.clone(),
                            optional: o.optional,
                        },
                    )
                })
                .collect(),
            env: env.into_iter().map(|(k, v)| (k, Secret::new(v))).collect(),
            files: files
                .into_iter()
                .map(|(k, v)| (k, Secret::new(v)))
                .collect(),
            github_token,
        };

        let after_seq: i64 = sqlx::query_scalar(
            "UPDATE engine.step_runs SET started_at = COALESCE(started_at, now()),
                 queue_secs = CASE WHEN started_at IS NULL THEN extract(epoch FROM now() - queued_at) ELSE queue_secs END,
                 last_activity_at = COALESCE(last_activity_at, now())
             WHERE run_id = $1
             RETURNING COALESCE((SELECT max(seq) FROM engine.session_events WHERE run_id = $1), 0)::bigint",
        )
        .bind(&rs.run_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .unwrap_or(0);

        let (tx, rx) = mpsc::unbounded_channel();
        let writer = tokio::spawn(store_session_events(self.pool.clone(), rx));
        let res = self
            .runner
            .run(&job.project, start, after_seq as u64, tx)
            .await;
        let _ = writer.await;
        let out = res?;
        if out.resumed {
            sqlx::query("UPDATE engine.step_runs SET resumes = resumes + 1 WHERE run_id = $1")
                .bind(&rs.run_id)
                .execute(&self.pool)
                .await
                .map_err(db)?;
        }
        let f = out.finished;

        // Never trust sandbox-reported facts: the branch head must be on GitHub.
        let mut git = None;
        for (i, g) in f.git.iter().enumerate() {
            let head = self
                .connector()
                .branch_head(&repo_key(&g.repo), &g.branch)
                .await
                .map_err(|e| StepError::Unavailable(format!("verifying {}: {e}", g.repo)))?;
            let pushed = g.pushed && head.as_deref() == Some(g.head_sha.as_str());
            if i == 0 {
                let base = ws
                    .iter()
                    .find(|(r, ..)| *r == g.repo)
                    .map(|(.., b)| b.clone())
                    .unwrap_or_else(|| default_branch.clone());
                let ahead = if pushed {
                    self.connector()
                        .ahead_by(&repo_key(&g.repo), &base, &g.head_sha)
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or(0)
                } else {
                    0
                };
                git = Some(GitFacts {
                    branch: g.branch.clone(),
                    head: head.clone().unwrap_or_default(),
                    pushed,
                    ahead: ahead as u32,
                    changed_files: Vec::new(),
                });
            }
        }
        let mut exit = f.exit.clone();
        let mut result = f.result.clone();
        let mut note = f.error.clone();
        if let (Some(spec), Some(r)) = (&rs.result, &result)
            && let Err(e) = crate::schema::validate(&spec.schema, r)
        {
            note = Some(format!("result does not match its schema: {e}"));
            result = None;
            exit = "failure".into();
        }
        let usage = f.usage.clone();
        sqlx::query(
            "UPDATE engine.step_runs SET ended_at = now(), exit = $2, tokens = $3, cost_usd = $4::float8::numeric,
                 active_secs = extract(epoch FROM now() - started_at),
                 sandbox = COALESCE(sandbox, '{}'::jsonb) || jsonb_build_object('error', $5::text)
             WHERE run_id = $1",
        )
        .bind(&rs.run_id)
        .bind(&exit)
        .bind(usage.as_ref().map(|u| json!({"input": u.input_tokens, "output": u.output_tokens, "cache_read": u.cache_read_input_tokens, "cache_creation": u.cache_creation_input_tokens, "models": u.models})))
        .bind(usage.as_ref().and_then(|u| u.cost_usd))
        .bind(note)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(Done::Event(TaskEvent::StepFinished(FlowFinished {
            run_id: f.run_id,
            exit,
            outputs: f.outputs,
            result,
            git,
        })))
    }

    /// The step cannot start for a reason retrying will not fix: nothing is sent to the sandbox,
    /// the run is recorded as `not_started` and the task pauses at the step (see
    /// [`TaskEvent::StepNotStarted`]).
    async fn not_started(&self, rs: &RunStep, reason: String) -> Result<Done, StepError> {
        tracing::warn!(run = %rs.run_id, "step not started: {reason}");
        sqlx::query(
            "UPDATE engine.step_runs SET ended_at = now(), exit = 'not_started',
                 sandbox = COALESCE(sandbox, '{}'::jsonb) || jsonb_build_object('error', $2::text)
             WHERE run_id = $1",
        )
        .bind(&rs.run_id)
        .bind(&reason)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(Done::Event(TaskEvent::StepNotStarted {
            run_id: rs.run_id.clone(),
            reason,
        }))
    }

    async fn steer(&self, job: &Job) -> Result<Done, StepError> {
        let run_id = job.input["run_id"].as_str().unwrap_or_default();
        let text = job.input["text"].as_str().unwrap_or_default();
        self.runner.steer(&job.project, run_id, text).await?;
        Ok(Done::Quiet)
    }

    async fn stop_step(&self, job: &Job) -> Result<Done, StepError> {
        let run_id = job.input["run_id"].as_str().unwrap_or_default();
        self.runner.stop(&job.project, run_id).await?;
        Ok(Done::Quiet)
    }

    async fn call_action(&self, job: &Job) -> Result<Done, StepError> {
        let call_id = job.input["call_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let name = job.input["name"].as_str().unwrap_or_default().to_string();
        let args = job.input["args"].as_object().cloned().unwrap_or_default();
        let (repo, _, state) = self.task_state(job.task_id).await?;
        let key = format!("{}:{call_id}", job.task_id);
        let (ok, result) = match crate::actions::call(self, &repo, &state, &name, &args, &key).await
        {
            Ok(v) => (true, v),
            Err(crate::actions::ActionError::Refused(why)) => (false, json!({"error": why})),
            Err(crate::actions::ActionError::Transient(e)) => {
                return Err(StepError::Unavailable(e));
            }
        };
        Ok(Done::Event(TaskEvent::ActionFinished {
            call_id,
            ok,
            result,
        }))
    }

    async fn refresh_fact(&self, job: &Job) -> Result<Done, StepError> {
        let fact = job.input["fact"].as_str().unwrap_or_default().to_string();
        let (repo, _, state) = self.task_state(job.task_id).await?;
        let value = crate::actions::fact(self, job.task_id, &repo, &state, &fact)
            .await
            .map_err(StepError::Unavailable)?;
        sqlx::query(
            "INSERT INTO engine.facts (task_id, name, value, source) VALUES ($1, $2, $3, 'github')
             ON CONFLICT (task_id, name) DO UPDATE SET value = EXCLUDED.value, at = now()",
        )
        .bind(job.task_id)
        .bind(&fact)
        .bind(&value)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(Done::Event(TaskEvent::FactUpdated { fact, value }))
    }
}

/// A provider refusal that retrying the same request cannot fix (GitHub 4xx such as
/// "The permissions requested are not granted to this installation").
fn refused(e: &enzarb_connector::Error) -> bool {
    matches!(
        e,
        enzarb_connector::Error::Invalid(_) | enzarb_connector::Error::PermissionDenied(_)
    )
}

fn db(e: sqlx::Error) -> StepError {
    StepError::Unavailable(format!("database: {e}"))
}

/// Caps a payload at [`MAX_PAYLOAD`], keeping head and tail.
pub fn cap_payload(payload: Value) -> (Value, bool) {
    let s = payload.to_string();
    if s.len() <= MAX_PAYLOAD {
        return (payload, false);
    }
    let half = MAX_PAYLOAD / 2 - 64;
    let mut a = half;
    while !s.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = s.len() - half;
    while !s.is_char_boundary(b) {
        b += 1;
    }
    (
        json!({"head": &s[..a], "tail": &s[b..], "bytes": s.len()}),
        true,
    )
}

async fn store_session_events(pool: sqlx::PgPool, mut rx: mpsc::UnboundedReceiver<SessionEvent>) {
    while let Some(e) = rx.recv().await {
        let (payload, cut) = cap_payload(e.payload);
        let at = DateTime::parse_from_rfc3339(&e.at)
            .map(|t| t.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        let res = sqlx::query(
            "WITH ins AS (
                INSERT INTO engine.session_events (run_id, seq, at, kind, payload, truncated)
                VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT DO NOTHING RETURNING 1)
             UPDATE engine.step_runs SET last_activity_at = GREATEST(COALESCE(last_activity_at, $3), $3)
             WHERE run_id = $1",
        )
        .bind(&e.run_id)
        .bind(e.seq as i32)
        .bind(at)
        .bind(&e.kind)
        .bind(payload)
        .bind(e.truncated || cut)
        .execute(&pool)
        .await;
        if let Err(err) = res {
            tracing::warn!(run = %e.run_id, seq = e.seq, "storing session event: {err}");
        }
    }
}
