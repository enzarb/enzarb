//! `POST /v1/runs` (start by hand) against a real Postgres, and the triggers of the
//! implement-issues template: slash/mention comment, assignment and `manual`.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use enzarb_tracks::api::{AppState, router};
use enzarb_tracks::db;
use enzarb_tracks::github::{RepoSource, Snapshot, SourceError};
use enzarb_tracks::projects::{ProjectLookup, ProjectRef};
use ez_workflow::{Engine, Event, evaluate_triggers};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ImageExt;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

/// The trigger part of the `implement-labeled-issues` template (ui/.../templates.json).
const COND: &str = r#"(event.name == 'issues' and event.action == 'labeled' and event.label.name == 'ez') or (event.name == 'issues' and event.action == 'assigned' and event.assignee.login == 'ez-agent') or (event.name == 'issue_comment' and event.issue.pull_request is undefined and event.comment.user.type != 'Bot' and event.comment.author_association in ['OWNER', 'MEMBER', 'COLLABORATOR'] and event.comment.body | regex_match('(?im)^[[:space:]]*(/ez|@enzarb)[[:space:]]+implement([[:space:]]|$)')) or event.name == 'manual'"#;

fn workflows() -> String {
    format!(
        r#"
version = 1
[workflows.implement]
on = ["issues.labeled", "issues.assigned", "issue_comment.created", "manual"]
if = "{}"
start = "build"
[workflows.implement.nodes.build]
type = "command"
task = "build"
next = "done"
[workflows.implement.nodes.done]
type = "end"
status = "done"
[workflows.review]
on = ["pull_request.opened"]
start = "done"
[workflows.review.nodes.done]
type = "end"
status = "done"
"#,
        COND.replace('"', "\\\"")
    )
}

struct Projects;
#[async_trait]
impl ProjectLookup for Projects {
    async fn owner(&self, repo: &str) -> Result<Option<ProjectRef>, String> {
        Ok((repo == "o/r").then(|| ProjectRef {
            name: "p".into(),
            default_branch: "main".into(),
        }))
    }
}

struct Source;
#[async_trait]
impl RepoSource for Source {
    async fn snapshot(&self, _: &str, _: &str) -> Result<Snapshot, SourceError> {
        Ok(Snapshot {
            sha: "abc1234".into(),
            workflows: Some(workflows()),
            modes: None,
        })
    }
    async fn issue(&self, _: &str, n: u64) -> Result<Option<Value>, SourceError> {
        Ok((n < 100).then(|| {
            json!({"number": n, "title": "Crash", "body": null, "state": "open",
                   "html_url": format!("https://github.com/o/r/issues/{n}"),
                   "user": {"login": "alice"}, "labels": [{"name": "triage"}]})
        }))
    }
}

async fn start(body: Value) -> (StatusCode, Value, sqlx::PgPool) {
    let node = Postgres::default().with_tag("18").start().await.unwrap();
    let port = node.get_host_port_ipv4(5432).await.unwrap();
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = db::connect(&url, 4).await.unwrap();
    db::migrate(&pool).await.unwrap();
    // Keep the container alive for the test's lifetime.
    Box::leak(Box::new(node));
    let (s, v) = post(&pool, body).await;
    (s, v, pool)
}

async fn post(pool: &sqlx::PgPool, body: Value) -> (StatusCode, Value) {
    let app = router(
        AppState::new(TOKEN.as_bytes().to_vec())
            .with_pool(pool.clone())
            .with_routing(Arc::new(Source), Arc::new(Projects)),
        1024 * 1024,
    );
    let req = Request::builder()
        .method("POST")
        .uri("/v1/runs")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn req(workflow: &str, n: u64) -> Value {
    json!({"repo": "O/R", "number": n, "workflow": workflow, "actor": "bob"})
}

#[tokio::test]
async fn manual_start_creates_a_pending_run_and_dedupes() {
    let (status, v, pool) = start(req("implement", 7)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let id = v["run"]["id"].as_str().unwrap().to_string();
    assert!(v["run"]["event_id"].as_i64().unwrap() < 0);

    let (st, key, subject, state): (String, String, Value, Value) = sqlx::query_as(
        "SELECT status, subject_key, subject, state FROM tracks.runs WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((st.as_str(), key.as_str()), ("pending", "o/r#7"));
    assert_eq!(subject["number"], 7);
    assert!(state.to_string().contains("\"manual\""), "{state}");

    // A second live run of the same workflow on the same subject is refused.
    let (status, v) = post(&pool, req("implement", 7)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{v}");
    assert_eq!(v["error"]["code"], "already_running");
    assert_eq!(v["run"], id);
    // Another subject is fine.
    let (status, _) = post(&pool, req("implement", 8)).await;
    assert_eq!(status, StatusCode::OK);
    // Once the first run ended, the same subject can start again (Retry).
    sqlx::query(
        "UPDATE tracks.runs SET status = 'failed', finished_at = now() WHERE id = $1::uuid",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .unwrap();
    let (status, v) = post(&pool, req("implement", 7)).await;
    assert_eq!(status, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn manual_start_refuses_unknown_unlisted_and_missing() {
    let (status, v, pool) = start(req("review", 7)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert_eq!(v["error"]["code"], "not_startable");
    let (status, v) = post(&pool, req("nope", 7)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_such_workflow"))
    );
    let (status, v) = post(&pool, req("implement", 500)).await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (StatusCode::NOT_FOUND, Some("no_such_issue"))
    );
    let (status, _) = post(
        &pool,
        json!({"repo": "x/y", "number": 1, "workflow": "implement", "actor": "b"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = post(
        &pool,
        json!({"repo": "../x", "number": 1, "workflow": "implement", "actor": "b"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

fn matched(e: &Event) -> bool {
    let file = ez_workflow::parse(&workflows()).unwrap();
    evaluate_triggers(&Engine::new(), &file, e)
        .into_iter()
        .find(|t| t.workflow == "implement")
        .unwrap()
        .matched
}

fn comment(body: &str, assoc: &str, user_type: &str, pr: bool) -> Event {
    let mut issue = json!({"number": 3, "title": "T", "user": {"login": "a"}, "html_url": "u"});
    if pr {
        issue["pull_request"] = json!({"url": "x"});
    }
    Event::new(
        "issue_comment",
        Some("created".into()),
        json!({"action": "created", "issue": issue,
               "comment": {"body": body, "author_association": assoc,
                           "user": {"login": "c", "type": user_type}}}),
    )
}

#[test]
fn the_comment_trigger_needs_a_command_at_a_line_start_from_a_writer() {
    assert!(matched(&comment("/ez implement", "OWNER", "User", false)));
    assert!(matched(&comment(
        "Looks good.\n  @EnZarb Implement\nthanks",
        "MEMBER",
        "User",
        false
    )));
    assert!(matched(&comment(
        "/ez implement please",
        "COLLABORATOR",
        "User",
        false
    )));
    // Not at the start of a line, or another command.
    assert!(!matched(&comment(
        "please /ez implement",
        "OWNER",
        "User",
        false
    )));
    assert!(!matched(&comment(
        "`/ez implement`",
        "OWNER",
        "User",
        false
    )));
    assert!(!matched(&comment(
        "/ez implementation",
        "OWNER",
        "User",
        false
    )));
    // Not a writer.
    assert!(!matched(&comment("/ez implement", "NONE", "User", false)));
    assert!(!matched(&comment(
        "/ez implement",
        "CONTRIBUTOR",
        "User",
        false
    )));
    // The bot's own comments never trigger it, even when they quote the command.
    assert!(!matched(&comment("/ez implement", "MEMBER", "Bot", false)));
    // Pull requests are not issues.
    assert!(!matched(&comment("/ez implement", "OWNER", "User", true)));
}

#[test]
fn label_assign_and_manual_triggers() {
    let ev = |name: &str, action: Option<&str>, extra: Value| {
        let mut p = json!({"issue": {"number": 3, "user": {"login": "a"}, "html_url": "u"}});
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        Event::new(name, action.map(Into::into), p)
    };
    assert!(matched(&ev(
        "issues",
        Some("labeled"),
        json!({"label": {"name": "ez"}})
    )));
    assert!(!matched(&ev(
        "issues",
        Some("labeled"),
        json!({"label": {"name": "bug"}})
    )));
    assert!(matched(&ev(
        "issues",
        Some("assigned"),
        json!({"assignee": {"login": "ez-agent"}})
    )));
    assert!(!matched(&ev(
        "issues",
        Some("assigned"),
        json!({"assignee": {"login": "bob"}})
    )));
    assert!(matched(&ev(
        "manual",
        None,
        json!({"sender": {"login": "bob"}})
    )));
}
