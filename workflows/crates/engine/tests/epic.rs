//! Epics against Postgres (testcontainers): the sweep that finishes an epic when its
//! sub-issues are done, and sub-issues that the bot opens for a stored epic.

use std::sync::Arc;

use chrono::Utc;
use engine::machine::Triggers;
use engine::testing::TestDb;
use engine::{ProjectInfo, Reducer, ReducerConfig, StaticPermissions, StaticProjectIndex, Ticker};
use enzarb_connector::{
    Actor, Event, EventData, EventType, ItemKind, ItemRef, Permission, WorkItemKey,
};
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

async fn item(pool: &PgPool, n: u64, state: &str, data: Value) {
    sqlx::query(
        "INSERT INTO workflows.work_items (key, project_id, workflow, state, state_data)
         VALUES ($1, 'enzarb', 'issue', $2, $3)",
    )
    .bind(key(n))
    .bind(state)
    .bind(data)
    .execute(pool)
    .await
    .unwrap();
}

async fn state_of(pool: &PgPool, n: u64) -> Option<(String, Value)> {
    sqlx::query_as("SELECT state, state_data FROM workflows.work_items WHERE key = $1")
        .bind(key(n))
        .fetch_optional(pool)
        .await
        .unwrap()
}

fn reducer(pool: &PgPool) -> Reducer {
    let index = StaticProjectIndex::shared();
    index.insert(
        REPO,
        ProjectInfo {
            name: "enzarb".into(),
            triggers: Triggers::default(),
            default_branch: "main".into(),
        },
    );
    Reducer::new(
        pool.clone(),
        index,
        Arc::new(StaticPermissions::new(Permission::Read)),
        ReducerConfig::default(),
    )
}

fn wk(n: u64) -> WorkItemKey {
    key(n).parse().unwrap()
}

#[tokio::test]
async fn an_epic_is_done_when_every_sub_issue_is() {
    let db = db!();
    let p = db.pool.clone();
    item(
        &p,
        1,
        "AwaitingChildren",
        json!({
            "number": 1, "title": "Epic", "author": "alice", "epic": true,
            "children": [
                { "key": "api", "item": key(2), "title": "API" },
                { "key": "ui", "item": key(3), "title": "UI" },
            ],
        }),
    )
    .await;
    item(&p, 2, "Implementing", json!({})).await;
    item(&p, 3, "Triaging", json!({})).await;
    let changed =
        engine::dependencies::record_epic(&p, &wk(1), &[(wk(2), vec![]), (wk(3), vec![wk(2)])])
            .await
            .unwrap();
    assert!(changed);
    assert!(
        !engine::dependencies::record_epic(&p, &wk(1), &[(wk(2), vec![]), (wk(3), vec![wk(2)])])
            .await
            .unwrap(),
        "recording twice changes nothing"
    );

    let reducer = reducer(&p);
    let ticker = Ticker::new(p.clone());

    // The sweep notices the epic has open sub-issues and stores them.
    assert_eq!(ticker.sweep_dependencies().await.unwrap(), 1);
    reducer.drain().await;
    let (s, data) = state_of(&p, 1).await.unwrap();
    assert_eq!(s, "AwaitingChildren");
    assert_eq!(data["open_dependencies"].as_array().unwrap().len(), 2);
    assert_eq!(
        ticker.sweep_dependencies().await.unwrap(),
        0,
        "quiet when nothing changed"
    );

    // One sub-issue done: still waiting.
    sqlx::query("UPDATE workflows.work_items SET state = 'Done' WHERE key = $1")
        .bind(key(2))
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(ticker.sweep_dependencies().await.unwrap(), 1);
    reducer.drain().await;
    let (s, data) = state_of(&p, 1).await.unwrap();
    assert_eq!(s, "AwaitingChildren");
    assert_eq!(data["open_dependencies"][0]["label"], "#3");

    // The last one done: the epic closes as Done and says so.
    sqlx::query("UPDATE workflows.work_items SET state = 'Done' WHERE key = $1")
        .bind(key(3))
        .execute(&p)
        .await
        .unwrap();
    assert_eq!(ticker.sweep_dependencies().await.unwrap(), 1);
    reducer.drain().await;
    let (s, _) = state_of(&p, 1).await.unwrap();
    assert_eq!(s, "Done");
    let comments: Vec<String> = sqlx::query_scalar(
        "SELECT j.input->>'body' FROM workflows.jobs j
         JOIN workflows.work_items w ON w.id = j.work_item_id
         WHERE w.key = $1 AND j.kind = 'post_comment'",
    )
    .bind(key(1))
    .fetch_all(&p)
    .await
    .unwrap();
    assert!(
        comments
            .iter()
            .any(|c| c.contains("All 2 sub-issues are done")),
        "{comments:?}"
    );
    // The epic's issue is closed as completed (ordinary issues close via `Closes #n`).
    let closes: Vec<Value> = sqlx::query_scalar(
        "SELECT j.input FROM workflows.jobs j
         JOIN workflows.work_items w ON w.id = j.work_item_id
         WHERE w.key = $1 AND j.kind = 'close_issue'",
    )
    .bind(key(1))
    .fetch_all(&p)
    .await
    .unwrap();
    assert_eq!(closes.len(), 1, "{closes:?}");
    assert_eq!(closes[0]["reason"], "completed");
    assert_eq!(closes[0]["item"], json!(wk(1)));
    for n in [2, 3] {
        let other: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM workflows.jobs j
             JOIN workflows.work_items w ON w.id = j.work_item_id
             WHERE w.key = $1 AND j.kind = 'close_issue'",
        )
        .bind(key(n))
        .fetch_one(&p)
        .await
        .unwrap();
        assert_eq!(other, 0);
    }
    // No change request was ever handed off.
    let handoffs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflows.events WHERE dedupe_key LIKE 'handoff:%'",
    )
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!(handoffs, 0);
}

async fn bot_opens(pool: &PgPool, delivery: &str, number: u64, body: &str, login: &str) {
    let data = EventData {
        repo: REPO.parse().unwrap(),
        actor: Actor {
            login: login.into(),
            id: 9000,
            is_bot: true,
        },
        item: ItemRef {
            kind: ItemKind::Issue,
            number,
            url: format!("https://github.com/enzarb/enzarb-ui/issues/{number}"),
            title: "Add the API".into(),
            body: body.into(),
            author: login.into(),
            head_ref: None,
            base_ref: None,
            head_sha: None,
        },
        comment: None,
        merged: None,
        review: None,
        ci: None,
        label: None,
    };
    let ev = Event::new(
        "github",
        delivery,
        EventType::WorkItemOpened,
        Utc::now(),
        data,
    );
    let _: Option<i64> =
        sqlx::query_scalar("SELECT workflows_api.ingest_event($1, $2, $3, $4, $5)")
            .bind("github")
            .bind(ev.dedupe_key())
            .bind(ev.subject.to_string())
            .bind(EventType::WorkItemOpened.as_str())
            .bind(serde_json::to_value(&ev).unwrap())
            .fetch_one(pool)
            .await
            .unwrap();
}

fn marker(parent: u64, sub: &str) -> String {
    format!(
        "API part.\n\n<!-- enzarb:epic={}:{sub} -->\n<!-- enzarb:epic-depth=1 -->",
        key(parent)
    )
}

fn epic_data(depth: u32) -> Value {
    json!({
        "number": 10, "title": "Epic", "author": "alice", "epic": true, "epic_depth": depth,
        "sub_issues": [{ "key": "api", "title": "API", "body": "b", "depends_on": [] }],
    })
}

#[tokio::test]
async fn the_bot_opens_a_sub_issue_only_for_a_stored_epic() {
    let db = db!();
    let p = db.pool.clone();
    item(&p, 10, "Decomposing", epic_data(0)).await;
    item(&p, 20, "Planning", epic_data(0)).await;
    let reducer = reducer(&p);

    // A sub-issue of the epic that is creating its children starts the normal flow.
    bot_opens(&p, "d1", 11, &marker(10, "api"), "enzarb[bot]").await;
    // The parent is not in a state that creates children.
    bot_opens(&p, "d2", 21, &marker(20, "api"), "enzarb[bot]").await;
    // The parent is unknown.
    bot_opens(&p, "d3", 31, &marker(99, "api"), "enzarb[bot]").await;
    // The key is not in the approved plan.
    bot_opens(&p, "d4", 12, &marker(10, "other"), "enzarb[bot]").await;
    // Another bot with a perfect marker.
    bot_opens(&p, "d5", 13, &marker(10, "api"), "dependabot[bot]").await;
    reducer.drain().await;

    let (s, data) = state_of(&p, 11)
        .await
        .expect("the sub-issue started a workflow");
    assert_eq!(s, "Triaging");
    assert_eq!(data["epic_depth"], 1);
    assert_eq!(data["epic_parent"], key(10));
    for n in [21, 31, 12, 13] {
        assert!(state_of(&p, n).await.is_none(), "#{n} must be ignored");
    }
    // It was triaged like any issue: one triage job, nothing else that writes code.
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT j.kind FROM workflows.jobs j JOIN workflows.work_items w ON w.id = j.work_item_id
         WHERE w.key = $1 AND j.kind LIKE 'agent\\_%'",
    )
    .bind(key(11))
    .fetch_all(&p)
    .await
    .unwrap();
    assert_eq!(kinds, ["agent_triage"]);
}
