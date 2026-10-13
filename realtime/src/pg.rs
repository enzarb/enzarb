//! Postgres implementations of the listener traits.
//!
//! Required grants for the runtime role (`enzarb_realtime`): `CONNECT` on the database,
//! `USAGE` on schema `workflows_api` and `SELECT` on `workflows_api.work_item_project_v (id, project)` only.
//! `LISTEN` needs no grant.

use anyhow::Result;
use async_trait::async_trait;
use sqlx::{
    PgPool,
    postgres::{PgListener, PgPoolOptions},
};

use crate::listener::{Connector, Listener, ProjectLookup};

const PROBE_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
pub const CHANNEL: &str = "work_item_changed";

pub struct PgConnector {
    pub url: String,
}

struct PgStream(PgListener);

#[async_trait]
impl Listener for PgStream {
    async fn recv(&mut self) -> Result<i64> {
        // `try_recv` (not `recv`) so that a dropped connection surfaces instead of being silently
        // re-established: the supervisor must know in order to send `resync`.
        loop {
            // A half-open TCP connection never errors by itself: probe when idle.
            let got = match tokio::time::timeout(PROBE_AFTER, self.0.try_recv()).await {
                Ok(r) => r?,
                Err(_) => {
                    tokio::time::timeout(PROBE_AFTER, sqlx::query("SELECT 1").execute(&mut self.0))
                        .await
                        .map_err(|_| anyhow::anyhow!("probe timed out"))??;
                    continue;
                }
            };
            match got {
                Some(n) => match n.payload().parse() {
                    Ok(id) => return Ok(id),
                    Err(_) => tracing::warn!(payload = n.payload(), "non-numeric payload ignored"),
                },
                None => anyhow::bail!("connection closed"),
            }
        }
    }
}

#[async_trait]
impl Connector for PgConnector {
    async fn connect(&self) -> Result<Box<dyn Listener>> {
        let mut l = PgListener::connect(&self.url).await?;
        l.listen(CHANNEL).await?;
        Ok(Box::new(PgStream(l)))
    }
}

pub struct PgLookup {
    pool: PgPool,
}

impl PgLookup {
    /// A tiny lazily-connected pool, separate from the LISTEN connection.
    pub fn new(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect_lazy(url)?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl ProjectLookup for PgLookup {
    async fn project(&self, id: i64) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT project FROM workflows_api.work_item_project_v WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }
}
