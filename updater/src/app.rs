use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};

use crate::{
    charts::Charts,
    filter::{self, Filtered, ReleaseEvent, Upgrade},
    registry::Registry,
    signature,
};

#[derive(Clone)]
pub struct AppState {
    pub secret: Arc<Vec<u8>>,
    pub registry: Arc<dyn Registry>,
    pub charts: Arc<dyn Charts>,
    /// Serializes read-compare-apply so concurrent deliveries cannot apply out of order.
    pub lock: Arc<tokio::sync::Mutex<()>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/hook", post(hook))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

fn hdr<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok())
}

async fn hook(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    if !signature::verify(&st.secret, &body, hdr(&headers, "x-hub-signature-256")) {
        tracing::warn!(decision = "reject", reason = "bad signature");
        return (StatusCode::UNAUTHORIZED, "bad signature");
    }
    match hdr(&headers, "x-github-event") {
        Some("ping") => {
            tracing::info!(decision = "ping");
            return (StatusCode::OK, "pong");
        }
        Some("release") => {}
        other => {
            tracing::info!(decision = "ignore", reason = "event", event = other);
            return (StatusCode::ACCEPTED, "ignored");
        }
    }
    let Ok(ev) = serde_json::from_slice::<ReleaseEvent>(&body) else {
        tracing::warn!(decision = "reject", reason = "bad payload");
        return (StatusCode::BAD_REQUEST, "bad payload");
    };
    let version = match filter::filter(&ev) {
        Filtered::Act(v) => v,
        Filtered::Ignore(reason) => {
            tracing::info!(decision = "ignore", reason);
            return (StatusCode::ACCEPTED, "ignored");
        }
    };
    let v = version.to_string();

    match st.registry.chart_exists(&v).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(decision = "retry", version = %v, reason = "chart not in registry yet");
            return (StatusCode::SERVICE_UNAVAILABLE, "chart not published");
        }
        Err(e) => {
            tracing::error!(decision = "retry", version = %v, error = %format!("{e:#}"), reason = "registry check failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "registry check failed");
        }
    }

    let _guard = st.lock.lock().await;
    let current = match st.charts.current_version().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(decision = "error", version = %v, error = %format!("{e:#}"), reason = "read HelmChart");
            return (StatusCode::INTERNAL_SERVER_ERROR, "cannot read HelmChart");
        }
    };
    match filter::compare(&version, current.as_deref()) {
        Err(e) => {
            tracing::error!(decision = "error", version = %v, error = %e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "current version unparsable",
            )
        }
        Ok(Upgrade::Equal) => {
            tracing::info!(decision = "noop", version = %v, current = current.as_deref(), reason = "already at version");
            (StatusCode::OK, "no-op")
        }
        Ok(Upgrade::Older) => {
            tracing::info!(decision = "noop", version = %v, current = current.as_deref(), reason = "older than current, never downgrade");
            (StatusCode::OK, "no-op")
        }
        Ok(Upgrade::Patch) => match st.charts.set_version(&v).await {
            Ok(()) => {
                tracing::info!(decision = "patched", version = %v, from = current.as_deref());
                (StatusCode::OK, "patched")
            }
            Err(e) => {
                tracing::error!(decision = "error", version = %v, error = %format!("{e:#}"), reason = "patch HelmChart");
                (StatusCode::INTERNAL_SERVER_ERROR, "patch failed")
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anyhow::{Result, anyhow};
    use async_trait::async_trait;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;
    use crate::signature::sign;

    const SECRET: &[u8] = b"whsec";

    struct FakeRegistry(Result<bool, String>);
    #[async_trait]
    impl Registry for FakeRegistry {
        async fn chart_exists(&self, _: &str) -> Result<bool> {
            self.0.clone().map_err(|e| anyhow!(e))
        }
    }

    struct FakeCharts {
        current: Option<String>,
        set: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Charts for FakeCharts {
        async fn current_version(&self) -> Result<Option<String>> {
            Ok(self.current.clone())
        }
        async fn set_version(&self, v: &str) -> Result<()> {
            self.set.lock().unwrap().push(v.into());
            Ok(())
        }
    }

    fn payload(action: &str, repo: &str, tag: &str) -> Vec<u8> {
        serde_json::json!({
            "action": action,
            "repository": {"full_name": repo},
            "release": {"tag_name": tag, "prerelease": false, "draft": false},
        })
        .to_string()
        .into_bytes()
    }

    async fn call(
        reg: Result<bool, String>,
        current: Option<&str>,
        event: &str,
        body: Vec<u8>,
        sig: Option<String>,
    ) -> (StatusCode, Vec<String>) {
        let charts = Arc::new(FakeCharts {
            current: current.map(Into::into),
            set: Mutex::new(vec![]),
        });
        let app = router(AppState {
            secret: Arc::new(SECRET.to_vec()),
            registry: Arc::new(FakeRegistry(reg)),
            charts: charts.clone(),
            lock: Default::default(),
        });
        let sig = sig.unwrap_or_else(|| sign(SECRET, &body));
        let req = Request::post("/hook")
            .header("x-github-event", event)
            .header("x-hub-signature-256", sig)
            .body(Body::from(body))
            .unwrap();
        let st = app.oneshot(req).await.unwrap().status();
        let set = charts.set.lock().unwrap().clone();
        (st, set)
    }

    fn rel(tag: &str) -> Vec<u8> {
        payload("published", filter::REPO, tag)
    }

    #[tokio::test]
    async fn bad_signature_is_401() {
        let (st, set) = call(
            Ok(true),
            Some("1.0.0"),
            "release",
            rel("v1.1.0"),
            Some("sha256=00".into()),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert!(set.is_empty());
    }

    #[tokio::test]
    async fn ping_ok_and_other_events_ignored() {
        assert_eq!(
            call(Ok(true), None, "ping", b"{}".to_vec(), None).await.0,
            StatusCode::OK
        );
        assert_eq!(
            call(Ok(true), None, "push", b"{}".to_vec(), None).await.0,
            StatusCode::ACCEPTED
        );
    }

    #[tokio::test]
    async fn filtered_releases_are_202() {
        let (st, set) = call(
            Ok(true),
            Some("1.0.0"),
            "release",
            payload("created", filter::REPO, "v1.1.0"),
            None,
        )
        .await;
        assert_eq!((st, set.len()), (StatusCode::ACCEPTED, 0));
        let (st, _) = call(
            Ok(true),
            Some("1.0.0"),
            "release",
            payload("published", "x/y", "v1.1.0"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED);
        let (st, _) = call(Ok(true), Some("1.0.0"), "release", rel("v1.1.0-rc.1"), None).await;
        assert_eq!(st, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn upgrade_patches() {
        let (st, set) = call(Ok(true), Some("1.0.0"), "release", rel("v1.1.0"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(set, vec!["1.1.0"]);
    }

    #[tokio::test]
    async fn equal_and_older_are_noops() {
        for cur in ["1.1.0", "1.2.0"] {
            let (st, set) = call(Ok(true), Some(cur), "release", rel("v1.1.0"), None).await;
            assert_eq!(st, StatusCode::OK);
            assert!(set.is_empty());
        }
    }

    #[tokio::test]
    async fn missing_chart_or_registry_error_is_503() {
        for reg in [Ok(false), Err("boom".to_string())] {
            let (st, set) = call(reg, Some("1.0.0"), "release", rel("v1.1.0"), None).await;
            assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
            assert!(set.is_empty());
        }
    }
}
