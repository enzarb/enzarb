//! Test fixture: a Postgres 18 database with migrations applied.
//! Enabled by the `testing` feature.
//!
//! Two modes:
//! - `ENZARB_TEST_PG_URL` set: use that server (one per run). Runtime roles and a migrated
//!   template database `enzarb_template` are created once per server (and rebuilt when the
//!   migrations change); each test clones it into `test_<unique>`, dropped on `Drop`.
//! - unset: a throwaway `postgres:18` container per test (needs Docker).

use sha2::{Digest, Sha256};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use std::sync::atomic::{AtomicU64, Ordering};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};

/// Env var naming a shared Postgres server (admin URL, e.g.
/// `postgres://postgres:postgres@localhost:5432/postgres`).
pub const PG_URL_ENV: &str = "ENZARB_TEST_PG_URL";
/// Name of the migrated template database on the shared server.
pub const TEMPLATE_DB: &str = "enzarb_template";
/// Session advisory lock serialising template setup and cloning across processes.
const LOCK_KEY: i64 = 0x656e_7a61_7262;
const ROLES: [&str; 3] = ["enzarb_connectors", "enzarb_ui", "enzarb_workflows"];

pub struct TestDb {
    /// Superuser pool (owner role; migrations were applied through it).
    pub pool: PgPool,
    /// Superuser connection URL (to this test's database), for building role-specific URLs.
    pub url: String,
    _container: Option<ContainerAsync<Postgres>>,
    /// Shared mode: (admin URL, database name) to drop on `Drop`.
    cleanup: Option<(String, String)>,
}

/// Hex SHA-256 over the embedded migrations (version, description and checksum of each).
pub fn migrations_checksum() -> String {
    let mut h = Sha256::new();
    for m in crate::db::MIGRATOR.iter() {
        h.update(m.version.to_be_bytes());
        h.update(m.description.as_bytes());
        h.update(&*m.checksum);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Replace the database name in a `postgres://…/db[?query]` URL.
fn with_database(url: &str, db: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (url, String::new()),
    };
    let (prefix, _) = base.rsplit_once('/').expect("database url has a path");
    format!("{prefix}/{db}{query}")
}

fn unique_name(prefix: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{prefix}_{}_{nanos:x}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

fn shared_url() -> Option<String> {
    std::env::var(PG_URL_ENV).ok().filter(|u| !u.is_empty())
}

/// Start a container and return it with its superuser URL, or `None` when Docker is unavailable.
async fn start_container() -> Option<(ContainerAsync<Postgres>, String)> {
    let container = match Postgres::default().with_tag("18").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP: Docker/testcontainers unavailable ({e}); integration test skipped");
            return None;
        }
    };
    let host = container.get_host().await.expect("container host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("container port");
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    Some((container, url))
}

/// Idempotent, but only race-free when nobody else creates roles concurrently (a private
/// container). The shared-server path does the same under its advisory lock.
async fn create_roles(pool: &PgPool) {
    for role in ROLES {
        sqlx::query(&create_role_sql(role))
            .execute(pool)
            .await
            .expect("create role");
    }
}

fn create_role_sql(role: &str) -> String {
    format!(
        "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{role}') \
         THEN CREATE ROLE {role} LOGIN PASSWORD '{role}'; END IF; END $$"
    )
}

impl TestDb {
    /// Create runtime roles (tests only; pgop does this in prod) and a migrated database.
    /// Returns `None` with a message on stderr when no server is configured and Docker is
    /// unavailable.
    pub async fn start() -> Option<TestDb> {
        match shared_url() {
            Some(admin) => Some(Self::start_shared(&admin).await),
            None => Self::start_container().await,
        }
    }

    /// An unmigrated database. With `ENZARB_TEST_PG_URL` set, an empty database on the shared
    /// server, where the cluster-global runtime roles may already exist; use
    /// [`TestDb::start_bare_container`] when a test needs them absent.
    pub async fn start_bare() -> Option<TestDb> {
        match shared_url() {
            Some(admin) => {
                let name = unique_name("bare");
                let pool = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&admin)
                    .await
                    .expect("connect to shared postgres");
                sqlx::query(&format!("CREATE DATABASE {name}"))
                    .execute(&pool)
                    .await
                    .expect("create db");
                pool.close().await;
                Some(Self::attach(&admin, name, 2).await)
            }
            None => Self::start_bare_container().await,
        }
    }

    /// Like [`TestDb::start_bare`] but always a dedicated container, so no roles exist.
    pub async fn start_bare_container() -> Option<TestDb> {
        let (container, url) = start_container().await?;
        let pool = crate::db::connect(&url, 2).await.expect("connect");
        Some(TestDb {
            pool,
            url,
            _container: Some(container),
            cleanup: None,
        })
    }

    async fn start_container() -> Option<TestDb> {
        let (container, url) = start_container().await?;
        let pool = crate::db::connect(&url, 5).await.expect("connect");
        create_roles(&pool).await;
        crate::db::migrate(&pool).await.expect("migrate");
        Some(TestDb {
            pool,
            url,
            _container: Some(container),
            cleanup: None,
        })
    }

    async fn attach(admin_url: &str, name: String, max_connections: u32) -> TestDb {
        let url = with_database(admin_url, &name);
        let pool = crate::db::connect(&url, max_connections)
            .await
            .expect("connect");
        TestDb {
            pool,
            url,
            _container: None,
            cleanup: Some((admin_url.to_string(), name)),
        }
    }

    async fn start_shared(admin_url: &str) -> TestDb {
        let name = unique_name("test");
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await
            .expect("connect to shared postgres");
        // The advisory lock is session-scoped: hold this one connection until the clone exists.
        let mut conn = admin.acquire().await.expect("admin connection");
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *conn)
            .await
            .expect("advisory lock");

        // Roles are cluster-global, so creating them is only race-free under the lock.
        for role in ROLES {
            sqlx::query(&create_role_sql(role))
                .execute(&mut *conn)
                .await
                .expect("create role");
        }

        let want = migrations_checksum();
        let have: Option<String> = sqlx::query_scalar(
            "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1",
        )
        .bind(TEMPLATE_DB)
        .fetch_optional(&mut *conn)
        .await
        .expect("read template checksum")
        .flatten();
        if have.as_deref() != Some(&want) {
            sqlx::query(&format!(
                "DROP DATABASE IF EXISTS {TEMPLATE_DB} WITH (FORCE)"
            ))
            .execute(&mut *conn)
            .await
            .expect("drop stale template");
            sqlx::query(&format!("CREATE DATABASE {TEMPLATE_DB}"))
                .execute(&mut *conn)
                .await
                .expect("create template");
            let tpl = crate::db::connect(&with_database(admin_url, TEMPLATE_DB), 1)
                .await
                .expect("connect to template");
            crate::db::migrate(&tpl).await.expect("migrate template");
            // A template with open sessions cannot be cloned.
            tpl.close().await;
            sqlx::query(&format!("COMMENT ON DATABASE {TEMPLATE_DB} IS '{want}'"))
                .execute(&mut *conn)
                .await
                .expect("write template checksum");
        }

        sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE {TEMPLATE_DB}"))
            .execute(&mut *conn)
            .await
            .expect("clone template");
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(LOCK_KEY)
            .execute(&mut *conn)
            .await
            .expect("advisory unlock");
        drop(conn);
        admin.close().await;
        Self::attach(admin_url, name, 5).await
    }

    /// Pool logged in as a runtime role (password equals the role name).
    pub async fn pool_as(&self, role: &str) -> PgPool {
        let rest = self.url.split_once('@').expect("url").1;
        crate::db::connect(&format!("postgres://{role}:{role}@{rest}"), 2)
            .await
            .expect("connect as role")
    }

    /// Install a copy of the UI-owned `app.claude_credentials` table and
    /// `app_api.claude_credential_get` (enzarb-ui `migrations/0004_claude_credentials.sql`),
    /// executable by `enzarb_workflows`. Idempotent.
    pub async fn install_claude_credentials(&self) {
        for stmt in [
            "CREATE SCHEMA IF NOT EXISTS app",
            "CREATE SCHEMA IF NOT EXISTS app_api",
            "CREATE TABLE IF NOT EXISTS app.claude_credentials (
                owner text PRIMARY KEY, key_id text NOT NULL, ciphertext bytea NOT NULL,
                created_at timestamptz NOT NULL DEFAULT now(),
                updated_at timestamptz NOT NULL DEFAULT now())",
            "CREATE OR REPLACE FUNCTION app_api.claude_credential_get(owner text)
             RETURNS TABLE (key_id text, ciphertext bytea)
             LANGUAGE sql STABLE SECURITY DEFINER SET search_path = pg_catalog, app
             AS $$ SELECT t.key_id, t.ciphertext FROM app.claude_credentials t WHERE t.owner = $1 $$",
            "REVOKE ALL ON FUNCTION app_api.claude_credential_get(text) FROM PUBLIC",
            "GRANT USAGE ON SCHEMA app_api TO enzarb_workflows",
            "GRANT EXECUTE ON FUNCTION app_api.claude_credential_get(text) TO enzarb_workflows",
        ] {
            sqlx::query(stmt)
                .execute(&self.pool)
                .await
                .expect("claude credentials fixture");
        }
    }

    /// Upsert one stored (encrypted) credential row.
    pub async fn put_claude_credential(&self, owner: &str, key_id: &str, ciphertext: &[u8]) {
        sqlx::query(
            "INSERT INTO app.claude_credentials (owner, key_id, ciphertext) VALUES ($1, $2, $3)
             ON CONFLICT (owner) DO UPDATE SET key_id = $2, ciphertext = $3",
        )
        .bind(owner)
        .bind(key_id)
        .bind(ciphertext)
        .execute(&self.pool)
        .await
        .expect("put claude credential");
    }
}

impl Drop for TestDb {
    /// Shared mode: best-effort `DROP DATABASE` on a short-lived thread with its own runtime
    /// (works inside `#[tokio::test]`). Errors are ignored; leaked databases are acceptable.
    fn drop(&mut self) {
        let Some((admin, name)) = self.cleanup.take() else {
            return;
        };
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async move {
                if let Ok(pool) = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&admin)
                    .await
                {
                    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                        .execute(&pool)
                        .await;
                    pool.close().await;
                }
            });
        })
        .join();
    }
}
