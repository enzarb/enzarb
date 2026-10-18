//! The recorded GitHub fixtures drive the real connector (wiremock) and
//! `FakeConnector`; both must give the same result for each quirk.

use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use enzarb_connector::fake::FakeConnector;
use enzarb_connector::{Connector, MergeReadiness, RepoKey, WorkItemKey};
use enzarb_connector_github::{GitHubConfig, GitHubConnector, RetryPolicy};
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const APP_ID: &str = "123456";
const INSTALLATION: u64 = 55001122;
const PRIVATE_KEY: &str = include_str!("fixtures/test_app_key.pem");

fn repo() -> RepoKey {
    RepoKey::new("github", "enzarb", "enzarb-ui")
}

fn item(n: u64) -> WorkItemKey {
    WorkItemKey::new(repo(), n)
}

fn connector(server: &MockServer) -> GitHubConnector {
    // reqwest has no default rustls provider; Err means one is already installed.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut cfg = GitHubConfig::new(APP_ID, PRIVATE_KEY);
    cfg.api_url = server.uri();
    cfg.retry = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(20),
    };
    GitHubConnector::new(cfg).unwrap()
}

async fn mount_auth(server: &MockServer) {
    let expires =
        (Utc::now() + chrono::Duration::seconds(3600)).to_rfc3339_opts(SecondsFormat::Secs, true);
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/installation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": INSTALLATION})))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/app/installations/{INSTALLATION}/access_tokens"
        )))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token": "ghs_installation", "expires_at": expires})),
        )
        .mount(server)
        .await;
}

/// A fixture file without its `_source` note.
fn fixture(name: &str) -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../connector/tests/fixtures/github/"
    );
    let text = std::fs::read_to_string(format!("{path}{name}")).unwrap();
    let mut v: Value = serde_json::from_str(&text).unwrap();
    v.as_object_mut().unwrap().remove("_source");
    v
}

/// `(status, body)` of an error fixture.
fn error_fixture(name: &str) -> (u16, Value) {
    let v = fixture(name);
    (
        u16::try_from(v["status"].as_u64().unwrap()).unwrap(),
        v["body"].clone(),
    )
}

fn readiness_of(pull: &Value) -> MergeReadiness {
    MergeReadiness {
        head_sha: pull["head"]["sha"].as_str().unwrap().into(),
        base_sha: pull["base"]["sha"].as_str().map(Into::into),
        mergeable: pull["mergeable"].as_bool(),
        mergeable_state: pull["mergeable_state"].as_str().unwrap().into(),
        ci: enzarb_connector::CiState::Pending,
    }
}

/// The part of readiness the pull request itself decides (CI comes from other endpoints).
fn pull_fields(r: &MergeReadiness) -> (String, Option<String>, Option<bool>, String) {
    (
        r.head_sha.clone(),
        r.base_sha.clone(),
        r.mergeable,
        r.mergeable_state.clone(),
    )
}

async fn mount_pull_once(server: &MockServer, pull: &Value, once: bool) {
    let head = pull["head"]["sha"].as_str().unwrap();
    let m = Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull));
    if once {
        m.up_to_n_times(1).mount(server).await;
    } else {
        m.mount(server).await;
    }
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/enzarb/enzarb-ui/commits/{head}/status"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"state": "pending", "total_count": 0, "statuses": []})),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/repos/enzarb/enzarb-ui/commits/{head}/check-runs"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"total_count": 0, "check_runs": []})),
        )
        .mount(server)
        .await;
}

#[tokio::test]
async fn mergeable_null_then_known() {
    let null = fixture("pull_mergeable_null.json");
    let known = fixture("pull_mergeable_known.json");

    let server = MockServer::start().await;
    mount_auth(&server).await;
    mount_pull_once(&server, &null, true).await;
    mount_pull_once(&server, &known, false).await;
    let real = connector(&server);
    let real_first = real.get_merge_readiness(&item(7), 7).await.unwrap();
    let real_second = real.get_merge_readiness(&item(7), 7).await.unwrap();

    let fake = FakeConnector::new();
    fake.set_merge_readiness(item(7), readiness_of(&known));
    let fake_first = fake.get_merge_readiness(&item(7), 7).await.unwrap();
    let fake_second = fake.get_merge_readiness(&item(7), 7).await.unwrap();

    assert_eq!(pull_fields(&real_first), pull_fields(&fake_first));
    assert_eq!(pull_fields(&real_second), pull_fields(&fake_second));
    // The quirk itself: unknown first, then computed.
    assert_eq!(real_first.mergeable, None);
    assert_eq!(real_first.mergeable_state, "unknown");
    assert!(real_second.mergeable.is_some());
    assert_ne!(real_second.mergeable_state, "unknown");
}

async fn mount_issue_id(server: &MockServer, n: u64, id: u64) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/enzarb/enzarb-ui/issues/{n}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": id, "number": n})))
        .mount(server)
        .await;
}

async fn mount_422(server: &MockServer, post_path: &str, fixture_name: &str) {
    let (status, body) = error_fixture(fixture_name);
    Mock::given(method("POST"))
        .and(path(post_path))
        .respond_with(ResponseTemplate::new(status).set_body_json(body))
        .expect(1)
        .mount(server)
        .await;
}

async fn mount_list(server: &MockServer, list_path: &str, rows: Value) {
    Mock::given(method("GET"))
        .and(path(list_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(rows))
        .mount(server)
        .await;
}

/// The fake's duplicate rejection must carry the fixture's status and error text.
fn assert_rejection_matches(fake: &FakeConnector, op: &str, fixture_name: &str) {
    let (status, body) = error_fixture(fixture_name);
    let rejected = fake.rejected_duplicates();
    assert_eq!(rejected.len(), 1);
    assert_eq!(rejected[0].op, op);
    assert_eq!(rejected[0].status, status);
    assert!(
        rejected[0]
            .message
            .contains(body["errors"][0].as_str().unwrap()),
        "{:?} vs {body}",
        rejected[0].message
    );
}

/// No request reached the task-list fallback (it edits the parent's body).
async fn assert_no_fallback(server: &MockServer) {
    for r in server.received_requests().await.unwrap() {
        assert!(
            r.method.as_str() != "PATCH" && !r.url.path().ends_with("/issues/1"),
            "task-list fallback used: {} {}",
            r.method,
            r.url.path()
        );
    }
}

#[tokio::test]
async fn sub_issue_duplicate_422() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    mount_issue_id(&server, 8, 5008).await;
    mount_422(
        &server,
        "/repos/enzarb/enzarb-ui/issues/1/sub_issues",
        "sub_issue_duplicate_422.json",
    )
    .await;
    mount_list(
        &server,
        "/repos/enzarb/enzarb-ui/issues/1/sub_issues",
        json!([{"number": 8}]),
    )
    .await;
    let real = connector(&server).link_sub_issue(&item(1), &item(8)).await;

    let fake = FakeConnector::new();
    fake.link_sub_issue(&item(1), &item(8)).await.unwrap();
    let fake_dup = fake.link_sub_issue(&item(1), &item(8)).await;

    assert!(real.is_ok(), "{real:?}");
    assert_eq!(real.is_ok(), fake_dup.is_ok());
    assert_eq!(fake.sub_issues(&item(1)), vec![item(8)]);
    assert_rejection_matches(&fake, "link_sub_issue", "sub_issue_duplicate_422.json");
    assert_no_fallback(&server).await;
}

#[tokio::test]
async fn dependency_duplicate_422() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    mount_issue_id(&server, 3, 777).await;
    mount_422(
        &server,
        "/repos/enzarb/enzarb-ui/issues/5/dependencies/blocked_by",
        "dependency_duplicate_422.json",
    )
    .await;
    mount_list(
        &server,
        "/repos/enzarb/enzarb-ui/issues/5/dependencies/blocked_by",
        json!([{"number": 3, "repository_url": "https://api.github.com/repos/enzarb/enzarb-ui"}]),
    )
    .await;
    let real = connector(&server)
        .add_dependency(&item(5), &item(3), "k")
        .await;

    let fake = FakeConnector::new();
    fake.add_dependency(&item(5), &item(3), "k1").await.unwrap();
    let fake_dup = fake.add_dependency(&item(5), &item(3), "k2").await;

    assert!(real.is_ok(), "{real:?}");
    assert_eq!(real.is_ok(), fake_dup.is_ok());
    assert_eq!(fake.dependencies(&item(5)), vec![item(3)]);
    assert_rejection_matches(&fake, "add_dependency", "dependency_duplicate_422.json");
}

/// A 422 alone is not a duplicate: the edge must also be listed.
#[tokio::test]
async fn dependency_422_without_the_edge_is_an_error() {
    let server = MockServer::start().await;
    mount_auth(&server).await;
    mount_issue_id(&server, 3, 777).await;
    mount_422(
        &server,
        "/repos/enzarb/enzarb-ui/issues/5/dependencies/blocked_by",
        "dependency_duplicate_422.json",
    )
    .await;
    mount_list(
        &server,
        "/repos/enzarb/enzarb-ui/issues/5/dependencies/blocked_by",
        json!([]),
    )
    .await;
    let real = connector(&server)
        .add_dependency(&item(5), &item(3), "k")
        .await;
    assert!(real.is_err(), "{real:?}");
}
