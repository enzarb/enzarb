//! The `migrate` subcommand.

use sqlx::PgPool;

/// Runtime roles pgop creates; the migrations grant to them.
pub const RUNTIME_ROLES: [&str; 3] = ["enzarb_connectors", "enzarb_ui", "enzarb_workflows"];

#[derive(Debug, thiserror::Error)]
pub enum MigrateError {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(
        "required database roles are missing: {0} (pgop creates them; refusing to migrate \
         without --require-roles being satisfied)"
    )]
    MissingRoles(String),
}

/// Roles from [`RUNTIME_ROLES`] that do not exist.
pub async fn missing_roles(pool: &PgPool) -> Result<Vec<&'static str>, sqlx::Error> {
    let mut missing = Vec::new();
    for role in RUNTIME_ROLES {
        let found: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM pg_roles WHERE rolname = $1")
            .bind(role)
            .fetch_optional(pool)
            .await?;
        if found.is_none() {
            missing.push(role);
        }
    }
    Ok(missing)
}

/// Apply migrations. With `require_roles`, nothing is applied unless all runtime roles exist.
pub async fn migrate(pool: &PgPool, require_roles: bool) -> Result<(), MigrateError> {
    if require_roles {
        let missing = missing_roles(pool).await?;
        if !missing.is_empty() {
            return Err(MigrateError::MissingRoles(missing.join(", ")));
        }
    }
    engine::db::migrate(pool).await?;
    Ok(())
}
