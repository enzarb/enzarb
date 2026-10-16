//! The ticker: generic time-based building blocks only. Timers become `Timer` events, quiet
//! running steps become `StallDetected`, session partitions are created ahead, pending secrets are
//! encrypted, sandbox drain holds are reconciled, and the adopt pass runs once on start.

use enzarb_flow::events::{Adopted, Event, Payload, SubjectKind, SubjectRef};
use enzarb_flow::interp::TaskEvent;
use serde_json::json;
use sqlx::Row;
use tokio::sync::watch;

use crate::engine::{Engine, repo_key, subject_key};
use crate::error::Error;
use crate::reducer::ENGINE_PREFIX;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TickStats {
    pub timers: u64,
    pub stalls: u64,
    pub secrets: u64,
}

impl Engine {
    pub async fn run_ticker(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), Error> {
        if let Err(e) = self.adopt().await {
            tracing::warn!("adopt pass: {e}");
        }
        loop {
            if let Err(e) = self.tick().await {
                tracing::warn!("tick: {e}");
            }
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                _ = tokio::time::sleep(self.cfg.poll.max(std::time::Duration::from_secs(1)) * 5) => {}
            }
        }
    }

    pub async fn tick(&self) -> Result<TickStats, Error> {
        let mut st = TickStats::default();
        // Timers.
        let due = sqlx::query(
            "UPDATE engine.timers t SET fired = true FROM engine.tasks k
             WHERE k.id = t.task_id AND NOT t.fired AND t.fire_at <= now()
             RETURNING t.task_id, t.timer_id, k.project",
        )
        .fetch_all(&self.pool)
        .await?;
        for r in &due {
            let (task, timer, project): (i64, String, String) = (r.get(0), r.get(1), r.get(2));
            self.emit(
                &format!("timer:{task}:{timer}"),
                &project,
                task,
                &TaskEvent::Timer { timer_id: timer },
            )
            .await?;
            st.timers += 1;
        }
        // Stall detection: no session events for `stall`; reported once per quiet period.
        let stalled = sqlx::query(
            "UPDATE engine.step_runs SET stalls = stalls + 1, stall_reported_at = now()
             WHERE started_at IS NOT NULL AND ended_at IS NULL
               AND COALESCE(last_activity_at, started_at) < now() - make_interval(secs => $1)
               AND (stall_reported_at IS NULL OR stall_reported_at < COALESCE(last_activity_at, started_at))
             RETURNING run_id, task_id, project, stalls",
        )
        .bind(self.cfg.stall.as_secs_f64())
        .fetch_all(&self.pool)
        .await?;
        for r in &stalled {
            let (run, task, project, n): (String, i64, String, i32) =
                (r.get(0), r.get(1), r.get(2), r.get(3));
            let reason = format!(
                "step {run} has shown no activity for {} minutes",
                self.cfg.stall.as_secs() / 60
            );
            self.emit(
                &format!("stall:{run}:{n}"),
                &project,
                task,
                &TaskEvent::StallDetected {
                    reason: Some(reason),
                },
            )
            .await?;
            st.stalls += 1;
        }
        // Partitions for this and next month.
        sqlx::query("SELECT engine.ensure_session_partition(now()::date), engine.ensure_session_partition((now() + interval '1 month')::date)")
            .execute(&self.pool)
            .await?;
        if let Some(k) = &self.secrets_key {
            st.secrets = crate::secrets::encrypt_pending(&self.pool, k).await?;
        }
        // Runtime roles may be created after the migration ran.
        if let Err(e) = sqlx::query("SELECT engine.apply_grants()")
            .execute(&self.pool)
            .await
        {
            tracing::debug!("grants: {e}");
        }
        if let Err(e) = self.reconcile_holds().await {
            tracing::warn!("sandbox holds: {e}");
        }
        Ok(st)
    }

    async fn emit(
        &self,
        dedupe: &str,
        project: &str,
        task: i64,
        ev: &TaskEvent,
    ) -> Result<(), Error> {
        let v = serde_json::to_value(ev).unwrap_or_default();
        let kind = v
            .get("kind")
            .and_then(|k| k.as_str())
            .unwrap_or("")
            .to_string();
        sqlx::query(
            "INSERT INTO engine.events (dedupe_key, project, task_id, type, payload)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (dedupe_key) DO NOTHING",
        )
        .bind(dedupe)
        .bind(project)
        .bind(task)
        .bind(format!("{ENGINE_PREFIX}{kind}"))
        .bind(v)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// First start: an `adopted` event for every open issue that has no task, so bindings decide.
    /// Idempotent (dedupe key per issue).
    pub async fn adopt(&self) -> Result<u64, Error> {
        let mut n = 0;
        for p in self.index.all() {
            for (repo, _) in &p.repos {
                let issues = match self.connector().list_open_issues(&repo_key(repo)).await {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::warn!(%repo, "adopt: listing issues: {e}");
                        continue;
                    }
                };
                for i in issues {
                    let key = subject_key(repo, i.number);
                    let ev = Event {
                        id: Some(format!("adopt:{key}")),
                        repo: Some(repo.clone()),
                        actor: None,
                        subject: Some(SubjectRef {
                            kind: SubjectKind::Issue,
                            number: i.number,
                        }),
                        at: Some(chrono::Utc::now().to_rfc3339()),
                        ..Event::new(Payload::Adopted(Adopted {
                            reason: "first start".into(),
                        }))
                    };
                    let done = sqlx::query(
                        "INSERT INTO engine.events (dedupe_key, project, subject_key, type, payload)
                         SELECT $1, $2, $3, 'adopted', $4
                         WHERE NOT EXISTS (SELECT 1 FROM engine.subjects WHERE key = $3)
                         ON CONFLICT (dedupe_key) DO NOTHING",
                    )
                    .bind(format!("adopt:{key}"))
                    .bind(&p.name)
                    .bind(&key)
                    .bind(serde_json::to_value(&ev).unwrap_or_else(|_| json!({})))
                    .execute(&self.pool)
                    .await?;
                    n += done.rows_affected();
                }
            }
        }
        Ok(n)
    }

    /// Checkpointed sandbox rolls: while a Project has a pending pod change, no new step starts;
    /// once none runs, the drain annotation lets the operator roll. A step interrupted by a forced
    /// roll is resumed (`step_status` reports it interrupted and the executor resumes it).
    pub async fn reconcile_holds(&self) -> Result<(), Error> {
        let states = match self.sandboxes.change_states().await {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!("change states: {e}");
                return Ok(());
            }
        };
        for s in states {
            match s.pending_generation {
                Some(g) => {
                    sqlx::query(
                        "INSERT INTO engine.sandbox_holds (project, generation) VALUES ($1, $2)
                         ON CONFLICT (project) DO UPDATE SET generation = EXCLUDED.generation",
                    )
                    .bind(&s.name)
                    .bind(g)
                    .execute(&self.pool)
                    .await?;
                    let running: i64 = sqlx::query_scalar(
                        "SELECT count(*) FROM engine.jobs WHERE project = $1 AND kind = 'run_step'
                         AND status = 'running' AND lease_until >= now()",
                    )
                    .bind(&s.name)
                    .fetch_one(&self.pool)
                    .await?;
                    if running == 0 && s.drained.as_deref() != Some(g.to_string().as_str()) {
                        let _ = self.sandboxes.set_drained(&s.name, Some(g)).await;
                    }
                }
                None => {
                    sqlx::query("DELETE FROM engine.sandbox_holds WHERE project = $1")
                        .bind(&s.name)
                        .execute(&self.pool)
                        .await?;
                    if s.drained.is_some() && s.sandbox_ready {
                        let _ = self.sandboxes.set_drained(&s.name, None).await;
                    }
                }
            }
        }
        Ok(())
    }
}
