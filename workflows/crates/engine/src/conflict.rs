//! Producer side of `MergeConflictDetected {base_sha}`.
//!
//! The machine cannot emit follow-up events, so the executor (readiness jobs) and the
//! ticker (base-head fan-out) both emit through here, via `workflows_api.ingest_event`.
//! The dedupe key `conflict:<work_item_key>:<base_sha>` gives one event per item per base
//! commit however often readiness is polled.

use serde_json::json;
use sqlx::PgPool;

use crate::error::Error;

/// Emit `MergeConflictDetected` for `item_key` against `base_sha`. Returns whether a new
/// event was created: false for an empty SHA (nothing is emitted) or a duplicate.
pub async fn emit_merge_conflict(
    pool: &PgPool,
    item_key: &str,
    base_sha: &str,
) -> Result<bool, Error> {
    if base_sha.is_empty() {
        return Ok(false);
    }
    let id: Option<i64> =
        sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
            .bind("engine")
            .bind(format!("conflict:{item_key}:{base_sha}"))
            .bind(item_key)
            .bind("MergeConflictDetected")
            .bind(json!({ "base_sha": base_sha }))
            .fetch_one(pool)
            .await?;
    Ok(id.is_some())
}
