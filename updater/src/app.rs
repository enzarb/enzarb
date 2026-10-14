use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};

use crate::{
    charts::Charts,
    filter::{self, Filtered, ReleaseEvent},
    poll::PollHealth,
    registry::Registry,
    rollout::{Status, StatusCache},
    signature,
    update::{self, Outcome},
};

#[derive(Clone)]
pub struct AppState {
    /// Webhook secret. `None` disables the webhook: `/hook` is not routed (404).
    pub secret: Option<Arc<Vec<u8>>>,
    /// Set when polling is on; `/readyz` then reflects the last poll.
    pub poll: Option<Arc<PollHealth>>,
    pub registry: Arc<dyn Registry>,
    pub charts: Arc<dyn Charts>,
    /// Serializes read-compare-apply so concurrent deliveries cannot apply out of order.
    pub lock: Arc<tokio::sync::Mutex<()>>,
    /// Chart rollout status, refreshed in the background; `/status` serves it from here.
    pub status: Arc<StatusCache>,
}

pub fn router(state: AppState) -> Router {
    let mut r = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/status", get(chart_status));
    if state.secret.is_some() {
        r = r.route("/hook", post(hook));
    }
    r.with_state(state)
}

fn hdr<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok())
}

async fn hook(
    State(st): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    let Some(secret) = &st.secret else {
        return (StatusCode::NOT_FOUND, "webhook disabled");
    };
    if !signature::verify(secret, &body, hdr(&headers, "x-hub-signature-256")) {
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

    match update::apply(&*st.charts, &st.lock, &version, "webhook").await {
        Outcome::Patched => (StatusCode::OK, "patched"),
        Outcome::Noop => (StatusCode::OK, "no-op"),
        Outcome::Missing => (StatusCode::OK, "no HelmChart, nothing to do"),
        Outcome::Unparsable(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "current version unparsable",
        ),
        Outcome::ReadFailed => (StatusCode::INTERNAL_SERVER_ERROR, "cannot read HelmChart"),
        Outcome::PatchFailed => (StatusCode::INTERNAL_SERVER_ERROR, "patch failed"),
    }
}

async fn chart_status(State(st): State<AppState>) -> Json<Status> {
    Json(st.status.get())
}

async fn readyz(State(st): State<AppState>) -> (StatusCode, &'static str) {
    match &st.poll {
        Some(h) if !h.ok() => (StatusCode::SERVICE_UNAVAILABLE, "last poll failed"),
        _ => (StatusCode::OK, "ok"),
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
    use crate::{charts::Current, signature::sign};

    const SECRET: &[u8] = b"whsec";

    struct FakeRegistry(Result<bool, String>);
    #[async_trait]
    impl Registry for FakeRegistry {
        async fn chart_exists(&self, _: &str) -> Result<bool> {
            self.0.clone().map_err(|e| anyhow!(e))
        }
        async fn list_tags(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
    }

    struct FakeCharts {
        current: Current,
        set: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl Charts for FakeCharts {
        async fn current(&self) -> Result<Current> {
            Ok(self.current.clone())
        }
        async fn set_version(&self, v: &str) -> Result<()> {
            self.set.lock().unwrap().push(v.into());
            Ok(())
        }
        async fn rollout(&self) -> Result<crate::rollout::Rollout> {
            Err(anyhow!("no cluster"))
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
        current: Current,
        event: &str,
        body: Vec<u8>,
        sig: Option<String>,
    ) -> (StatusCode, Vec<String>) {
        let charts = Arc::new(FakeCharts {
            current,
            set: Mutex::new(vec![]),
        });
        let app = router(AppState {
            secret: Some(Arc::new(SECRET.to_vec())),
            poll: None,
            registry: Arc::new(FakeRegistry(reg)),
            charts: charts.clone(),
            lock: Default::default(),
            status: Default::default(),
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

    fn cur(v: &str) -> Current {
        Current::Present(Some(v.into()))
    }

    fn rel(tag: &str) -> Vec<u8> {
        payload("published", filter::REPO, tag)
    }

    #[tokio::test]
    async fn bad_signature_is_401() {
        let (st, set) = call(
            Ok(true),
            cur("1.0.0"),
            "release",
            rel("helm/v1.1.0"),
            Some("sha256=00".into()),
        )
        .await;
        assert_eq!(st, StatusCode::UNAUTHORIZED);
        assert!(set.is_empty());
    }

    #[tokio::test]
    async fn ping_ok_and_other_events_ignored() {
        assert_eq!(
            call(
                Ok(true),
                Current::Present(None),
                "ping",
                b"{}".to_vec(),
                None
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            call(
                Ok(true),
                Current::Present(None),
                "push",
                b"{}".to_vec(),
                None
            )
            .await
            .0,
            StatusCode::ACCEPTED
        );
    }

    #[tokio::test]
    async fn filtered_releases_are_202() {
        let (st, set) = call(
            Ok(true),
            cur("1.0.0"),
            "release",
            payload("created", filter::REPO, "helm/v1.1.0"),
            None,
        )
        .await;
        assert_eq!((st, set.len()), (StatusCode::ACCEPTED, 0));
        let (st, _) = call(
            Ok(true),
            cur("1.0.0"),
            "release",
            payload("published", "x/y", "helm/v1.1.0"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED);
        let (st, _) = call(
            Ok(true),
            cur("1.0.0"),
            "release",
            rel("helm/v1.1.0-rc.1"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn upgrade_patches() {
        let (st, set) = call(Ok(true), cur("1.0.0"), "release", rel("helm/v1.1.0"), None).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(set, vec!["1.1.0"]);
    }

    #[tokio::test]
    async fn equal_and_older_are_noops() {
        for c in ["1.1.0", "1.2.0"] {
            let (st, set) = call(Ok(true), cur(c), "release", rel("helm/v1.1.0"), None).await;
            assert_eq!(st, StatusCode::OK);
            assert!(set.is_empty());
        }
    }

    #[tokio::test]
    async fn missing_helmchart_is_a_noop_and_not_created() {
        let (st, set) = call(
            Ok(true),
            Current::Missing,
            "release",
            rel("helm/v1.1.0"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(set.is_empty());
    }

    async fn status(app: Router, method: &str, path: &str) -> StatusCode {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        app.oneshot(req).await.unwrap().status()
    }

    fn state(secret: Option<&[u8]>, poll: Option<Arc<PollHealth>>) -> AppState {
        AppState {
            secret: secret.map(|s| Arc::new(s.to_vec())),
            poll,
            registry: Arc::new(FakeRegistry(Ok(true))),
            charts: Arc::new(FakeCharts {
                current: Current::Missing,
                set: Mutex::new(vec![]),
            }),
            lock: Default::default(),
            status: Default::default(),
        }
    }

    #[tokio::test]
    async fn healthz_is_ok_text_even_when_registry_and_charts_fail() {
        let mut st = state(None, None);
        st.registry = Arc::new(FakeRegistry(Err("down".into())));
        let app = router(st);
        let req = Request::get("/healthz").body(Body::empty()).unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    #[tokio::test]
    async fn status_serves_the_cached_rollout_as_json() {
        let st = state(None, None);
        let req = || Request::get("/status").body(Body::empty()).unwrap();
        let json = |res: axum::response::Response| async move {
            let b = axum::body::to_bytes(res.into_body(), 4096).await.unwrap();
            serde_json::from_slice::<serde_json::Value>(&b).unwrap()
        };
        // before the first refresh: all null, not stuck
        let res = router(st.clone()).oneshot(req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            json(res).await,
            serde_json::json!({"installed": null, "desired": null, "latest": null, "stuck": false})
        );
        st.status.set_for_test(Status {
            installed: Some("0.9.0".into()),
            desired: Some("0.10.0".into()),
            latest: Some("0.10.0".into()),
            stuck: true,
        });
        let res = router(st).oneshot(req()).await.unwrap();
        assert_eq!(
            json(res).await,
            serde_json::json!({"installed": "0.9.0", "desired": "0.10.0", "latest": "0.10.0", "stuck": true})
        );
    }

    #[tokio::test]
    async fn without_a_secret_hook_is_404() {
        let app = router(state(None, None));
        assert_eq!(
            status(app.clone(), "POST", "/hook").await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(status(app, "GET", "/healthz").await, StatusCode::OK);
    }

    #[tokio::test]
    async fn readyz_reflects_last_poll() {
        let h = Arc::new(PollHealth::default());
        let app = router(state(None, Some(h.clone())));
        assert_eq!(
            status(app.clone(), "GET", "/readyz").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        h.set(true);
        assert_eq!(status(app.clone(), "GET", "/readyz").await, StatusCode::OK);
        h.set(false);
        assert_eq!(
            status(app, "GET", "/readyz").await,
            StatusCode::SERVICE_UNAVAILABLE
        );
        // polling off: always ready
        assert_eq!(
            status(router(state(Some(b"s"), None)), "GET", "/readyz").await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn missing_chart_or_registry_error_is_503() {
        for reg in [Ok(false), Err("boom".to_string())] {
            let (st, set) = call(reg, cur("1.0.0"), "release", rel("helm/v1.1.0"), None).await;
            assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
            assert!(set.is_empty());
        }
    }
}
