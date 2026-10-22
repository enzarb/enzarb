//! The HTTP API end to end through the router (no network).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use enzarb_tracks::api::{AppState, MAX_DRY_RUN_EVENTS, router};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";
const WORKFLOWS: &str = include_str!("../../ez/crates/ez-workflow/tests/fixtures/workflows.toml");
const MODES: &str = include_str!("../../ez/crates/ez-workflow/tests/fixtures/modes.toml");

fn app() -> Router {
    router(AppState::new(TOKEN.as_bytes().to_vec()), 1024 * 1024)
}

async fn call(
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, v)
}

async fn post(path: &str, body: Value) -> (StatusCode, Value) {
    call("POST", path, Some(TOKEN), Some(body)).await
}

#[tokio::test]
async fn health_is_open_and_v1_needs_the_token() {
    assert_eq!(call("GET", "/healthz", None, None).await.0, StatusCode::OK);
    assert_eq!(call("GET", "/readyz", None, None).await.0, StatusCode::OK);
    let (s, v) = call("GET", "/v1/modes/catalog", None, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "unauthorized");
    let wrong = "x".repeat(32);
    assert_eq!(
        call("GET", "/v1/modes/catalog", Some(&wrong), None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (s, _) = call("POST", "/v1/dry-run", Some("short"), Some(json!({}))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn catalog_lists_tools_and_templates() {
    let (s, v) = call("GET", "/v1/modes/catalog", Some(TOKEN), None).await;
    assert_eq!(s, StatusCode::OK);
    let tools: Vec<&str> = v["builtinTools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for t in [
        "Read",
        "Write",
        "Edit",
        "Bash",
        "AskUserQuestion",
        "SwitchMode",
    ] {
        assert!(tools.contains(&t), "{t} missing from {tools:?}");
    }
    assert!(!v["templates"].as_array().unwrap().is_empty());
    assert_eq!(v["miseTasks"], json!([]));
}

#[tokio::test]
async fn modes_parse_validate_and_render_keep_comments() {
    let text = "# our modes\ndefault_mode = \"plan\"\n\n[modes.plan]\n# planning only\ndescription = \"Plan\"\ntools = [\"Read\"]\nnext = [\"build\"]\n\n[modes.build]\ndescription = \"Build\"\n";
    let (s, v) = post("/v1/modes/parse", json!({"text": text})).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["doc"]["defaultMode"], "plan");
    assert_eq!(v["doc"]["modes"][0]["id"], "plan");
    assert_eq!(v["issues"], json!([]));

    let mut doc = v["doc"].clone();
    doc["modes"][1]["description"] = json!("Build it");
    let (s, r) = post("/v1/modes/render", json!({"old": text, "doc": doc})).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    let out = r["text"].as_str().unwrap();
    assert!(
        out.contains("# our modes") && out.contains("# planning only"),
        "{out}"
    );
    assert!(out.contains("description = \"Build it\""), "{out}");

    // A dangling `next` is an error: validate reports it and render refuses.
    let mut bad = doc.clone();
    bad["modes"][0]["next"] = json!(["ghost"]);
    let (s, v) = post("/v1/modes/validate", json!({"doc": bad})).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        v["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["severity"] == "error"),
        "{v}"
    );
    let (s, v) = post("/v1/modes/render", json!({"doc": bad})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["error"]["code"], "invalid_modes");
    assert!(!v["issues"].as_array().unwrap().is_empty());

    let (s, v) = post("/v1/modes/parse", json!({"text": "[modes"})).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["error"]["code"], "invalid_modes_toml");
}

#[tokio::test]
async fn workflows_check_and_render() {
    let (s, v) = post(
        "/v1/workflows/check",
        json!({"workflows_toml": WORKFLOWS, "modes_toml": MODES}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["report"]["diagnostics"], json!([]));
    assert_eq!(v["file"]["workflows"]["fix-issue"]["start"], "triage");

    // Without modes every agent node is an error; with broken modes the file error comes first.
    let (_, v) = post("/v1/workflows/check", json!({"workflows_toml": WORKFLOWS})).await;
    let d = v["report"]["diagnostics"].as_array().unwrap();
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(
        d.iter()
            .all(|x| x["severity"] == "error" && x["path"].as_str().unwrap().ends_with(".mode"))
    );
    let (_, v) = post(
        "/v1/workflows/check",
        json!({"workflows_toml": WORKFLOWS, "modes_toml": "x ="}),
    )
    .await;
    assert_eq!(v["report"]["diagnostics"][0]["path"], "");
    assert!(
        v["report"]["diagnostics"][0]["message"]
            .as_str()
            .unwrap()
            .contains("modes.toml")
    );

    let (_, v) = post(
        "/v1/workflows/check",
        json!({"workflows_toml": "version = 1\n[workflows.w]\n"}),
    )
    .await;
    assert!(v.get("file").is_none());
    assert_eq!(v["report"]["diagnostics"][0]["severity"], "error");

    // Render the parsed file back.
    let (_, checked) = post(
        "/v1/workflows/check",
        json!({"workflows_toml": WORKFLOWS, "modes_toml": MODES}),
    )
    .await;
    let (s, v) = post(
        "/v1/workflows/render",
        json!({"file": checked["file"], "modes_toml": MODES}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["text"]
            .as_str()
            .unwrap()
            .contains("[workflows.fix-issue.nodes.triage]")
    );
    assert_eq!(v["report"]["diagnostics"], json!([]));
}

fn labeled(id: i64, label: &str) -> Value {
    json!({
        "id": id, "event": "issues", "action": "labeled",
        "payload": {"label": {"name": label}, "issue": {"number": 42, "title": "Crash", "body": "Boom", "user": {"login": "alice"}}}
    })
}

#[tokio::test]
async fn dry_runs_logged_events() {
    let (s, v) = post(
        "/v1/dry-run",
        json!({
            "workflows_toml": WORKFLOWS, "modes_toml": MODES,
            "events": [labeled(7, "ez"), labeled(8, "bug")],
            "options": {"assume": {"triage": {"result": {"decision": "fix", "summary": "S"}}}}
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let r = v["results"].as_array().unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["event_id"], 7);
    assert_eq!(r[0]["triggers"][0]["matched"], true);
    let steps = r[0]["traces"][0]["steps"].as_array().unwrap();
    assert_eq!(steps[0]["node"], "triage");
    assert_eq!(steps[1]["node"], "implement");
    assert_eq!(
        steps[1]["rendered"]["instruction"],
        "Fix issue #42. Plan: S"
    );
    assert_eq!(r[0]["traces"][0]["end"]["kind"], "needs_outcome");
    assert_eq!(r[1]["event_id"], 8);
    assert_eq!(r[1]["triggers"][0]["matched"], false);
    assert_eq!(r[1]["traces"], json!([]));
}

#[tokio::test]
async fn dry_run_reports_invalid_files_and_bad_requests() {
    let (s, v) = post(
        "/v1/dry-run",
        json!({"workflows_toml": "nope", "modes_toml": MODES, "events": [labeled(1, "ez")]}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["results"], json!([]));
    assert_eq!(v["report"]["diagnostics"][0]["severity"], "error");

    let (s, v) = post(
        "/v1/dry-run",
        json!({"workflows_toml": WORKFLOWS, "events": []}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "invalid_request");
    let many: Vec<Value> = (0..=MAX_DRY_RUN_EVENTS as i64)
        .map(|i| labeled(i, "ez"))
        .collect();
    let (s, _) = post(
        "/v1/dry-run",
        json!({"workflows_toml": WORKFLOWS, "events": many}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let (s, v) = post("/v1/dry-run", json!({"events": "x"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(v["error"]["code"], "invalid_request");
}

#[tokio::test]
async fn bodies_over_the_limit_are_refused() {
    let big = "x".repeat(2 * 1024 * 1024);
    let (s, _) = post("/v1/modes/parse", json!({"text": big})).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}
