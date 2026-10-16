//! LISTEN -> lookup -> frame against a real Postgres. Skipped when Docker is unavailable.

use std::{sync::Arc, time::Duration};

use enzarb_realtime::{
    hub::{Frame, Hub},
    listener::{self, Backoff},
    pg::{PgConnector, PgLookup},
};
use sqlx::{Executor, postgres::PgPoolOptions};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{ImageExt, runners::AsyncRunner},
};

/// Env var naming one shared Postgres server for the whole run (docs/dev.md). When set, the test
/// uses its own database on it instead of its own container.
const PG_URL_ENV: &str = "ENZARB_TEST_PG_URL";

/// Drops the per-test database on the shared server (best effort) when the test ends.
struct DropDb {
    admin: String,
    db: String,
}

impl Drop for DropDb {
    fn drop(&mut self) {
        let (admin, db) = (self.admin.clone(), self.db.clone());
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
                    let _ = pool
                        .execute(format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)").as_str())
                        .await;
                }
            });
        })
        .join();
    }
}

/// `(superuser url, role url, guards)`: a throwaway container, or a fresh database on the shared
/// server. `None` when neither is available.
async fn target() -> Option<(String, String, Box<dyn std::any::Any>)> {
    if let Some(admin) = std::env::var(PG_URL_ENV).ok().filter(|u| !u.is_empty()) {
        let (base, _) = admin.rsplit_once('/').unwrap();
        let (scheme, rest) = base.split_once("://").unwrap();
        let host = rest.rsplit_once('@').unwrap().1;
        let db = format!(
            "rt_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let pool = PgPoolOptions::new().connect(&admin).await.unwrap();
        pool.execute(format!("CREATE DATABASE {db}").as_str())
            .await
            .unwrap();
        pool.close().await;
        let url = format!("{scheme}://postgres:postgres@{host}/{db}");
        let rt_url = format!("{scheme}://enzarb_realtime:rt@{host}/{db}");
        return Some((url, rt_url, Box::new(DropDb { admin, db })));
    }
    let c = match Postgres::default().with_tag("18").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP: Docker unavailable ({e})");
            return None;
        }
    };
    let hp = format!(
        "{}:{}",
        c.get_host().await.unwrap(),
        c.get_host_port_ipv4(5432).await.unwrap()
    );
    Some((
        format!("postgres://postgres:postgres@{hp}/postgres"),
        format!("postgres://enzarb_realtime:rt@{hp}/postgres"),
        Box::new(c),
    ))
}

#[tokio::test]
async fn notify_to_frame() {
    let Some((url, rt_url, _guard)) = target().await else {
        return;
    };
    let admin = PgPoolOptions::new().connect(&url).await.unwrap();
    // Roles are cluster-global: create-or-update so a shared server can hold it already.
    admin
        .execute(
            "DO $$ BEGIN
               IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'enzarb_realtime') THEN
                 CREATE ROLE enzarb_realtime LOGIN PASSWORD 'rt';
               ELSE
                 ALTER ROLE enzarb_realtime LOGIN PASSWORD 'rt';
               END IF;
             END $$;
             CREATE SCHEMA engine_api;
             CREATE TABLE public.wi (id bigint PRIMARY KEY, project text NOT NULL);
             CREATE VIEW engine_api.secret_v AS SELECT id, project, 'secret' AS other FROM public.wi;
             CREATE VIEW engine_api.task_v AS SELECT id AS task_id, project FROM public.wi;
             INSERT INTO public.wi VALUES (7, 'enzarb');
             GRANT USAGE ON SCHEMA engine_api TO enzarb_realtime;
             GRANT SELECT ON engine_api.task_v TO enzarb_realtime;",
        )
        .await
        .unwrap();
    // The runtime role holds only the new grants (nothing on secret_v).

    let hub = Arc::new(Hub::new(16));
    let mut rx = hub.subscribe();
    tokio::spawn(listener::run(
        Arc::new(PgConnector { url: url.clone() }),
        Arc::new(PgLookup::new(&rt_url).unwrap()),
        hub.clone(),
        Backoff::default(),
    ));
    for _ in 0..100 {
        if hub.is_ready() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(hub.is_ready());
    admin
        .execute("SELECT pg_notify('engine_projection', '7')")
        .await
        .unwrap();
    let f = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        f,
        Frame::Task {
            id: 7,
            project: "enzarb".into()
        }
    );
}
