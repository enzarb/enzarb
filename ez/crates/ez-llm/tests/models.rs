//! `list_models` against a local fake server that serves scripted responses in order.

use ez_llm::{ApiKind, list_models};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Seen = Vec<(String, Vec<(String, String)>)>;

/// Serve each `(status, body)` to successive connections; returns the base URL and the
/// (path, headers) of every request seen once the script is exhausted.
async fn serve_script(
    script: Vec<(&'static str, String)>,
) -> (String, tokio::task::JoinHandle<Seen>) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for k in ["NO_PROXY", "no_proxy"] {
            // SAFETY: test-only; set before or while no other thread reads this variable.
            unsafe {
                std::env::set_var(k, "127.0.0.1,localhost");
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let h = tokio::spawn(async move {
        let mut seen = vec![];
        for (status, body) in script {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = sock.read(&mut tmp).await.unwrap();
                assert!(n > 0);
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf).to_string();
            let mut lines = head.lines();
            let path = lines
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_string();
            let headers = lines
                .filter_map(|l| l.split_once(':'))
                .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                .collect();
            seen.push((path, headers));
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            let _ = sock.shutdown().await;
        }
        seen
    });
    (url, h)
}

fn header<'a>(h: &'a [(String, String)], k: &str) -> Option<&'a str> {
    h.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

#[tokio::test]
async fn anthropic_follows_pagination_and_keeps_order() {
    let p1 = r#"{"data":[{"id":"claude-b","display_name":"B"},{"id":"claude-a","display_name":"A"}],"has_more":true,"last_id":"claude-a"}"#;
    let p2 = r#"{"data":[{"id":"claude-0"}],"has_more":false,"last_id":"claude-0"}"#;
    let (url, h) = serve_script(vec![("200 OK", p1.into()), ("200 OK", p2.into())]).await;
    let m = list_models(ApiKind::Anthropic, &format!("{url}/"), Some("sk-test"))
        .await
        .unwrap();
    let ids: Vec<_> = m.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["claude-b", "claude-a", "claude-0"]);
    assert_eq!(m[0].display_name.as_deref(), Some("B"));
    assert_eq!(m[2].display_name, None);
    let seen = h.await.unwrap();
    assert_eq!(seen[0].0, "/v1/models?limit=1000");
    assert_eq!(seen[1].0, "/v1/models?limit=1000&after_id=claude-a");
    assert_eq!(header(&seen[0].1, "x-api-key"), Some("sk-test"));
    assert_eq!(header(&seen[0].1, "anthropic-version"), Some("2023-06-01"));
}

#[tokio::test]
async fn openai_sorts_alphabetically_and_sends_bearer() {
    let body =
        r#"{"object":"list","data":[{"id":"zeta"},{"id":"alpha"},{"id":"mid"},{"id":"alpha"}]}"#;
    let (url, h) = serve_script(vec![("200 OK", body.into())]).await;
    let m = list_models(ApiKind::Openai, &format!("{url}/v1"), Some("k"))
        .await
        .unwrap();
    let ids: Vec<_> = m.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["alpha", "mid", "zeta"]);
    let seen = h.await.unwrap();
    assert_eq!(seen[0].0, "/v1/models");
    assert_eq!(header(&seen[0].1, "authorization"), Some("Bearer k"));
}

#[tokio::test]
async fn openai_without_key_sends_no_auth() {
    let (url, h) = serve_script(vec![("200 OK", r#"{"data":[]}"#.into())]).await;
    let m = list_models(ApiKind::Openai, &url, None).await.unwrap();
    assert!(m.is_empty());
    assert!(header(&h.await.unwrap()[0].1, "authorization").is_none());
}

#[tokio::test]
async fn error_bodies_are_surfaced() {
    let (url, _h) = serve_script(vec![(
        "401 Unauthorized",
        r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#
            .into(),
    )])
    .await;
    let e = list_models(ApiKind::Anthropic, &url, Some("bad"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("401") && e.contains("invalid x-api-key"), "{e}");

    let (url, _h) = serve_script(vec![("500 Internal Server Error", "boom".into())]).await;
    let e = list_models(ApiKind::Openai, &url, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("500") && e.contains("boom"), "{e}");

    let (url, _h) = serve_script(vec![("200 OK", "<html>".into())]).await;
    assert!(list_models(ApiKind::Openai, &url, None).await.is_err());
}

#[tokio::test]
async fn connection_refused_is_an_error() {
    // A port that was just bound and released is refused (port 1 may be dropped by a firewall).
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    assert!(list_models(ApiKind::Openai, &closed, None).await.is_err());
}
