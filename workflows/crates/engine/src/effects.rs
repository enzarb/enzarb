//! Effects become jobs, timers, relations and the projection, in the reducer's transaction.

use enzarb_flow::events::Payload;
use enzarb_flow::interp::{Effect, Projection, RunExec, RunStep};
use serde_json::{Value, json};
use sqlx::{Postgres, Transaction};

use crate::engine::{Engine, subject_key};
use crate::error::Error;
use crate::reducer::TaskRow;

type Tx<'a> = Transaction<'a, Postgres>;

impl Engine {
    pub(crate) async fn queue_effect(
        &self,
        tx: &mut Tx<'_>,
        task: &TaskRow,
        event_id: Option<i64>,
        fx: &Effect,
    ) -> Result<(), Error> {
        let ev = event_id.map(|e| e.to_string()).unwrap_or_else(|| "start".into());
        match fx {
            Effect::RunStep(rs) => self.queue_step(tx, task, rs).await?,
            Effect::Steer { run_id, event } => {
                let text = match &event.payload {
                    Payload::Comment(c) => c.body.clone(),
                    _ => serde_json::to_string(event).unwrap_or_default(),
                };
                let login = event.actor.as_ref().map(|a| a.login.as_str()).unwrap_or("");
                let text = if login.is_empty() { text } else { format!("{login}: {text}") };
                self.queue_job(tx, task, "steer", json!({"run_id": run_id, "text": text}),
                    &format!("steer:{}:{run_id}:{ev}", task.id), 10, None, None).await?;
                sqlx::query("UPDATE engine.step_runs SET steers = steers + 1 WHERE run_id = $1")
                    .bind(run_id)
                    .execute(&mut **tx)
                    .await?;
            }
            Effect::StopStep { run_id } => {
                self.queue_job(tx, task, "stop_step", json!({"run_id": run_id}),
                    &format!("stop:{}:{run_id}", task.id), 10, None, None).await?;
            }
            Effect::CallAction { call_id, name, args } => {
                self.queue_job(tx, task, "call_action",
                    json!({"call_id": call_id, "name": name, "args": args}),
                    &format!("action:{}:{call_id}", task.id), 5, None, None).await?;
            }
            Effect::RefreshFact { fact } => {
                self.queue_job(tx, task, "refresh_fact", json!({"fact": fact}),
                    &format!("fact:{}:{fact}:{ev}", task.id), 5, None, None).await?;
            }
            Effect::SetTimer { timer_id, seconds } => {
                sqlx::query(
                    "INSERT INTO engine.timers (task_id, timer_id, fire_at)
                     VALUES ($1, $2, now() + make_interval(secs => $3))
                     ON CONFLICT (task_id, timer_id) DO UPDATE SET fire_at = EXCLUDED.fire_at, fired = false",
                )
                .bind(task.id)
                .bind(timer_id)
                .bind(*seconds as f64)
                .execute(&mut **tx)
                .await?;
            }
            Effect::CreateRelations { kind, targets } => {
                for t in targets {
                    if let Some(key) = relation_key(&task.repo, t) {
                        sqlx::query(
                            "INSERT INTO engine.relations (task_id, kind, target_key, source)
                             VALUES ($1, $2, $3, 'workflow') ON CONFLICT DO NOTHING",
                        )
                        .bind(task.id)
                        .bind(kind)
                        .bind(key)
                        .execute(&mut **tx)
                        .await?;
                    }
                }
            }
            Effect::UpdateProjection(p) => self.write_projection(tx, task, p).await?,
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn queue_job(
        &self,
        tx: &mut Tx<'_>,
        task: &TaskRow,
        kind: &str,
        input: Value,
        key: &str,
        priority: i32,
        group: Option<&str>,
        max: Option<i32>,
    ) -> Result<(), Error> {
        sqlx::query(
            "INSERT INTO engine.jobs (task_id, project, kind, input, idempotency_key, priority,
                                      concurrency_group, concurrency_max)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(task.id)
        .bind(&task.project)
        .bind(kind)
        .bind(input)
        .bind(key)
        .bind(priority)
        .bind(group)
        .bind(max)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn queue_step(&self, tx: &mut Tx<'_>, task: &TaskRow, rs: &RunStep) -> Result<(), Error> {
        let (runtime, model) = match &rs.exec {
            RunExec::Agent { runtime, model, .. } => (Some(runtime.name.clone()), model.clone()),
            RunExec::Command { .. } => (None, None),
        };
        self.queue_job(
            tx,
            task,
            "run_step",
            json!({"step": rs}),
            &format!("run:{}:{}:{}", task.id, rs.run_id, rs.attempt),
            rs.priority,
            rs.concurrency.as_ref().map(|c| c.group.as_str()),
            rs.concurrency.as_ref().map(|c| c.max as i32),
        )
        .await?;
        sqlx::query(
            "INSERT INTO engine.step_runs (run_id, task_id, project, node, step, attempt, runtime, model)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (run_id) DO UPDATE SET attempt = EXCLUDED.attempt, queued_at = now(),
                started_at = NULL, ended_at = NULL, exit = NULL",
        )
        .bind(&rs.run_id)
        .bind(task.id)
        .bind(&task.project)
        .bind(&rs.node)
        .bind(&rs.step)
        .bind(rs.attempt as i32)
        .bind(runtime)
        .bind(model)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    async fn write_projection(&self, tx: &mut Tx<'_>, task: &TaskRow, p: &Projection) -> Result<(), Error> {
        let d = &task.state.doc.subjects;
        let title = d
            .issue
            .as_ref()
            .map(|i| i.title.clone())
            .or_else(|| d.pull_requests.first().map(|p| p.title.clone()))
            .unwrap_or_default();
        sqlx::query(
            "INSERT INTO engine.projection (task_id, project, repo, workflow, title, subjects, status,
                 end_status, phase, node, \"column\", waiting_on, reason, text, actions, artifacts,
                 needs_you, cost_usd, updated_at)
             SELECT t.id, t.project, t.repo, t.workflow_ref, $2,
                 COALESCE((SELECT jsonb_agg(jsonb_build_object('kind', s.kind, 'key', s.key, 'url', s.url)
                                            ORDER BY s.linked_at)
                           FROM engine.subjects s WHERE s.task_id = t.id), '[]'),
                 $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                 COALESCE((SELECT sum(cost_usd) FROM engine.step_runs r WHERE r.task_id = t.id), 0),
                 now()
             FROM engine.tasks t WHERE t.id = $1
             ON CONFLICT (task_id) DO UPDATE SET title = EXCLUDED.title, subjects = EXCLUDED.subjects,
                 status = EXCLUDED.status, end_status = EXCLUDED.end_status, phase = EXCLUDED.phase,
                 node = EXCLUDED.node, \"column\" = EXCLUDED.\"column\", waiting_on = EXCLUDED.waiting_on,
                 reason = EXCLUDED.reason, text = EXCLUDED.text, actions = EXCLUDED.actions,
                 artifacts = EXCLUDED.artifacts, needs_you = EXCLUDED.needs_you,
                 cost_usd = EXCLUDED.cost_usd, updated_at = now()",
        )
        .bind(task.id)
        .bind(title)
        .bind(&p.status)
        .bind(&p.end_status)
        .bind(&p.phase)
        .bind(&p.node)
        .bind(&p.column)
        .bind(p.waiting_on.map(|w| serde_json::to_value(w).unwrap_or_default()))
        .bind(&p.reason)
        .bind(&p.text)
        .bind(serde_json::to_value(&p.actions).unwrap_or_default())
        .bind(serde_json::to_value(&p.artifacts).unwrap_or_default())
        .bind(p.needs_you)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }
}

/// A relation target: a task id, a subject key, an issue number (same repo) or `{repo, number}`.
fn relation_key(repo: &str, t: &Value) -> Option<String> {
    match t {
        Value::Number(n) => n.as_u64().map(|n| subject_key(repo, n)),
        Value::String(s) if s.starts_with("github:") || s.starts_with("task:") => Some(s.clone()),
        Value::String(s) => s
            .trim_start_matches('#')
            .parse::<u64>()
            .ok()
            .map(|n| subject_key(repo, n)),
        Value::Object(o) => {
            if let Some(task) = o.get("task") {
                return Some(format!("task:{}", task.as_str().map(str::to_string).unwrap_or_else(|| task.to_string())));
            }
            let n = o.get("number").and_then(Value::as_u64)?;
            let r = o.get("repo").and_then(Value::as_str).unwrap_or(repo);
            Some(subject_key(r, n))
        }
        _ => None,
    }
}
