//! The HTTP API (cluster-internal; the UI calls it after its own access checks).
//!
//! Envelopes and everything from `ez-workflow` are snake_case; mode documents keep ez-core's
//! camelCase (`ModesDoc`, `Issue`, `Catalog`), the shape `ez modes --json` already has.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ez_core::modes_api::{self, Issue, ModesDoc};
use ez_workflow::validate::{Diagnostic, Severity};
use ez_workflow::{DryRunOptions, Engine, Event, ModeCatalog, Report, TriggerEval, WorkflowsFile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use subtle::ConstantTimeEq;

/// Most events in one dry-run request.
pub const MAX_DRY_RUN_EVENTS: usize = 50;

#[derive(Clone)]
pub struct AppState {
    pub token: Arc<Vec<u8>>,
    pub engine: Arc<Engine>,
}

impl AppState {
    pub fn new(token: Vec<u8>) -> Self {
        Self {
            token: Arc::new(token),
            engine: Arc::new(Engine::new()),
        }
    }
}

pub fn router(state: AppState, max_body_bytes: usize) -> Router {
    let v1 = Router::new()
        .route("/v1/modes/catalog", get(modes_catalog))
        .route("/v1/modes/parse", post(modes_parse))
        .route("/v1/modes/validate", post(modes_validate))
        .route("/v1/modes/render", post(modes_render))
        .route("/v1/workflows/check", post(workflows_check))
        .route("/v1/workflows/render", post(workflows_render))
        .route("/v1/dry-run", post(dry_run))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth));
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(|| async { "ok" }))
        .merge(v1)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .with_state(state)
}

// ----- errors and auth -----

/// `{"error": {"code", "message"}}` with a status.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    // Boxed: with serde_json's preserve_order (unified from other crates) a Value is large.
    extra: Option<Box<(&'static str, Value)>>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            extra: None,
        }
    }

    fn with(mut self, key: &'static str, value: Value) -> Self {
        self.extra = Some(Box::new((key, value)));
        self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = json!({ "error": { "code": self.code, "message": self.message } });
        if let Some(extra) = self.extra {
            let (k, v) = *extra;
            body[k] = v;
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<JsonRejection> for ApiError {
    fn from(r: JsonRejection) -> Self {
        let status = if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        };
        ApiError::new(status, "invalid_request", r.body_text())
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

async fn auth(State(s): State<AppState>, req: Request, next: Next) -> Response {
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| bool::from(t.as_bytes().ct_eq(s.token.as_slice())));
    if !ok {
        return ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or wrong bearer token",
        )
        .into_response();
    }
    next.run(req).await
}

/// Runs CPU-bound work (templates, validation) off the async workers.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| {
        tracing::error!(error = %e, "blocking task failed");
        ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "internal error",
        )
    })
}

// ----- modes -----

async fn modes_catalog() -> Json<modes_api::Catalog> {
    Json(modes_api::base_catalog())
}

#[derive(Deserialize)]
struct ModesParseReq {
    text: String,
}

#[derive(Serialize)]
struct ModesParseResp {
    doc: ModesDoc,
    issues: Vec<Issue>,
}

/// Parses `.ez/modes.toml` leniently (cross references become issues, not errors) for the editor.
async fn modes_parse(
    body: Result<Json<ModesParseReq>, JsonRejection>,
) -> ApiResult<ModesParseResp> {
    let Json(req) = body?;
    let mut doc = modes_api::parse_doc(&req.text).map_err(|e| {
        ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_modes_toml",
            format!("{e:#}"),
        )
    })?;
    // As ez's own loader does: the on-disk id lets `render` follow a rename (section headers are
    // rewritten in place instead of the mode being dropped and re-added).
    for m in &mut doc.modes {
        m.original_id = Some(m.id.clone());
    }
    let issues = modes_api::validate_doc(&doc, None);
    Ok(Json(ModesParseResp { doc, issues }))
}

#[derive(Deserialize)]
struct ModesValidateReq {
    doc: ModesDoc,
}

#[derive(Serialize)]
struct IssuesResp {
    issues: Vec<Issue>,
}

async fn modes_validate(
    body: Result<Json<ModesValidateReq>, JsonRejection>,
) -> ApiResult<IssuesResp> {
    let Json(req) = body?;
    Ok(Json(IssuesResp {
        issues: modes_api::validate_doc(&req.doc, None),
    }))
}

#[derive(Deserialize)]
struct ModesRenderReq {
    /// The file the editor loaded (keeps its comments and layout); absent for a new file.
    #[serde(default)]
    old: Option<String>,
    doc: ModesDoc,
}

#[derive(Serialize)]
struct ModesRenderResp {
    text: String,
    issues: Vec<Issue>,
    /// Modes written out fresh because their layout couldn't be edited in place.
    rewritten: Vec<String>,
}

/// Writes a document back to TOML, changing only what was edited. Refuses a document with
/// errors, so an invalid file is never produced for a commit.
async fn modes_render(
    body: Result<Json<ModesRenderReq>, JsonRejection>,
) -> ApiResult<ModesRenderResp> {
    let Json(req) = body?;
    let issues = modes_api::validate_doc(&req.doc, None);
    if issues.iter().any(|i| i.severity == "error") {
        return Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_modes",
            "the modes have errors",
        )
        .with("issues", serde_json::to_value(&issues).unwrap_or_default()));
    }
    let r = blocking(move || ez_core::modes_edit::render(req.old.as_deref(), &req.doc))
        .await?
        .map_err(|e| {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "render_failed",
                format!("{e:#}"),
            )
        })?;
    Ok(Json(ModesRenderResp {
        text: r.text,
        issues,
        rewritten: r.rewritten,
    }))
}

// ----- workflows -----

/// The modes a workflows file is checked against. An unparseable modes file is reported as an
/// error and checked as an empty catalog (so agent nodes also report their unknown modes).
fn mode_catalog(modes_toml: Option<&str>, report: &mut Vec<Diagnostic>) -> Option<ModeCatalog> {
    let text = modes_toml?;
    match ModeCatalog::from_modes_toml(text) {
        Ok(c) => Some(c),
        Err(e) => {
            report.push(Diagnostic {
                severity: Severity::Error,
                path: String::new(),
                message: e,
            });
            Some(ModeCatalog::default())
        }
    }
}

#[derive(Deserialize)]
struct WorkflowsCheckReq {
    workflows_toml: String,
    /// `.ez/modes.toml` of the same commit; absent when the commit has none.
    #[serde(default)]
    modes_toml: Option<String>,
}

#[derive(Serialize)]
struct WorkflowsCheckResp {
    report: Report,
    /// The parsed file, for the graph designer (absent when it doesn't parse).
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<WorkflowsFile>,
}

async fn workflows_check(
    State(s): State<AppState>,
    body: Result<Json<WorkflowsCheckReq>, JsonRejection>,
) -> ApiResult<WorkflowsCheckResp> {
    let Json(req) = body?;
    let resp = blocking(move || {
        let mut pre = Vec::new();
        let modes = mode_catalog(req.modes_toml.as_deref(), &mut pre);
        let (file, mut report) = ez_workflow::check(&s.engine, &req.workflows_toml, modes.as_ref());
        pre.append(&mut report.diagnostics);
        report.diagnostics = pre;
        WorkflowsCheckResp { report, file }
    })
    .await?;
    Ok(Json(resp))
}

#[derive(Deserialize)]
struct WorkflowsRenderReq {
    file: WorkflowsFile,
    #[serde(default)]
    modes_toml: Option<String>,
}

#[derive(Serialize)]
struct WorkflowsRenderResp {
    text: String,
    report: Report,
}

/// Writes a workflows document (the designer's JSON) as TOML, with its validation report.
/// Comments are not preserved.
async fn workflows_render(
    State(s): State<AppState>,
    body: Result<Json<WorkflowsRenderReq>, JsonRejection>,
) -> ApiResult<WorkflowsRenderResp> {
    let Json(req) = body?;
    blocking(move || {
        let text = ez_workflow::to_toml(&req.file)
            .map_err(|e| ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "render_failed", e))?;
        let mut pre = Vec::new();
        let modes = mode_catalog(req.modes_toml.as_deref(), &mut pre);
        let (_, mut report) = ez_workflow::check(&s.engine, &text, modes.as_ref());
        pre.append(&mut report.diagnostics);
        report.diagnostics = pre;
        Ok(Json(WorkflowsRenderResp { text, report }))
    })
    .await?
}

// ----- dry run -----

#[derive(Deserialize)]
struct DryRunEvent {
    /// `connectors.event_v.id`, echoed back.
    id: i64,
    /// `connectors.event_v.event`.
    event: String,
    #[serde(default)]
    action: Option<String>,
    payload: Value,
}

#[derive(Deserialize)]
struct DryRunReq {
    workflows_toml: String,
    #[serde(default)]
    modes_toml: Option<String>,
    events: Vec<DryRunEvent>,
    #[serde(default)]
    options: DryRunOptions,
}

#[derive(Serialize)]
struct DryRunResult {
    event_id: i64,
    triggers: Vec<TriggerEval>,
    traces: Vec<ez_workflow::Trace>,
}

#[derive(Serialize)]
struct DryRunResp {
    report: Report,
    /// One per event, in request order (empty when the file has errors).
    results: Vec<DryRunResult>,
}

/// Dry-runs a (possibly unsaved) workflows file against logged events: no sandbox, no GitHub
/// writes, no model calls (ADR-0029 §6).
async fn dry_run(
    State(s): State<AppState>,
    body: Result<Json<DryRunReq>, JsonRejection>,
) -> ApiResult<DryRunResp> {
    let Json(req) = body?;
    if req.events.is_empty() || req.events.len() > MAX_DRY_RUN_EVENTS {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            format!("send 1-{MAX_DRY_RUN_EVENTS} events"),
        ));
    }
    let resp = blocking(move || {
        let mut pre = Vec::new();
        let modes = mode_catalog(req.modes_toml.as_deref(), &mut pre);
        let file = match ez_workflow::parse(&req.workflows_toml) {
            Ok(f) => f,
            Err(e) => {
                pre.push(Diagnostic {
                    severity: Severity::Error,
                    path: String::new(),
                    message: e,
                });
                return DryRunResp {
                    report: Report { diagnostics: pre },
                    results: Vec::new(),
                };
            }
        };
        let mut report = ez_workflow::validate(&s.engine, &file, modes.as_ref());
        pre.append(&mut report.diagnostics);
        let report = Report { diagnostics: pre };
        if !report.is_ok() {
            return DryRunResp {
                report,
                results: Vec::new(),
            };
        }
        let results = req
            .events
            .into_iter()
            .map(|e| {
                let ev = Event::new(e.event, e.action, e.payload);
                let d = ez_workflow::dry_run(&s.engine, &file, modes.as_ref(), &ev, &req.options);
                DryRunResult {
                    event_id: e.id,
                    triggers: d.triggers,
                    traces: d.traces,
                }
            })
            .collect();
        DryRunResp { report, results }
    })
    .await?;
    Ok(Json(resp))
}
