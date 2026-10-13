//! Draining a Project's sandbox for a pod-shape change.
//!
//! While `Project.status.pendingChange` is set the operator waits for the sandbox to drain.
//! Each sweep, [`Executor::reconcile_sandbox_changes`]:
//! 1. records a hold row in `workflows.project_sandbox_changes` for every pending Project
//!    (the claim then starts no new agent job for it; running jobs are never cancelled);
//! 2. when no agent job of the Project is `running`, sets the Project annotation
//!    `enzarb.dev/drained-generation` to the pending generation (idempotent, re-asserted when
//!    the generation moves);
//! 3. drops the hold once `pendingChange` is gone and the sandbox is Ready again (or the Project
//!    is gone), and clears a leftover annotation so the next episode starts clean;
//! 4. stops enforcing a hold older than [`ExecutorConfig::sandbox_change_hold_ceiling`] and
//!    warns once, in case the operator never clears it.
//!
//! The hold insert and the running count happen under the claim's advisory lock, so a job
//! claimed just before the hold appears is counted and a later claim sees the hold.

use std::collections::HashSet;

use enzarb_crds::{ANNOTATION_DRAINED_GENERATION, COND_SANDBOX_READY, Project};

use super::Executor;
use crate::error::Error;

/// What the reconciler needs to know about one Project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectChangeState {
    pub name: String,
    /// `status.pendingChange.generation`.
    pub pending_generation: Option<i64>,
    /// Condition `SandboxReady` is `True`.
    pub sandbox_ready: bool,
    /// Current `enzarb.dev/drained-generation` annotation (raw).
    pub drained: Option<String>,
}

/// Pure projection of a Project, for tests.
pub fn change_state(p: &Project) -> ProjectChangeState {
    let status = p.status.as_ref();
    ProjectChangeState {
        name: p.metadata.name.clone().unwrap_or_default(),
        pending_generation: status
            .and_then(|s| s.pending_change.as_ref())
            .map(|c| c.generation),
        sandbox_ready: status.is_some_and(|s| {
            s.conditions
                .iter()
                .any(|c| c.type_ == COND_SANDBOX_READY && c.status == "True")
        }),
        drained: p
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get(ANNOTATION_DRAINED_GENERATION))
            .cloned(),
    }
}

impl Executor {
    /// One reconcile pass. Errors listing Projects leave holds as they are.
    pub async fn reconcile_sandbox_changes(&self) -> Result<(), Error> {
        let states = match self.sandboxes.change_states().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "sandbox change: listing Projects failed");
                return Ok(());
            }
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(7426001)")
            .execute(&mut *tx)
            .await?;
        let held: Vec<String> =
            sqlx::query_scalar("SELECT project_id FROM workflows.project_sandbox_changes")
                .fetch_all(&mut *tx)
                .await?;
        let mut to_ack: Vec<(String, i64)> = Vec::new();
        let mut to_clear: Vec<String> = Vec::new();
        for s in &states {
            if let Some(generation) = s.pending_generation {
                sqlx::query(
                    "INSERT INTO workflows.project_sandbox_changes (project_id, generation)
                     VALUES ($1, $2)
                     ON CONFLICT (project_id) DO UPDATE
                       SET generation = EXCLUDED.generation, last_seen_at = now()",
                )
                .bind(&s.name)
                .bind(generation)
                .execute(&mut *tx)
                .await?;
                let running: i64 = sqlx::query_scalar(
                    r"SELECT count(*) FROM jobs
                      WHERE status = 'running' AND project_id = $1 AND kind LIKE 'agent\_%'",
                )
                .bind(&s.name)
                .fetch_one(&mut *tx)
                .await?;
                if running == 0 && s.drained.as_deref() != Some(&generation.to_string()) {
                    to_ack.push((s.name.clone(), generation));
                }
            } else if s.sandbox_ready {
                // Episode over. Drop the hold and a stale ack: generations restart per episode.
                sqlx::query("DELETE FROM workflows.project_sandbox_changes WHERE project_id = $1")
                    .bind(&s.name)
                    .execute(&mut *tx)
                    .await?;
                if s.drained.is_some() {
                    to_clear.push(s.name.clone());
                }
            }
        }
        // Projects that no longer exist.
        let known: HashSet<&str> = states.iter().map(|s| s.name.as_str()).collect();
        for p in held.iter().filter(|p| !known.contains(p.as_str())) {
            sqlx::query("DELETE FROM workflows.project_sandbox_changes WHERE project_id = $1")
                .bind(p)
                .execute(&mut *tx)
                .await?;
        }
        // Ceiling warnings, once per episode.
        let expired: Vec<String> = sqlx::query_scalar(
            "SELECT project_id FROM workflows.project_sandbox_changes
             WHERE first_seen_at <= now() - make_interval(secs => $1)",
        )
        .bind(self.cfg.sandbox_change_hold_ceiling.as_secs_f64())
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        {
            let mut warned = self.change_warned.lock().expect("warned lock");
            warned.retain(|p| expired.contains(p));
            for p in expired {
                if warned.insert(p.clone()) {
                    tracing::warn!(project = %p, ceiling = ?self.cfg.sandbox_change_hold_ceiling,
                        "sandbox change still pending past the hold ceiling; agent jobs are no longer held");
                }
            }
        }

        for (project, generation) in to_ack {
            match self.sandboxes.set_drained(&project, Some(generation)).await {
                Ok(()) => {
                    tracing::info!(%project, generation, "sandbox drained; acknowledged to the operator")
                }
                Err(e) => {
                    tracing::warn!(%project, error = %e, "sandbox change: could not set drained annotation; retrying next sweep")
                }
            }
        }
        for project in to_clear {
            if let Err(e) = self.sandboxes.set_drained(&project, None).await {
                tracing::warn!(%project, error = %e, "sandbox change: could not clear drained annotation");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn change_state_reads_pending_ready_and_annotation() {
        let p: Project = serde_json::from_value(serde_json::json!({
            "apiVersion": "enzarb.dev/v1alpha1", "kind": "Project",
            "metadata": {"name": "p", "annotations": {"enzarb.dev/drained-generation": "3"}},
            "spec": {"owner": "o"},
            "status": {
                "conditions": [{"type": "SandboxReady", "status": "True",
                    "lastTransitionTime": "2026-01-01T00:00:00Z", "reason": "r", "message": ""}],
                "pendingChange": {"generation": 3, "since": "2026-01-01T00:00:00Z", "reason": "x"}
            }
        }))
        .unwrap();
        let s = change_state(&p);
        assert_eq!(
            (s.pending_generation, s.sandbox_ready, s.drained.as_deref()),
            (Some(3), true, Some("3"))
        );
    }
}
