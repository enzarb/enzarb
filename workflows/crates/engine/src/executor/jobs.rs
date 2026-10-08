//! Connector jobs: `set_progress`, `post_comment`, `open_change_request`.

use std::sync::Arc;

use enzarb_connector::{ChangeRequestSpec, Connector, Progress, ProgressTarget, WorkItemKey};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Claimed, Done, Executor, JobError};
use crate::machine::JobKind;

#[derive(Deserialize)]
struct SetProgress {
    target: ProgressTarget,
    progress: Progress,
}

#[derive(Deserialize)]
struct PostComment {
    item: WorkItemKey,
    body: String,
}

#[derive(Deserialize)]
struct OpenCr {
    spec: ChangeRequestSpec,
}

fn input<T: serde::de::DeserializeOwned>(job: &Claimed) -> Result<T, JobError> {
    serde_json::from_value(job.input.clone())
        .map_err(|e| JobError::permanent(format!("unreadable {} input: {e}", job.kind)))
}

impl Executor {
    pub(crate) fn connector_for(&self, name: &str) -> Result<&Arc<dyn Connector>, JobError> {
        self.connectors
            .get(name)
            .ok_or_else(|| JobError::permanent(format!("no connector named {name:?}")))
    }

    pub(crate) async fn run_connector_job(
        &self,
        job: &Claimed,
        kind: JobKind,
    ) -> Result<Done, JobError> {
        let key = job.idempotency_key.as_str();
        let output = match kind {
            JobKind::SetProgress => {
                let i: SetProgress = input(job)?;
                if i.progress == Progress::Working && self.agent_run_ended(job).await {
                    return Ok(Done::Output(json!({ "skipped": "the run already ended" })));
                }
                let c = self.connector_for(&i.target.item.repo.connector)?;
                c.set_progress(&i.target, i.progress, key).await?;
                json!({})
            }
            JobKind::PostComment => {
                let i: PostComment = input(job)?;
                let c = self.connector_for(&i.item.repo.connector)?;
                to_value(c.post_comment(&i.item, &i.body, key).await?)
            }
            JobKind::OpenChangeRequest => {
                let i: OpenCr = input(job)?;
                let c = self.connector_for(&i.spec.repo.connector)?;
                to_value(c.open_change_request(i.spec, key).await?)
            }
            other => {
                return Err(JobError::permanent(format!(
                    "{} is not a connector job",
                    other.as_str()
                )));
            }
        };
        Ok(Done::Output(output))
    }

    /// True when the agent job queued by the same event already succeeded or failed, so a
    /// `Working` marker would overwrite its `Done` / `Failed`.
    async fn agent_run_ended(&self, job: &Claimed) -> bool {
        sqlx::query_scalar(
            r"SELECT EXISTS (SELECT 1 FROM jobs a
                  WHERE a.work_item_id = $1 AND a.kind LIKE 'agent\_%'
                    AND a.status IN ('succeeded', 'failed')
                    AND starts_with(a.idempotency_key, regexp_replace($2, '[^:]*$', '')))",
        )
        .bind(job.work_item_id)
        .bind(&job.idempotency_key)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(false)
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}
