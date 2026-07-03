use std::env;

use anyhow::{Context, Result};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use hmac::{Hmac, Mac};
use kube::{
    Api, Client,
    api::{ApiResource, DynamicObject, Patch, PatchParams},
    core::GroupVersionKind,
};
use serde::Deserialize;
use sha2::Sha256;
use tracing::{error, info};

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
struct AppState {
    webhook_secret: Vec<u8>,
    kube_client: Client,
    helmchart_name: String,
    helmchart_namespace: String,
}

#[derive(Deserialize)]
struct ReleasePayload {
    action: String,
    release: Release,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let webhook_secret = env::var("GITHUB_WEBHOOK_SECRET")
        .context("GITHUB_WEBHOOK_SECRET must be set")?
        .into_bytes();

    let kube_client = Client::try_default()
        .await
        .context("failed to create kube client")?;

    let state = AppState {
        webhook_secret,
        kube_client,
        helmchart_name: env::var("HELMCHART_NAME").unwrap_or_else(|_| "enzarb".to_string()),
        helmchart_namespace: env::var("HELMCHART_NAMESPACE")
            .unwrap_or_else(|_| "kube-system".to_string()),
    };

    let app = Router::new()
        .route("/github-webhook", post(handle_webhook))
        .route("/healthz", get(healthz))
        .with_state(state);

    let addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| ":8080".to_string());
    let addr = if let Some(rest) = addr.strip_prefix(':') {
        format!("0.0.0.0:{rest}")
    } else {
        addr
    };

    info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

async fn handle_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let sig = match headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.to_string(),
        None => return (StatusCode::UNAUTHORIZED, "missing x-hub-signature-256").into_response(),
    };

    if !verify_signature(&state.webhook_secret, &body, &sig) {
        return (StatusCode::UNAUTHORIZED, "invalid signature").into_response();
    }

    let event = headers
        .get("x-github-event")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if event != "release" {
        return (StatusCode::OK, "ignored").into_response();
    }

    // GitHub webhooks can be delivered as raw JSON (application/json) or as a
    // form field (application/x-www-form-urlencoded) where the JSON is in the
    // `payload` key. Handle both.
    let json_body: &[u8] = if body.starts_with(b"payload=") {
        let encoded = body.strip_prefix(b"payload=").unwrap_or(&body);
        let decoded = percent_decode(encoded);
        let payload: ReleasePayload = match serde_json::from_slice(&decoded) {
            Ok(p) => p,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("invalid payload: {e}")).into_response();
            }
        };
        return finish_webhook(state, payload).await;
    } else {
        &body
    };
    let payload: ReleasePayload = match serde_json::from_slice(json_body) {
        Ok(p) => p,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("invalid payload: {e}")).into_response();
        }
    };

    finish_webhook(state, payload).await
}

async fn finish_webhook(state: AppState, payload: ReleasePayload) -> axum::response::Response {
    if payload.action != "published" {
        return (StatusCode::OK, "ignored").into_response();
    }

    let tag = &payload.release.tag_name;
    let version = tag.strip_prefix('v').unwrap_or(tag);

    info!(
        "release.published {tag} → patching HelmChart {}/{} to {version}",
        state.helmchart_namespace, state.helmchart_name
    );

    if let Err(e) = patch_helmchart(&state, version).await {
        error!("patch failed: {e:#}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("patch failed: {e}"),
        )
            .into_response();
    }

    info!("HelmChart patched to {version}");
    (StatusCode::OK, format!("patched to {version}")).into_response()
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'%' if i + 2 < input.len() => {
                if let Ok(b) =
                    u8::from_str_radix(std::str::from_utf8(&input[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    out.push(b);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn verify_signature(secret: &[u8], body: &[u8], signature: &str) -> bool {
    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let expected = hex::encode(mac.finalize().into_bytes());
    let expected_sig = format!("sha256={expected}");
    expected_sig == signature
}

async fn patch_helmchart(state: &AppState, version: &str) -> Result<()> {
    let gvk = GroupVersionKind::gvk("helm.cattle.io", "v1", "HelmChart");
    let ar = ApiResource {
        group: gvk.group.to_string(),
        version: gvk.version.to_string(),
        kind: gvk.kind.to_string(),
        api_version: format!("{}/{}", gvk.group, gvk.version),
        plural: "helmcharts".to_string(),
    };

    let api: Api<DynamicObject> =
        Api::namespaced_with(state.kube_client.clone(), &state.helmchart_namespace, &ar);

    let patch = serde_json::json!({ "spec": { "version": version } });
    api.patch(
        &state.helmchart_name,
        &PatchParams::default(),
        &Patch::Merge(patch),
    )
    .await
    .context("patch HelmChart")?;

    Ok(())
}
