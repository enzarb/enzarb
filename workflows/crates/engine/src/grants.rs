//! Grants reconciler for `enzarb_realtime`.
//!
//! pgop may create the role after the migration that grants it ran, so the ticker re-checks on
//! every pass. The work is done by `workflows.reconcile_realtime_grants()` (SECURITY DEFINER),
//! because the runtime role cannot grant on objects it does not own. The function grants exactly
//! `CONNECT` on the database, `USAGE` on `workflows_api` and `SELECT` on
//! `workflows_api.work_item_project_v`, and only what is missing.

use sqlx::PgPool;

use crate::error::Error;

/// One reconcile pass. `Ok(true)` if grants were made, `Ok(false)` if the role is absent or
/// already has everything.
pub async fn reconcile_realtime(pool: &PgPool) -> Result<bool, Error> {
    let granted: bool = sqlx::query_scalar("SELECT workflows.reconcile_realtime_grants()")
        .fetch_one(pool)
        .await?;
    Ok(granted)
}

/// True if `e` is "function does not exist" (SQLSTATE 42883): `migrate` has not yet run a
/// version that includes the reconciler.
pub fn is_missing_function(e: &Error) -> bool {
    matches!(e, Error::Db(sqlx::Error::Database(d)) if d.code().as_deref() == Some("42883"))
}
