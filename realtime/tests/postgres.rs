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

#[tokio::test]
async fn notify_to_frame() {
    let c = match Postgres::default().with_tag("18").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP: Docker unavailable ({e})");
            return;
        }
    };
    let url = format!(
        "postgres://postgres:postgres@{}:{}/postgres",
        c.get_host().await.unwrap(),
        c.get_host_port_ipv4(5432).await.unwrap()
    );
    let admin = PgPoolOptions::new().connect(&url).await.unwrap();
    admin
        .execute(
            "CREATE SCHEMA workflows_api;
             CREATE TABLE public.wi (id bigint PRIMARY KEY, project text NOT NULL);
             CREATE VIEW workflows_api.work_item_v AS SELECT id, project, 'secret' AS other FROM public.wi;
             CREATE VIEW workflows_api.work_item_project_v AS SELECT id, project FROM public.wi;
             INSERT INTO public.wi VALUES (7, 'enzarb');
             CREATE ROLE enzarb_realtime LOGIN PASSWORD 'rt';
             GRANT USAGE ON SCHEMA workflows_api TO enzarb_realtime;
             GRANT SELECT ON workflows_api.work_item_project_v TO enzarb_realtime;",
        )
        .await
        .unwrap();
    // The runtime role holds only the new grants (nothing on work_item_v).
    let rt_url = format!(
        "postgres://enzarb_realtime:rt@{}:{}/postgres",
        c.get_host().await.unwrap(),
        c.get_host_port_ipv4(5432).await.unwrap()
    );

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
        .execute("SELECT pg_notify('work_item_changed', '7')")
        .await
        .unwrap();
    let f = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        f,
        Frame::WorkItem {
            id: 7,
            project: "enzarb".into()
        }
    );
}
