//! ADR-0022 generic building blocks against a wiremock GitHub. No live calls.

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use enzarb_connector::{Connector, ItemKind, RepoKey, WorkItemKey};
use enzarb_connector_github::{GitHubConfig, GitHubConnector, RetryPolicy};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const INSTALLATION: u64 = 77;
const PRIVATE_KEY: &str = include_str!("fixtures/test_app_key.pem");

fn repo() -> RepoKey {
    RepoKey::new("github", "enzarb", "enzarb-ui")
}

fn item(n: u64) -> WorkItemKey {
    WorkItemKey::new(repo(), n)
}

async fn setup() -> (MockServer, GitHubConnector) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/installation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": INSTALLATION})))
        .mount(&server)
        .await;
    let exp = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
    Mock::given(method("POST"))
        .and(path(format!(
            "/app/installations/{INSTALLATION}/access_tokens"
        )))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token": "ghs_inst", "expires_at": exp})),
        )
        .mount(&server)
        .await;
    let mut cfg = GitHubConfig::new("1", PRIVATE_KEY);
    cfg.api_url = server.uri();
    cfg.retry = RetryPolicy {
        max_attempts: 1,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(1),
    };
    let c = GitHubConnector::new(cfg).unwrap();
    (server, c)
}

async fn get(server: &MockServer, p: &str, status: u16, body: Value) {
    Mock::given(method("GET"))
        .and(path(p.to_string()))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn resolve_ref_returns_sha_or_none() {
    let (s, c) = setup().await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/commits/v1.0.0",
        200,
        json!({"sha": "abc"}),
    )
    .await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/commits/nope",
        422,
        json!({"message": "No commit found"}),
    )
    .await;
    assert_eq!(
        c.resolve_ref(&repo(), "v1.0.0").await.unwrap().as_deref(),
        Some("abc")
    );
    assert_eq!(c.resolve_ref(&repo(), "nope").await.unwrap(), None);
}

#[tokio::test]
async fn list_open_issues_skips_pull_requests() {
    let (s, c) = setup().await;
    let user = json!({"login": "a", "id": 1});
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"number": 1, "html_url": "u1", "title": "t", "user": user},
            {"number": 2, "html_url": "u2", "user": user, "pull_request": {}},
        ])))
        .mount(&s)
        .await;
    let got = c.list_open_issues(&repo()).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].number, 1);
    assert_eq!(got[0].kind, ItemKind::Issue);
}

#[tokio::test]
async fn pr_references_merge_graphql_and_body() {
    let (s, c) = setup().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data": {"repository": {"pullRequest": {
                "body": "Fixes #3",
                "closingIssuesReferences": {"nodes": [
                    {"number": 5, "repository": {"nameWithOwner": "enzarb/enzarb-ui"}},
                    {"number": 9, "repository": {"nameWithOwner": "other/repo"}},
                ]}
            }}}}),
        ))
        .mount(&s)
        .await;
    assert_eq!(c.pr_references(&item(7)).await.unwrap(), vec![3, 5]);
}

#[tokio::test]
async fn pr_references_fall_back_to_body() {
    let (s, c) = setup().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "no"})))
        .mount(&s)
        .await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/pulls/7",
        200,
        json!({"number": 7, "html_url": "u", "body": "closes #4", "user": {"login": "a", "id": 1},
               "head": {"ref": "h"}, "base": {"ref": "main"}}),
    )
    .await;
    assert_eq!(c.pr_references(&item(7)).await.unwrap(), vec![4]);
}

#[tokio::test]
async fn labels_add_and_remove_idempotently() {
    let (s, c) = setup().await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/issues/3/labels"))
        .and(body_json(json!({"labels": ["needs plan"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&s)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/repos/enzarb/enzarb-ui/issues/3/labels/needs%20plan"))
        .respond_with(
            ResponseTemplate::new(404).set_body_json(json!({"message": "Label does not exist"})),
        )
        .expect(1)
        .mount(&s)
        .await;
    c.add_label(&item(3), "needs plan").await.unwrap();
    c.remove_label(&item(3), "needs plan").await.unwrap();
}

#[tokio::test]
async fn update_pull_request_patches_given_fields() {
    let (s, c) = setup().await;
    Mock::given(method("PATCH"))
        .and(path("/repos/enzarb/enzarb-ui/pulls/7"))
        .and(body_json(json!({"title": "New"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&s)
        .await;
    c.update_pull_request(&item(7), Some("New"), None)
        .await
        .unwrap();
    c.update_pull_request(&item(7), None, None).await.unwrap();
}

#[tokio::test]
async fn ci_details_lists_failures_with_log_tail() {
    let (s, c) = setup().await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/commits/sha1/status",
        200,
        json!({"state": "failure", "total_count": 1, "statuses": [
            {"context": "ci/ext", "state": "error", "target_url": "https://ci/1"}
        ]}),
    )
    .await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/commits/sha1/check-runs",
        200,
        json!({"total_count": 3, "check_runs": [
            {"id": 11, "name": "test", "status": "completed", "conclusion": "failure",
             "html_url": "https://gh/11", "app": {"slug": "github-actions"}},
            {"id": 12, "name": "lint", "status": "completed", "conclusion": "success"},
            {"id": 13, "name": "Copilot review", "status": "completed", "conclusion": "failure"},
        ]}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/actions/jobs/11/logs"))
        .respond_with(ResponseTemplate::new(200).set_body_string("line1\nerror: boom\n"))
        .mount(&s)
        .await;
    let got = c.ci_details(&repo(), "sha1").await.unwrap();
    assert_eq!(
        got,
        json!({"sha": "sha1", "state": "failure", "failing": [
            {"name": "ci/ext", "url": "https://ci/1", "log_excerpt": ""},
            {"name": "test", "url": "https://gh/11", "log_excerpt": "line1\nerror: boom\n"},
        ]})
    );
}

#[tokio::test]
async fn ci_details_pending_and_success() {
    let (s, c) = setup().await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/commits/sha2/status",
        200,
        json!({"state": "pending", "total_count": 0, "statuses": []}),
    )
    .await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/commits/sha2/check-runs",
        200,
        json!({"total_count": 1, "check_runs": [{"id": 1, "name": "t", "status": "in_progress"}]}),
    )
    .await;
    let got = c.ci_details(&repo(), "sha2").await.unwrap();
    assert_eq!(got["state"], "pending");
    assert_eq!(got["failing"], json!([]));
}

#[tokio::test]
async fn list_reviews_keeps_latest_per_author() {
    let (s, c) = setup().await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/pulls/7/reviews",
        200,
        json!([
            {"user": {"login": "a", "id": 1}, "state": "CHANGES_REQUESTED", "body": "fix"},
            {"user": {"login": "b", "id": 2}, "state": "COMMENTED", "body": ""},
            {"user": {"login": "a", "id": 1}, "state": "APPROVED", "body": "ok"},
            {"user": {"login": "c", "id": 3}, "state": "PENDING"},
        ]),
    )
    .await;
    assert_eq!(
        c.list_reviews(&item(7)).await.unwrap(),
        json!([
            {"author": "a", "state": "approved", "body": "ok"},
            {"author": "b", "state": "commented", "body": ""},
        ])
    );
}

#[tokio::test]
async fn contains_uses_compare() {
    let (s, c) = setup().await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/compare/abc...v1",
        200,
        json!({"status": "ahead"}),
    )
    .await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/compare/abc...v0",
        200,
        json!({"status": "diverged"}),
    )
    .await;
    get(
        &s,
        "/repos/enzarb/enzarb-ui/compare/abc...gone",
        404,
        json!({}),
    )
    .await;
    assert_eq!(c.contains(&repo(), "abc", "v1").await.unwrap(), Some(true));
    assert_eq!(c.contains(&repo(), "abc", "v0").await.unwrap(), Some(false));
    assert_eq!(c.contains(&repo(), "abc", "gone").await.unwrap(), None);
}

#[tokio::test]
async fn mint_step_token_requests_exact_permissions() {
    let (s, c) = setup().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/app/installations/{INSTALLATION}/access_tokens"
        )))
        .and(body_json(json!({
            "repositories": ["enzarb-ui"],
            "permissions": {"actions": "read", "contents": "write"},
        })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token": "ghs_step", "expires_at": "2030-01-01T00:00:00Z"})),
        )
        .with_priority(1)
        .expect(1)
        .mount(&s)
        .await;
    let perms: BTreeMap<String, String> = [("contents", "write"), ("actions", "read")]
        .into_iter()
        .map(|(k, v)| (k.into(), v.into()))
        .collect();
    let t = c.mint_step_token(&[repo()], &perms).await.unwrap();
    assert_eq!(t.token, "ghs_step");
    assert!(c.mint_step_token(&[], &perms).await.is_err());
    assert!(
        c.mint_step_token(&[repo()], &BTreeMap::new())
            .await
            .is_err()
    );
}
