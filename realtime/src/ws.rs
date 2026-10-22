//! HTTP surface: `GET /live/v1/stream` (websocket), `/healthz`, `/readyz`.
//!
//! The stream JWT comes from the first *present* of: `Authorization: Bearer`, the `enzarb_live`
//! cookie (needs an allowed `Origin`, CSWSH protection), the deprecated `?token=` query parameter.
//! A present source never falls through to a later one.

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::{
        HeaderMap, StatusCode,
        header::{AUTHORIZATION, COOKIE, HOST, ORIGIN, RETRY_AFTER},
    },
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, broadcast::error::RecvError, watch},
    time::Instant,
};

use crate::{
    auth::{self, Claims},
    hub::{Frame, Hub},
};

/// Application close code: the token expired, reconnect with a fresh one.
pub const CLOSE_TOKEN_EXPIRED: u16 = 4001;
/// Close code 1001: the server is shutting down, or the client stopped answering pings.
pub const CLOSE_GOING_AWAY: u16 = 1001;
/// Close code 1011: unexpected server-side condition.
pub const CLOSE_INTERNAL: u16 = 1011;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    pub secret: Arc<Vec<u8>>,
    pub ping_interval: Duration,
    pub max_token_ttl: Duration,
    /// One permit per allowed concurrent websocket.
    pub limit: Arc<Semaphore>,
    /// Connections refused because the cap was reached.
    pub rejected: Arc<AtomicU64>,
    /// Allowed `Origin`s for cookie auth (`scheme://host[:port]`); empty means "same as `Host`".
    pub allowed_origins: Arc<Vec<String>>,
    /// Successful authentications that used the deprecated `?token=` parameter.
    pub deprecated_query: Arc<AtomicU64>,
    /// Flips to `true` when the server begins shutting down.
    pub shutdown: watch::Receiver<bool>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/live/v1/stream", get(stream))
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .with_state(state)
}

async fn readyz(State(s): State<AppState>) -> Response {
    if s.hub.is_ready() {
        "ok".into_response()
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "listener not connected").into_response()
    }
}

#[derive(Deserialize)]
struct StreamQuery {
    token: Option<String>,
}

/// Name of the cookie carrying the stream JWT.
pub const COOKIE_NAME: &str = "enzarb_live";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Bearer,
    Cookie,
    Query,
}

impl Source {
    /// Static label for logs; never a credential.
    fn label(self) -> &'static str {
        match self {
            Source::Bearer => "bearer",
            Source::Cookie => "cookie",
            Source::Query => "query",
        }
    }
}

/// `Bearer <non-empty>` with a case-insensitive scheme.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = v.split_once(' ')?;
    let token = rest.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

/// The first `enzarb_live` value over all `Cookie` headers, verbatim (no decoding).
fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE_NAME).then(|| value.to_string())
        })
}

/// Picks the first source that is *present*. The token is `None` when that source is present but
/// unusable (malformed `Authorization`), which must be a 401 rather than a fall-through.
fn token_source(headers: &HeaderMap, query: Option<String>) -> Option<(Source, Option<String>)> {
    if headers.contains_key(AUTHORIZATION) {
        return Some((Source::Bearer, bearer_token(headers)));
    }
    if let Some(c) = cookie_value(headers) {
        return Some((Source::Cookie, Some(c)));
    }
    query.map(|q| (Source::Query, Some(q)))
}

/// A parsed `scheme://host[:port]`; scheme and host lowercased, port exactly as written.
#[derive(Debug, PartialEq, Eq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: Option<String>,
}

/// Parses `host[:port]` (no userinfo, path, query, fragment, whitespace).
fn parse_authority(a: &str) -> Option<(String, Option<String>)> {
    let (host, port) = if a.starts_with('[') {
        let end = a.find(']')?;
        let rest = &a[end + 1..];
        let port = if rest.is_empty() {
            None
        } else {
            Some(rest.strip_prefix(':')?)
        };
        (&a[..=end], port)
    } else {
        match a.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (a, None),
        }
    };
    let chars_ok = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']' | ':'));
    if !chars_ok || host == "[]" {
        return None;
    }
    if let Some(p) = port
        && (p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some((host.to_ascii_lowercase(), port.map(str::to_string)))
}

/// Strict `http(s)://host[:port]`; `null`, paths (even `/`), userinfo, etc. are rejected.
pub fn parse_origin(o: &str) -> Option<Origin> {
    let (scheme, rest) = o.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let (host, port) = parse_authority(rest)?;
    Some(Origin { scheme, host, port })
}

/// Whether `origin` may use cookie auth. With an allowlist: exact scheme, host and port match.
/// Without: its authority must equal the `Host` header (the only header consulted).
pub fn origin_allowed(origin: &str, host: Option<&str>, allowlist: &[String]) -> bool {
    let Some(o) = parse_origin(origin) else {
        return false;
    };
    if !allowlist.is_empty() {
        return allowlist
            .iter()
            .filter_map(|a| parse_origin(a))
            .any(|a| a == o);
    }
    host.and_then(parse_authority)
        .is_some_and(|(h, p)| h == o.host && p == o.port)
}

async fn stream(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<StreamQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some((source, token)) = token_source(&headers, q.token) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // CSWSH: the browser attaches the cookie on its own, so the page's origin must be vetted
    // before the token is even looked at.
    if source == Source::Cookie {
        let origin = headers.get(ORIGIN).and_then(|v| v.to_str().ok());
        let host = headers.get(HOST).and_then(|v| v.to_str().ok());
        if !origin.is_some_and(|o| origin_allowed(o, host, &s.allowed_origins)) {
            tracing::info!(source = source.label(), "stream origin rejected");
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let Some(token) = token else {
        tracing::info!(
            source = source.label(),
            "stream auth rejected: malformed credentials"
        );
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let claims = match auth::validate(&token, &s.secret, s.max_token_ttl) {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(error = ?e, source = source.label(), "stream auth rejected");
            return StatusCode::UNAUTHORIZED.into_response();
        }
    };
    if source == Source::Query {
        let n = s.deprecated_query.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(deprecated_total = n, sub = %claims.sub, "token in query string is deprecated");
    }
    let Ok(permit) = s.limit.clone().try_acquire_owned() else {
        let total = s.rejected.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(sub = %claims.sub, rejected_total = total, "connection cap reached; rejecting");
        return (StatusCode::SERVICE_UNAVAILABLE, [(RETRY_AFTER, "1")]).into_response();
    };
    // Subscribe before the upgrade completes so nothing published in between is lost.
    let rx = s.hub.subscribe();
    let deadline = Instant::now() + auth::lifetime(&claims, s.max_token_ttl);
    let shutdown = s.shutdown.clone();
    ws.max_message_size(1024)
        .max_frame_size(1024)
        .on_upgrade(move |sock| {
            client(
                sock,
                rx,
                claims,
                deadline,
                s.ping_interval,
                shutdown,
                permit,
            )
        })
}

async fn close(sock: &mut WebSocket, code: u16, reason: &'static str) {
    let _ = send(
        sock,
        Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })),
    )
    .await;
}

async fn send(sock: &mut WebSocket, m: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, sock.send(m)).await,
        Ok(Ok(()))
    )
}

async fn send_frame(sock: &mut WebSocket, f: &Frame) -> bool {
    let json = serde_json::to_string(f).expect("frame serializes");
    send(sock, Message::Text(json.into())).await
}

async fn client(
    mut sock: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<Frame>,
    claims: Claims,
    deadline: Instant,
    ping_every: Duration,
    mut shutdown: watch::Receiver<bool>,
    _permit: OwnedSemaphorePermit, // released when the task ends
) {
    tracing::info!(sub = %claims.sub, repos = claims.repos.len(), "stream open");
    let mut ping = tokio::time::interval_at(Instant::now() + ping_every, ping_every);
    let pong_window = ping_every * 3 / 2;
    let pong_deadline = tokio::time::sleep_until(Instant::now() + pong_window);
    tokio::pin!(pong_deadline);
    let expiry = tokio::time::sleep_until(deadline);
    tokio::pin!(expiry);
    // If the sender is dropped without signalling, stop watching rather than spin.
    let mut watching = true;
    loop {
        tokio::select! {
            _ = &mut expiry => {
                close(&mut sock, CLOSE_TOKEN_EXPIRED, "token expired; reconnect with a fresh token").await;
                break;
            }
            r = async { shutdown.wait_for(|v| *v).await.is_ok() }, if watching => {
                if r {
                    close(&mut sock, CLOSE_GOING_AWAY, "server shutting down").await;
                    break;
                }
                watching = false;
            }
            _ = &mut pong_deadline => {
                tracing::info!(sub = %claims.sub, "no pong; dropping client");
                close(&mut sock, CLOSE_GOING_AWAY, "ping timeout").await;
                break;
            }
            _ = ping.tick() => {
                if !send(&mut sock, Message::Ping(Vec::new().into())).await { break; }
            }
            incoming = sock.recv() => match incoming {
                Some(Ok(Message::Pong(_))) => pong_deadline.as_mut().reset(Instant::now() + pong_window),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {} // client messages are ignored
            },
            f = rx.recv() => {
                let ok = match f {
                    Ok(f) if f.visible_to(&claims.repos) => send_frame(&mut sock, &f).await,
                    Ok(_) => true,
                    Err(RecvError::Lagged(n)) => {
                        tracing::warn!(sub = %claims.sub, skipped = n, "client lagged; resync");
                        send_frame(&mut sock, &Frame::Resync).await
                    }
                    Err(RecvError::Closed) => {
                        close(&mut sock, CLOSE_INTERNAL, "internal error").await;
                        break;
                    }
                };
                if !ok { break; }
            }
        }
    }
    tracing::info!(sub = %claims.sub, "stream closed");
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn hm(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn cookie_parser() {
        let c = |v: &str| cookie_value(&hm(&[("cookie", v)]));
        assert_eq!(c("a=1; enzarb_live=tok; b=2").as_deref(), Some("tok"));
        assert_eq!(c("a=1; b=2"), None);
        assert_eq!(c("enzarb_live_x=nope; xenzarb_live=nope"), None);
        assert_eq!(c("enzarb_live=a%20b").as_deref(), Some("a%20b"));
        assert_eq!(
            c("enzarb_live=one; enzarb_live=two").as_deref(),
            Some("one")
        );
        let two = hm(&[("cookie", "enzarb_live=one"), ("cookie", "enzarb_live=two")]);
        assert_eq!(cookie_value(&two).as_deref(), Some("one"));
    }

    #[test]
    fn authorization_present_is_bearer_source() {
        let src = |v: &str| {
            token_source(
                &hm(&[("authorization", v), ("cookie", "enzarb_live=c")]),
                Some("q".into()),
            )
        };
        assert_eq!(
            src("Bearer abc"),
            Some((Source::Bearer, Some("abc".into())))
        );
        assert_eq!(
            src("bEaReR abc"),
            Some((Source::Bearer, Some("abc".into())))
        );
        assert_eq!(src("Basic x"), Some((Source::Bearer, None)));
        assert_eq!(src("Bearer "), Some((Source::Bearer, None)));
        assert_eq!(src("Bearer"), Some((Source::Bearer, None)));
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_bytes(b"Bearer \xff").unwrap(),
        );
        assert_eq!(
            token_source(&h, Some("q".into())),
            Some((Source::Bearer, None))
        );
    }

    #[test]
    fn source_precedence() {
        let h = hm(&[("cookie", "enzarb_live=c")]);
        assert_eq!(
            token_source(&h, Some("q".into())),
            Some((Source::Cookie, Some("c".into())))
        );
        assert_eq!(
            token_source(&HeaderMap::new(), Some("q".into())),
            Some((Source::Query, Some("q".into())))
        );
        assert_eq!(token_source(&HeaderMap::new(), None), None);
    }

    fn allowed(origin: &str, host: &str) -> bool {
        origin_allowed(origin, Some(host), &[])
    }

    #[test]
    fn host_default_origin_rules() {
        assert!(allowed("https://enzarb.dev", "enzarb.dev"));
        assert!(allowed("HTTPS://Enzarb.DEV", "enzarb.dev"));
        assert!(allowed("http://localhost:8080", "localhost:8080"));
        assert!(!allowed("null", "enzarb.dev"));
        assert!(!allowed("https://enzarb.dev:8443", "enzarb.dev"));
        assert!(!allowed("https://enzarb.dev", "enzarb.dev:8443"));
        assert!(!allowed("https://h", "h:443"));
        assert!(!allowed("https://h:443", "h"));
        assert!(!allowed("https://enzarb.dev.evil.com", "enzarb.dev"));
        assert!(!allowed("https://evil-enzarb.dev", "enzarb.dev"));
        assert!(!allowed("https://enzarb", "enzarb.dev"));
        for bad in [
            "https://enzarb.dev/",
            "https://enzarb.dev/x",
            "https://u:p@enzarb.dev",
            "https://u@enzarb.dev",
            "https://enzarb.dev?x=1",
            "https://enzarb.dev#f",
            "https://enzarb.dev ",
            "https://enzarb.dev junk",
            "ftp://enzarb.dev",
            "enzarb.dev",
            "https://",
            "https://enzarb.dev:",
        ] {
            assert!(!allowed(bad, "enzarb.dev"), "{bad}");
        }
        assert!(!origin_allowed("https://enzarb.dev", None, &[]));
    }

    #[test]
    fn allowlist_replaces_host_default() {
        let list = vec!["https://enzarb.dev".to_string()];
        assert!(origin_allowed(
            "https://enzarb.dev",
            Some("internal"),
            &list
        ));
        assert!(origin_allowed(
            "HTTPS://ENZARB.dev",
            Some("internal"),
            &list
        ));
        assert!(!origin_allowed(
            "http://enzarb.dev",
            Some("enzarb.dev"),
            &list
        ));
        assert!(!origin_allowed("https://internal", Some("internal"), &list));
        assert!(!origin_allowed(
            "https://enzarb.dev:443",
            Some("enzarb.dev"),
            &list
        ));
    }
}
