//! Stacked change requests against Postgres (testcontainers): choosing the parent through its
//! issue, and the parent's merge retargeting its child exactly once.

use std::sync::Arc;

use engine::machine::Triggers;
use engine::testing::TestDb;
use engine::{ProjectInfo, Reducer, ReducerConfig, StaticPermissions, StaticProjectIndex};
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

fn parent_cr_data() -> Value {
    json!({
        "number": 5, "head_ref": "enzarb/issue-1", "base_ref": "main", "head_sha": "ptip",
        "author": "enzarb[bot]",
    })
}

fn child_cr_data() -> Value {
    json!({
        "number": 6, "head_ref": "enzarb/issue-2", "base_ref": "enzarb/issue-1",
        "head_sha": "sha1", "reviewed_sha": "sha1", "last_review": "approve", "ci": "success",
        "ci_sha": "sha1", "mergeable": true, "author": "enzarb[bot]",
        "stacked_on": key(5), "stack_parent_ref": "enzarb/issue-1", "stack_parent_tip": "ptip",
        "default_base": "main",
    })
}

async fn set_head_ref(pool: &PgPool, n: u64, head_ref: &str) {
    sqlx::query(
        "UPDATE workflows.work_items SET state_data = jsonb_set(state_data, '{head_ref}', to_jsonb($2::text))
         WHERE key = $1",
    )
    .bind(key(n))
    .bind(head_ref)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn the_stack_parent_is_found_through_the_parent_issue() {
    let db = db!();
    let p = db.pool.clone();
    // Parent issue #1 is Done (its change request #5 is open); the child issue #2 depends on it.
    item(&p, 1, "issue", "Done", json!({"change_request": 5})).await;
    item(&p, 5, "change_request", "AwaitingChanges", parent_cr_data()).await;
    item(&p, 2, "issue", "AwaitingApproval", json!({})).await;
    sqlx::query("SELECT workflows.set_dependencies($1, 'text', $2, true)")
        .bind(key(2))
        .bind(vec![key(1)])
        .fetch_one(&p)
        .await
        .unwrap();

    let k2 = key(2).parse().unwrap();
    let mut conn = p.acquire().await.unwrap();
    let sp = engine::dependencies::stack_parent(&mut conn, &k2, None)
        .await
        .unwrap()
        .expect("a stack parent");
    assert_eq!(sp.key, key(5));
    assert_eq!(sp.head_ref, "enzarb/issue-1");
    assert_eq!(sp.head_sha, "ptip");
    assert!(!sp.merged);

    // A parent branch that is not enzarb's own is not stacked on.
    set_head_ref(&p, 5, "feat/x").await;
    assert!(
        engine::dependencies::stack_parent(&mut conn, &k2, None)
            .await
            .unwrap()
            .is_none()
    );
    set_head_ref(&p, 5, "enzarb/issue-1").await;

    // Merged: nothing to stack on any more.
    sqlx::query("UPDATE workflows.work_items SET state = 'Merged' WHERE key = $1")
        .bind(key(5))
        .execute(&p)
        .await
        .unwrap();
    assert!(
        engine::dependencies::stack_parent(&mut conn, &k2, None)
            .await
            .unwrap()
            .is_none()
    );
    // An item already stacked reads its recorded parent, now merged.
    let sp = engine::dependencies::stack_parent(&mut conn, &k2, Some(&key(5)))
        .await
        .unwrap()
        .unwrap();
    assert!(sp.merged && !sp.closed);
}

#[tokio::test]
async fn a_merged_parent_retargets_its_stacked_child_once() {
    let db = db!();
    let p = db.pool.clone();
    item(&p, 5, "change_request", "AwaitingChanges", parent_cr_data()).await;
    item(&p, 6, "change_request", "AwaitingChanges", child_cr_data()).await;

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
    let retargets = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflows.jobs WHERE kind = 'retarget_change_request'",
        )
        .fetch_one(&p)
        .await
        .unwrap()
    };

    // The parent is still open: nothing to do for the child.
    let parent = key(5).parse().unwrap();
    let mut conn = p.acquire().await.unwrap();
    engine::dependencies::notify_stacked_children(&mut conn, &parent, "AwaitingChanges")
        .await
        .unwrap();
    drop(conn);
    reducer.drain().await;
    assert_eq!(retargets().await, 0);

    // The parent merges: the child is told at once and one retarget job is queued.
    sqlx::query("UPDATE workflows.work_items SET state = 'Merged' WHERE key = $1")
        .bind(key(5))
        .execute(&p)
        .await
        .unwrap();
    let mut conn = p.acquire().await.unwrap();
    // Delivered twice: the dedupe key keeps it to one event per parent and child.
    for _ in 0..2 {
        engine::dependencies::notify_stacked_children(&mut conn, &parent, "Merged")
            .await
            .unwrap();
    }
    drop(conn);
    reducer.drain().await;
    assert_eq!(retargets().await, 1);
    let (input, data): (Value, Value) = sqlx::query_as(
        "SELECT j.input, w.state_data FROM workflows.jobs j JOIN workflows.work_items w
           ON w.id = j.work_item_id WHERE j.kind = 'retarget_change_request'",
    )
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!(input["head_ref"], "enzarb/issue-2");
    assert_eq!(input["old_base"], "ptip");
    assert_eq!(input["expected_sha"], "sha1");
    assert_eq!(input["onto"], "main");
    assert_eq!(data["retargeting"], true);
    assert_eq!(data["stack_parent_merged"], true);

    // Nothing merged: the child is still waiting.
    let state: String = sqlx::query_scalar("SELECT state FROM workflows.work_items WHERE key = $1")
        .bind(key(6))
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(state, "AwaitingChanges");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM workflows.jobs WHERE kind = 'merge_change_request'"
        )
        .fetch_one(&p)
        .await
        .unwrap(),
        0
    );
}
