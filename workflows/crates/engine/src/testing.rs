//! Test fixture: a throwaway Postgres 18 container with migrations applied.
//! Enabled by the `testing` feature.

use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

pub struct TestDb {
    /// Superuser pool (owner role; migrations were applied through it).
    pub pool: PgPool,
    /// Superuser connection URL, for building role-specific URLs.
    pub url: String,
    _container: ContainerAsync<Postgres>,
}

impl TestDb {
    /// Start Postgres, create runtime roles (tests only; pgop does this in prod) and migrate.
    /// Returns `None` with a message on stderr when Docker is unavailable.
    pub async fn start() -> Option<TestDb> {
        let container = match Postgres::default().with_tag("18").start().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "SKIP: Docker/testcontainers unavailable ({e}); integration test skipped"
                );
                return None;
            }
        };
        let host = container.get_host().await.expect("container host");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("container port");
        let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
        let pool = crate::db::connect(&url, 5).await.expect("connect");
        for role in ["enzarb_connectors", "enzarb_ui", "enzarb_workflows"] {
            sqlx::query(&format!(
                "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') \
                 THEN CREATE ROLE {role} LOGIN PASSWORD '{role}'; END IF; END $$"
            ))
            .execute(&pool)
            .await
            .expect("create role");
        }
        crate::db::migrate(&pool).await.expect("migrate");
        Some(TestDb {
            pool,
            url,
            _container: container,
        })
    }

    /// Pool logged in as a runtime role (password equals the role name).
    pub async fn pool_as(&self, role: &str) -> PgPool {
        let rest = self.url.split_once('@').expect("url").1;
        crate::db::connect(&format!("postgres://{role}:{role}@{rest}"), 2)
            .await
            .expect("connect as role")
    }
}
