//! Keeps `connectors.installations` in step with installation webhooks.

use std::collections::BTreeSet;

use enzarb_connector_github::InstallationChange;
use sqlx::{PgConnection, Row};

/// Applies one installation change. Repo keys are stored lowercase.
pub async fn apply(conn: &mut PgConnection, c: &InstallationChange) -> Result<(), sqlx::Error> {
    let id = c.installation_id as i64;

    if c.event == "installation" && c.action == "deleted" {
        sqlx::query("DELETE FROM installations WHERE installation_id = $1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        return Ok(());
    }

    let row = sqlx::query("SELECT repos FROM installations WHERE installation_id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let mut repos: BTreeSet<String> = match row {
        Some(r) => {
            let v: serde_json::Value = r.try_get("repos")?;
            serde_json::from_value(v).unwrap_or_default()
        }
        None => BTreeSet::new(),
    };
    if c.event == "installation" && c.action == "created" {
        // A fresh installation: the payload is the full repo list.
        repos.clear();
    }
    repos.extend(c.repositories_added.iter().map(|r| lower(r.to_string())));
    for r in &c.repositories_removed {
        repos.remove(&lower(r.to_string()));
    }

    sqlx::query(
        "INSERT INTO installations (installation_id, account, repos, updated_at) \
         VALUES ($1, $2, $3, now()) \
         ON CONFLICT (installation_id) DO UPDATE \
         SET account = EXCLUDED.account, repos = EXCLUDED.repos, updated_at = now()",
    )
    .bind(id)
    .bind(&c.account_login)
    .bind(serde_json::json!(repos))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn lower(s: String) -> String {
    s.to_lowercase()
}
