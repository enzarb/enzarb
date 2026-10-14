//! Work item dependencies against Postgres (testcontainers): storage, cycles, the status view,
//! and the sweep that continues dependents when a dependency completes.

use std::sync::Arc;

use engine::machine::Triggers;
use engine::testing::TestDb;
use engine::{ProjectInfo, Reducer, ReducerConfig, StaticPermissions, StaticProjectIndex, Ticker};
use enzarb_connector::Permission;
use serde_json::{Value, json};
use sqlx::PgPool;

macro_rules! db {
    () => {
        match TestDb::start().await {
            Some(db) => db,
            None => return,
        }
    };
}

const REPO: &str = "github:enzarb/enzarb-ui";

fn key(n: u64) -> String {
    format!("{REPO}#{n}")
}

async fn item(pool: &PgPool, n: u64, workflow: &str, state: &str, data: Value) {
    sqlx::query(
        "INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data)
         VALUES ($1, 'enzarb', $2, $3, $4)",
    )
    .bind(key(n))
    .bind(workflow)
    .bind(state)
    .bind(data)
    .execute(pool)
    .await
    .unwrap();
}

async fn set(pool: &PgPool, n: u64, source: &str, deps: &[u64], replace: bool) -> bool {
    let deps: Vec<String> = deps.iter().map(|d| key(*d)).collect();
    sqlx::query_scalar("SELECT workflows.set_dependencies($1, $2, $3, $4)")
        .bind(key(n))
        .bind(source)
        .bind(deps)
        .bind(replace)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn deps_of(pool: &PgPool, n: u64) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT depends_on_key, source FROM workflows.work_item_dependencies
         WHERE work_item_key = $1 ORDER BY depends_on_key, source",
    )
    .bind(key(n))
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn cycle(pool: &PgPool, n: u64) -> Option<Vec<String>> {
    sqlx::query_scalar("SELECT workflows.dependency_cycle($1)")
        .bind(key(n))
        .fetch_one(pool)
        .await
        .unwrap()
}

fn cr_data() -> Value {
    json!({
        "number": 2, "head_ref": "feat/x", "base_ref": "main", "head_sha": "sha1",
        "reviewed_sha": "sha1", "last_review": "approve", "ci": "success",
        "ci_sha": "sha1", "mergeable": true, "body": "Closes #1",
    })
}

#[tokio::test]
async fn sources_do_not_overwrite_each_other_and_self_references_are_dropped() {
    let db = db!();
    let p = &db.pool;
    assert!(set(p, 5, "github", &[1, 2], true).await);
    assert!(set(p, 5, "text", &[2, 3, 5], true).await);
    assert_eq!(deps_of(p, 5).await.len(), 4, "self reference dropped");
    // Replacing one source leaves the other alone.
    assert!(set(p, 5, "github", &[1], true).await);
    let got = deps_of(p, 5).await;
    assert!(got.contains(&(key(2), "text".into())));
    assert!(!got.contains(&(key(2), "github".into())));
    // Unchanged input changes nothing.
    assert!(!set(p, 5, "github", &[1], true).await);
    // Comments only add.
    assert!(set(p, 5, "comment", &[7], false).await);
    assert!(!set(p, 5, "comment", &[], false).await);
    assert_eq!(deps_of(p, 5).await.len(), 4);
}

#[tokio::test]
async fn cycles_are_found_for_every_member() {
    let db = db!();
    let p = &db.pool;
    set(p, 1, "github", &[2], true).await;
    set(p, 2, "github", &[3], true).await;
    assert_eq!(cycle(p, 1).await, None);
    set(p, 3, "github", &[1], true).await;
    assert_eq!(
        cycle(p, 1).await,
        Some(vec![key(1), key(2), key(3), key(1)])
    );
    for n in [2, 3] {
        assert!(cycle(p, n).await.is_some(), "#{n}");
    }
    // A node that only leads into the cycle is not on it.
    set(p, 4, "text", &[1], true).await;
    assert_eq!(cycle(p, 4).await, None);
    // Breaking the loop clears it.
    set(p, 3, "github", &[], true).await;
    assert_eq!(cycle(p, 1).await, None);
}

#[tokio::test]
async fn status_view_reports_done_closed_open_and_pending() {
    let db = db!();
    let p = &db.pool;
    item(p, 1, "issue", "Done", json!({"title": "one"})).await;
    item(p, 2, "issue", "Cancelled", json!({})).await;
    item(p, 3, "issue", "Implementing", json!({})).await;
    set(p, 9, "github", &[1, 2, 3, 4], true).await;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT depends_on_key, status FROM workflows_api.work_item_dependency_v
         WHERE work_item_key = $1 ORDER BY depends_on_key",
    )
    .bind(key(9))
    .fetch_all(p)
    .await
    .unwrap();
    let status = |n: u64| rows.iter().find(|r| r.0 == key(n)).unwrap().1.clone();
    assert_eq!(status(1), "done");
    assert_eq!(status(2), "closed");
    assert_eq!(status(3), "open");
    assert_eq!(status(4), "pending");
}

#[tokio::test]
async fn a_change_request_inherits_its_issues_dependencies() {
    let db = db!();
    let p = &db.pool;
    item(p, 1, "issue", "Implementing", json!({})).await;
    item(p, 2, "change_request", "AwaitingChanges", cr_data()).await;
    set(p, 1, "github", &[7], true).await;
    let inherited: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT depends_on_key, via FROM workflows.effective_dependencies WHERE work_item_key = $1",
    )
    .bind(key(2))
    .fetch_all(p)
    .await
    .unwrap();
    assert_eq!(inherited, [(key(7), Some(key(1)))]);
}

#[tokio::test]
async fn completing_a_dependency_continues_the_dependent() {
    let db = db!();
    let p = db.pool.clone();
    item(&p, 1, "issue", "Implementing", json!({})).await;
    item(&p, 2, "change_request", "ReadyToMerge", cr_data()).await;
    set(&p, 1, "github", &[3], true).await;
    item(&p, 3, "issue", "Implementing", json!({"title": "base"})).await;

    let index = StaticProjectIndex::shared();
    index.insert(
        REPO,
        ProjectInfo {
            name: "enzarb".into(),
            triggers: Triggers::default(),
            default_branch: "main".into(),
        },
    );
    let reducer = Reducer::new(
        p.clone(),
        index,
        Arc::new(StaticPermissions::new(Permission::Read)),
        ReducerConfig::default(),
    );
    let ticker = Ticker::new(p.clone());
    let state = || async {
        sqlx::query_as::<_, (String, Value)>(
            "SELECT state, state_data FROM workflows.work_items WHERE key = $1",
        )
        .bind(key(2))
        .fetch_one(&p)
        .await
        .unwrap()
    };

    // The dependency (inherited from the issue) is open: no longer ready.
    assert_eq!(ticker.sweep_dependencies().await.unwrap(), 1);
    reducer.drain().await;
    let (s, data) = state().await;
    assert_eq!(s, "AwaitingChanges");
    assert_eq!(data["open_dependencies"][0]["label"], "#3");
    let blocked: bool = sqlx::query_scalar(
        "SELECT blocked_by_dependency FROM workflows_api.work_item_v WHERE key = $1",
    )
    .bind(key(2))
    .fetch_one(&p)
    .await
    .unwrap();
    assert!(blocked);
    // Nothing changed: the sweep is quiet.
    assert_eq!(ticker.sweep_dependencies().await.unwrap(), 0);

    // The dependency completes: the dependent continues without a human.
    sqlx::query("UPDATE workflows.work_items SET state = 'Done' WHERE key = $1")
        .bind(key(3))
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(ticker.sweep_dependencies().await.unwrap(), 1);
    reducer.drain().await;
    let (s, data) = state().await;
    assert_eq!(s, "ReadyToMerge");
    assert_eq!(data["open_dependencies"], json!([]));
}
