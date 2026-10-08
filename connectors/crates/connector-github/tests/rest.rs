//! REST behaviour against a wiremock GitHub. No live calls.

use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use enzarb_connector::{
    Actor, ChangeRequestSpec, CommentKind, CommitRef, Connector, CredScope, Error, ItemKind,
    Permission, Progress, ProgressTarget, RepoKey, Status, StatusState, WorkItemKey,
};
use enzarb_connector_github::{GitHubConfig, GitHubConnector, RetryPolicy};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

const APP_ID: &str = "123456";
const INSTALLATION: u64 = 55001122;
const PRIVATE_KEY: &str = include_str!("fixtures/test_app_key.pem");
const PUBLIC_KEY: &str = include_str!("fixtures/test_app_key.pub.pem");

fn repo() -> RepoKey {
    RepoKey::new("github", "enzarb", "enzarb-ui")
}

fn item(n: u64) -> WorkItemKey {
    WorkItemKey::new(repo(), n)
}

fn connector(server: &MockServer) -> GitHubConnector {
    let mut cfg = GitHubConfig::new(APP_ID, PRIVATE_KEY);
    cfg.api_url = server.uri();
    cfg.retry = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(20),
    };
    GitHubConnector::new(cfg).unwrap()
}

fn ts(offset_secs: i64) -> String {
    (Utc::now() + chrono::Duration::seconds(offset_secs)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Matches `Authorization: Bearer <RS256 JWT signed by the app key>`.
struct ValidAppJwt;

impl Match for ValidAppJwt {
    fn matches(&self, req: &Request) -> bool {
        let Some(h) = req.headers.get("authorization") else {
            return false;
        };
        let Some(tok) = h.to_str().ok().and_then(|s| s.strip_prefix("Bearer ")) else {
            return false;
        };
        let mut v = Validation::new(Algorithm::RS256);
        v.set_issuer(&[APP_ID]);
        v.set_required_spec_claims(&["exp", "iat", "iss"]);
        let key = DecodingKey::from_rsa_pem(PUBLIC_KEY.as_bytes()).unwrap();
        let Ok(data) = jsonwebtoken::decode::<Value>(tok, &key, &v) else {
            return false;
        };
        // GitHub rejects JWTs valid for more than 10 minutes.
        let iat = data.claims["iat"].as_i64().unwrap();
        let exp = data.claims["exp"].as_i64().unwrap();
        exp - iat <= 600 && exp > Utc::now().timestamp()
    }
}

/// Installation lookup + token endpoints. Returns nothing; callers pass the
/// token expiry offset and expected call counts.
async fn mount_auth(server: &MockServer, token_expiry_secs: i64, lookups: u64, tokens: u64) {
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/installation"))
        .and(ValidAppJwt)
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": INSTALLATION})))
        .expect(lookups)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/app/installations/{INSTALLATION}/access_tokens"
        )))
        .and(ValidAppJwt)
        .respond_with(ResponseTemplate::new(201).set_body_json(
            json!({"token": "ghs_installation", "expires_at": ts(token_expiry_secs)}),
        ))
        .expect(tokens)
        .mount(server)
        .await;
}

fn comment_json(id: u64, body: &str, login: &str, user_id: u64, created: &str) -> Value {
    json!({
        "id": id, "node_id": format!("IC_{id}"), "body": body,
        "html_url": format!("https://github.com/enzarb/enzarb-ui/issues/42#issuecomment-{id}"),
        "user": {"login": login, "id": user_id, "type": "User"},
        "created_at": created, "updated_at": created,
    })
}

// ---- auth ----

#[tokio::test]
async fn installation_and_token_are_cached() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/statuses/abc123"))
        .and(header("authorization", "Bearer ghs_installation"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(2)
        .mount(&server)
        .await;
    let c = connector(&server);
    let target = CommitRef {
        repo: repo(),
        sha: "abc123".into(),
    };
    let status = Status {
        state: StatusState::Success,
        context: "enzarb/test".into(),
        description: "ok".into(),
        target_url: None,
    };
    c.set_status(&target, status.clone(), "k1").await.unwrap();
    c.set_status(&target, status, "k2").await.unwrap();
}

#[tokio::test]
async fn token_refreshed_within_five_minutes_of_expiry() {
    let server = MockServer::start().await;
    // Expires in 4 minutes: always inside the 5-minute margin, so every call
    // mints a new one while the installation id stays cached.
    mount_auth(&server, 240, 1, 2).await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/statuses/abc123"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .mount(&server)
        .await;
    let c = connector(&server);
    let target = CommitRef {
        repo: repo(),
        sha: "abc123".into(),
    };
    let status = Status {
        state: StatusState::Pending,
        context: "enzarb/test".into(),
        description: String::new(),
        target_url: None,
    };
    c.set_status(&target, status.clone(), "k1").await.unwrap();
    c.set_status(&target, status, "k2").await.unwrap();
}

#[tokio::test]
async fn app_not_installed_is_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/installation"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})))
        .mount(&server)
        .await;
    let err = connector(&server)
        .check_permission(
            &repo(),
            &Actor {
                login: "ruckc".into(),
                id: 42,
                is_bot: false,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)), "{err:?}");
}

#[tokio::test]
async fn clone_credential_is_single_repo_and_minimal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/installation"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": INSTALLATION})))
        .mount(&server)
        .await;
    let expires = ts(3600);
    Mock::given(method("POST"))
        .and(path(format!("/app/installations/{INSTALLATION}/access_tokens")))
        .and(ValidAppJwt)
        .and(body_json(json!({
            "repositories": ["enzarb-ui"],
            "permissions": {"contents": "write", "pull_requests": "write"},
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(
            json!({"token": "ghs_scoped", "expires_at": expires, "repository_selection": "selected"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/app/installations/{INSTALLATION}/access_tokens"
        )))
        .and(body_json(json!({
            "repositories": ["enzarb-ui"],
            "permissions": {"contents": "read", "pull_requests": "read"},
        })))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token": "ghs_ro", "expires_at": expires})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let c = connector(&server);
    let rw = c
        .mint_clone_credential(&repo(), CredScope::ReadWrite, "k")
        .await
        .unwrap();
    assert_eq!(rw.token, "ghs_scoped");
    assert_eq!(rw.repo, repo());
    assert_eq!(rw.scope, CredScope::ReadWrite);
    assert!(!format!("{rw:?}").contains("ghs_scoped"));
    let ro = c
        .mint_clone_credential(&repo(), CredScope::Read, "k")
        .await
        .unwrap();
    assert_eq!(ro.token, "ghs_ro");
}

#[tokio::test]
async fn rejects_non_github_repo_and_bad_key() {
    let server = MockServer::start().await;
    let c = connector(&server);
    let other = RepoKey::new("gitlab", "a", "b");
    assert!(matches!(
        c.mint_clone_credential(&other, CredScope::Read, "k").await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        c.post_comment(&item(1), "x", "bad --> key").await,
        Err(Error::Invalid(_))
    ));
    assert!(GitHubConnector::new(GitHubConfig::new("1", "not a pem")).is_err());
}

// ---- comments ----

#[tokio::test]
async fn post_comment_embeds_marker() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            comment_json(1, "unrelated", "alice", 1001, "2026-10-08T12:00:00Z"),
            comment_json(
                2,
                "mentions <!-- enzarb:idem:other -->",
                "enzarb[bot]",
                9000,
                "2026-10-08T12:01:00Z"
            ),
        ])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .and(body_json(
            json!({"body": "Plan ready.\n\n<!-- enzarb:idem:plan-42 -->"}),
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(comment_json(
            3,
            "Plan ready.\n\n<!-- enzarb:idem:plan-42 -->",
            "enzarb[bot]",
            9000,
            "2026-10-08T12:02:00Z",
        )))
        .expect(1)
        .mount(&server)
        .await;
    let got = connector(&server)
        .post_comment(&item(42), "Plan ready.", "plan-42")
        .await
        .unwrap();
    assert_eq!(got.id, 3);
    assert!(got.url.ends_with("#issuecomment-3"));
}

#[tokio::test]
async fn post_comment_skips_when_marker_exists() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    // Marker is on page 2 of the listing.
    let page1: Vec<Value> = (1..=100)
        .map(|i| comment_json(i, "chatter", "alice", 1001, "2026-10-08T12:00:00Z"))
        .collect();
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page1))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([comment_json(
                500,
                "Plan ready.\n\n<!-- enzarb:idem:plan-42 -->",
                "enzarb[bot]",
                9000,
                "2026-10-08T12:30:00Z"
            )])),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;
    let got = connector(&server)
        .post_comment(&item(42), "Plan ready.", "plan-42")
        .await
        .unwrap();
    assert_eq!(got.id, 500);
}

// ---- retries ----

#[tokio::test]
async fn retries_5xx_then_succeeds() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/statuses/abc"))
        .respond_with(ResponseTemplate::new(502))
        .up_to_n_times(2)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/statuses/abc"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let status = Status {
        state: StatusState::Failure,
        context: "c".into(),
        description: "x".repeat(300),
        target_url: Some("https://enzarb.example/run/1".into()),
    };
    connector(&server)
        .set_status(
            &CommitRef {
                repo: repo(),
                sha: "abc".into(),
            },
            status,
            "k",
        )
        .await
        .unwrap();
    // The description was truncated to GitHub's 140 chars.
    let reqs = server.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&reqs.last().unwrap().body).unwrap();
    assert_eq!(body["description"].as_str().unwrap().len(), 140);
    assert_eq!(body["state"], "failure");
    assert_eq!(body["target_url"], "https://enzarb.example/run/1");
}

#[tokio::test]
async fn retries_secondary_rate_limit() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path(
            "/repos/enzarb/enzarb-ui/collaborators/ruckc/permission",
        ))
        .respond_with(
            ResponseTemplate::new(403)
                .insert_header("retry-after", "0")
                .set_body_json(json!({"message": "You have exceeded a secondary rate limit."})),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/repos/enzarb/enzarb-ui/collaborators/ruckc/permission",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"permission": "admin", "role_name": "admin", "user": {"login": "ruckc"}}),
        ))
        .mount(&server)
        .await;
    let p = connector(&server)
        .check_permission(
            &repo(),
            &Actor {
                login: "ruckc".into(),
                id: 42,
                is_bot: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(p, Permission::Admin);
}

#[tokio::test]
async fn exhausted_retries_are_retryable_errors() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/statuses/abc"))
        .respond_with(ResponseTemplate::new(500))
        .expect(3)
        .mount(&server)
        .await;
    let status = Status {
        state: StatusState::Error,
        context: "c".into(),
        description: String::new(),
        target_url: None,
    };
    let err = connector(&server)
        .set_status(
            &CommitRef {
                repo: repo(),
                sha: "abc".into(),
            },
            status,
            "k",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Transient(_)), "{err:?}");
    assert!(err.is_retryable());
}

#[tokio::test]
async fn plain_403_is_permission_denied_without_retry() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/statuses/abc"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"message": "Resource not accessible by integration"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let status = Status {
        state: StatusState::Success,
        context: "c".into(),
        description: String::new(),
        target_url: None,
    };
    let err = connector(&server)
        .set_status(
            &CommitRef {
                repo: repo(),
                sha: "abc".into(),
            },
            status,
            "k",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, Error::PermissionDenied(_)), "{err:?}");
}

// ---- progress ----

fn reaction(id: u64, content: &str, login: &str) -> Value {
    json!({"id": id, "node_id": format!("RE_{id}"), "content": content,
           "user": {"login": login, "id": id, "type": "User"}, "created_at": "2026-10-08T12:00:00Z"})
}

#[tokio::test]
async fn set_progress_replaces_only_our_previous_reaction() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    let base = "/repos/enzarb/enzarb-ui/issues/comments/987/reactions";
    Mock::given(method("POST"))
        .and(path(base))
        .and(body_json(json!({"content": "+1"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(reaction(10, "+1", "enzarb[bot]")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(base))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            reaction(5, "rocket", "enzarb[bot]"),
            reaction(6, "eyes", "enzarb[bot]"),
            reaction(7, "rocket", "alice"),
            reaction(8, "heart", "enzarb[bot]"),
            reaction(10, "+1", "enzarb[bot]"),
        ])))
        .mount(&server)
        .await;
    for id in [5, 6] {
        Mock::given(method("DELETE"))
            .and(path(format!("{base}/{id}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
    }
    // Alice's reaction and the bot's unrelated heart must be left alone.
    for id in [7, 8, 10] {
        Mock::given(method("DELETE"))
            .and(path(format!("{base}/{id}")))
            .respond_with(ResponseTemplate::new(204))
            .expect(0)
            .mount(&server)
            .await;
    }
    let target = ProgressTarget {
        item: item(42),
        comment_id: Some(987),
        comment_kind: Default::default(),
    };
    connector(&server)
        .set_progress(&target, Progress::Done, "k")
        .await
        .unwrap();
}

#[tokio::test]
async fn set_progress_on_review_comment_uses_pulls_comments_endpoint() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    let base = "/repos/enzarb/enzarb-ui/pulls/comments/321/reactions";
    Mock::given(method("POST"))
        .and(path(base))
        .and(body_json(json!({"content": "+1"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(reaction(10, "+1", "enzarb[bot]")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(base))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            reaction(5, "rocket", "enzarb[bot]"),
            reaction(7, "rocket", "alice"),
            reaction(10, "+1", "enzarb[bot]"),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{base}/5")))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("{base}/7")))
        .respond_with(ResponseTemplate::new(204))
        .expect(0)
        .mount(&server)
        .await;
    let target = ProgressTarget {
        item: item(42),
        comment_id: Some(321),
        comment_kind: CommentKind::Review,
    };
    connector(&server)
        .set_progress(&target, Progress::Done, "k")
        .await
        .unwrap();
}

#[tokio::test]
async fn unauthorized_invalidates_cached_token_and_retries_once() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/installation"))
        .and(ValidAppJwt)
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": INSTALLATION})))
        .mount(&server)
        .await;
    let tok_path = format!("/app/installations/{INSTALLATION}/access_tokens");
    Mock::given(method("POST"))
        .and(path(&tok_path))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token": "tok_old", "expires_at": ts(3600)})),
        )
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(&tok_path))
        .respond_with(
            ResponseTemplate::new(201)
                .set_body_json(json!({"token": "tok_new", "expires_at": ts(3600)})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let base = "/repos/enzarb/enzarb-ui/issues/42/reactions";
    Mock::given(method("POST"))
        .and(path(base))
        .and(header("authorization", "Bearer tok_old"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(json!({"message": "Bad credentials"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(base))
        .and(header("authorization", "Bearer tok_new"))
        .respond_with(ResponseTemplate::new(201).set_body_json(reaction(1, "eyes", "enzarb[bot]")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(base))
        .and(header("authorization", "Bearer tok_new"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let target = ProgressTarget {
        item: item(42),
        comment_id: None,
        comment_kind: CommentKind::Conversation,
    };
    connector(&server)
        .set_progress(&target, Progress::Received, "k")
        .await
        .unwrap();
}

#[tokio::test]
async fn set_progress_on_issue_uses_issue_reactions() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    let base = "/repos/enzarb/enzarb-ui/issues/42/reactions";
    Mock::given(method("POST"))
        .and(path(base))
        .and(body_json(json!({"content": "eyes"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(reaction(1, "eyes", "enzarb[bot]")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(base))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([reaction(
            1,
            "eyes",
            "enzarb[bot]"
        )])))
        .mount(&server)
        .await;
    let target = ProgressTarget {
        item: item(42),
        comment_id: None,
        comment_kind: Default::default(),
    };
    connector(&server)
        .set_progress(&target, Progress::Received, "k")
        .await
        .unwrap();
}

#[tokio::test]
async fn progress_reaction_names() {
    for (p, want) in [
        (Progress::Received, "eyes"),
        (Progress::Working, "rocket"),
        (Progress::Done, "+1"),
        (Progress::Failed, "confused"),
    ] {
        let server = MockServer::start().await;
        mount_auth(&server, 3600, 1, 1).await;
        let base = "/repos/enzarb/enzarb-ui/issues/42/reactions";
        Mock::given(method("POST"))
            .and(path(base))
            .and(body_json(json!({"content": want})))
            .respond_with(ResponseTemplate::new(201).set_body_json(reaction(
                1,
                want,
                "enzarb[bot]",
            )))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(base))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .mount(&server)
            .await;
        let target = ProgressTarget {
            item: item(42),
            comment_id: None,
            comment_kind: Default::default(),
        };
        connector(&server)
            .set_progress(&target, p, "k")
            .await
            .unwrap();
    }
}

// ---- change requests ----

fn pr_json(n: u64) -> Value {
    json!({
        "number": n, "html_url": format!("https://github.com/enzarb/enzarb-ui/pull/{n}"),
        "title": "Fix sidebar overflow", "body": "Closes #42", "state": "open",
        "user": {"login": "enzarb[bot]", "id": 9000, "type": "Bot"},
        "head": {"ref": "enzarb/issue-42", "sha": "4f9c2a1b", "label": "enzarb:enzarb/issue-42"},
        "base": {"ref": "main", "sha": "01234567", "label": "enzarb:main"},
    })
}

fn spec() -> ChangeRequestSpec {
    ChangeRequestSpec {
        repo: repo(),
        title: "Fix sidebar overflow".into(),
        body: "Closes #42".into(),
        head_ref: "enzarb/issue-42".into(),
        base_ref: "main".into(),
        draft: true,
    }
}

#[tokio::test]
async fn open_change_request_returns_existing_pr() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .and(query_param("head", "enzarb:enzarb/issue-42"))
        .and(query_param("state", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([pr_json(57)])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&server)
        .await;
    let got = connector(&server)
        .open_change_request(spec(), "k")
        .await
        .unwrap();
    assert_eq!(got.number, 57);
    assert_eq!(got.kind, ItemKind::ChangeRequest);
    assert_eq!(got.head_ref.as_deref(), Some("enzarb/issue-42"));
    assert_eq!(got.base_ref.as_deref(), Some("main"));
}

#[tokio::test]
async fn open_change_request_creates_when_missing() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .and(body_json(json!({
            "title": "Fix sidebar overflow", "body": "Closes #42",
            "head": "enzarb/issue-42", "base": "main", "draft": true,
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(pr_json(58)))
        .expect(1)
        .mount(&server)
        .await;
    let got = connector(&server)
        .open_change_request(spec(), "k")
        .await
        .unwrap();
    assert_eq!(got.number, 58);
    assert_eq!(got.head_sha.as_deref(), Some("4f9c2a1b"));
}

#[tokio::test]
async fn open_change_request_recovers_from_creation_race() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([pr_json(59)])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/repos/enzarb/enzarb-ui/pulls"))
        .respond_with(ResponseTemplate::new(422).set_body_json(json!({
            "message": "Validation Failed",
            "errors": [{"resource": "PullRequest", "code": "custom",
                        "message": "A pull request already exists for enzarb:enzarb/issue-42."}],
        })))
        .mount(&server)
        .await;
    let got = connector(&server)
        .open_change_request(spec(), "k")
        .await
        .unwrap();
    assert_eq!(got.number, 59);
}

// ---- permission ----

#[tokio::test]
async fn check_permission_maps_roles() {
    let cases = [
        (
            json!({"permission": "admin", "role_name": "admin"}),
            Permission::Admin,
        ),
        (
            json!({"permission": "write", "role_name": "maintain"}),
            Permission::Maintain,
        ),
        (
            json!({"permission": "write", "role_name": "write"}),
            Permission::Write,
        ),
        (
            json!({"permission": "read", "role_name": "triage"}),
            Permission::Triage,
        ),
        (
            json!({"permission": "read", "role_name": "read"}),
            Permission::Read,
        ),
        (
            json!({"permission": "none", "role_name": "none"}),
            Permission::None,
        ),
        // Older / Enterprise responses without role_name.
        (json!({"permission": "write"}), Permission::Write),
        (
            json!({"permission": "weird", "role_name": "custom-role"}),
            Permission::None,
        ),
    ];
    for (body, want) in cases {
        let server = MockServer::start().await;
        mount_auth(&server, 3600, 1, 1).await;
        Mock::given(method("GET"))
            .and(path(
                "/repos/enzarb/enzarb-ui/collaborators/alice/permission",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;
        let got = connector(&server)
            .check_permission(
                &repo(),
                &Actor {
                    login: "alice".into(),
                    id: 1001,
                    is_bot: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(got, want, "{body}");
    }
}

#[tokio::test]
async fn check_permission_unknown_user_is_none() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path(
            "/repos/enzarb/enzarb-ui/collaborators/ghost/permission",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Not Found"})))
        .mount(&server)
        .await;
    let got = connector(&server)
        .check_permission(
            &repo(),
            &Actor {
                login: "ghost".into(),
                id: 1,
                is_bot: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(got, Permission::None);
}

// ---- fetch_thread ----

fn issue_json(n: u64, pr: bool) -> Value {
    let mut v = json!({
        "number": n, "html_url": format!("https://github.com/enzarb/enzarb-ui/issues/{n}"),
        "title": "Sidebar overflows on mobile", "body": "It overflows.",
        "user": {"login": "ruckc", "id": 42, "type": "User"}, "state": "open",
        "created_at": "2026-10-08T12:00:00Z", "updated_at": "2026-10-08T12:00:00Z",
    });
    if pr {
        v["pull_request"] =
            json!({"url": format!("https://api.github.com/repos/enzarb/enzarb-ui/pulls/{n}")});
    }
    v
}

#[tokio::test]
async fn fetch_thread_issue_paginates_comments() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(42, false)))
        .mount(&server)
        .await;
    let page1: Vec<Value> = (1..=100)
        .map(|i| comment_json(i, "c", "alice", 1001, "2026-10-08T12:00:00Z"))
        .collect();
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page1))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/42/comments"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([comment_json(
                101,
                "last",
                "ruckc",
                42,
                "2026-10-08T13:00:00Z"
            )])),
        )
        .expect(1)
        .mount(&server)
        .await;
    let t = connector(&server).fetch_thread(&item(42)).await.unwrap();
    assert_eq!(t.item.kind, ItemKind::Issue);
    assert_eq!(t.item.title, "Sidebar overflows on mobile");
    assert_eq!(t.item.author, "ruckc");
    assert_eq!(t.comments.len(), 101);
    assert_eq!(t.comments.last().unwrap().author.login, "ruckc");
}

#[tokio::test]
async fn fetch_thread_pr_merges_review_comments_in_time_order() {
    let server = MockServer::start().await;
    mount_auth(&server, 3600, 1, 1).await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/57"))
        .respond_with(ResponseTemplate::new(200).set_body_json(issue_json(57, true)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls/57"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pr_json(57)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/issues/57/comments"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            comment_json(10, "first", "alice", 1001, "2026-10-08T12:00:00Z"),
            comment_json(12, "third", "alice", 1001, "2026-10-08T14:00:00Z"),
        ])))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/enzarb/enzarb-ui/pulls/57/comments"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!([comment_json(
                11,
                "second (inline)",
                "ruckc",
                42,
                "2026-10-08T13:00:00Z"
            ),])),
        )
        .mount(&server)
        .await;
    let t = connector(&server).fetch_thread(&item(57)).await.unwrap();
    assert_eq!(t.item.kind, ItemKind::ChangeRequest);
    assert_eq!(t.item.head_ref.as_deref(), Some("enzarb/issue-42"));
    let bodies: Vec<_> = t.comments.iter().map(|c| c.comment.body.as_str()).collect();
    assert_eq!(bodies, ["first", "second (inline)", "third"]);
}
