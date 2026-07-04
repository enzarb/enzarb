use std::io::Write;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio_util::io::ReaderStream;

use crate::cache::Cache;
use crate::upstream::Upstream;

pub struct AppState {
    pub cache: Arc<Cache>,
    pub upstream: Upstream,
    pub upstreams: Vec<String>,
    pub tag_ttl_secs: u64,
}

/// A parsed mirror request path:
/// `<upstream-host>/<repo…>/(manifests|blobs)/<ref>`.
struct MirrorRef {
    host: String,
    repo: String,
    kind: Kind,
    reference: String,
}

#[derive(PartialEq)]
enum Kind {
    Manifests,
    Blobs,
}

fn parse_path(path: &str, allowed: &[String]) -> Option<MirrorRef> {
    let (host, rest) = path.split_once('/')?;
    if !allowed.iter().any(|a| a == host) {
        return None;
    }
    let parts: Vec<&str> = rest.split('/').collect();
    // <repo…>/<kind>/<reference>; repo has at least one segment.
    if parts.len() < 3 {
        return None;
    }
    let kind = match parts[parts.len() - 2] {
        "manifests" => Kind::Manifests,
        "blobs" => Kind::Blobs,
        _ => return None,
    };
    Some(MirrorRef {
        host: host.to_string(),
        repo: parts[..parts.len() - 2].join("/"),
        kind,
        reference: parts[parts.len() - 1].to_string(),
    })
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub async fn ping() -> impl IntoResponse {
    StatusCode::OK
}

pub async fn healthz() -> impl IntoResponse {
    StatusCode::OK
}

pub async fn stats(State(st): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(st.cache.stats())
}

pub async fn v2(
    State(st): State<Arc<AppState>>,
    method: Method,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(mref) = parse_path(&path, &st.upstreams) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let head = method == Method::HEAD;
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|a| a.to_str().ok())
        .map(String::from);

    let result = match mref.kind {
        Kind::Blobs => serve_immutable(&st, &mref, format!("blob:{}", mref.reference), head).await,
        Kind::Manifests if mref.reference.starts_with("sha256:") => {
            serve_immutable(&st, &mref, format!("manifest:{}", mref.reference), head).await
        }
        Kind::Manifests => serve_tag(&st, &mref, accept.as_deref(), head).await,
    };
    match result {
        Ok(resp) => resp,
        Err(err) => {
            // Any failure is a 502: buildkit falls back to the real upstream.
            tracing::warn!(path, %err, "mirror fetch failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Serve a content-addressed object (blob or manifest-by-digest): cached
/// forever, fetched once.
async fn serve_immutable(
    st: &AppState,
    mref: &MirrorRef,
    key: String,
    head: bool,
) -> anyhow::Result<Response> {
    if let Some(entry) = st.cache.lookup(&key) {
        return respond_file(
            st,
            &key,
            &entry.content_type,
            &entry.digest,
            entry.size,
            head,
        )
        .await;
    }
    let kind = if mref.kind == Kind::Blobs {
        "blobs"
    } else {
        "manifests"
    };
    let upstream_path = format!("{}/{}/{}", mref.repo, kind, mref.reference);
    // Manifests need broad Accept or registries fall back to schema1.
    let accept = (kind == "manifests").then_some(MANIFEST_ACCEPT);
    let resp = st
        .upstream
        .request(
            reqwest::Method::GET,
            &mref.host,
            &mref.repo,
            &upstream_path,
            accept,
        )
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("upstream returned {}", resp.status());
    }
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let expected = mref
        .reference
        .starts_with("sha256:")
        .then_some(mref.reference.as_str());
    let (temp, digest, size) = download_verified(st, resp, expected).await?;
    st.cache.commit(&key, &temp, &content_type, &digest)?;
    respond_file(st, &key, &content_type, &digest, size, head).await
}

const MANIFEST_ACCEPT: &str = "application/vnd.oci.image.manifest.v1+json, \
     application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json, \
     application/vnd.docker.distribution.manifest.list.v2+json";

/// Serve a manifest by tag: cached with a TTL, revalidated by digest via HEAD.
async fn serve_tag(
    st: &AppState,
    mref: &MirrorRef,
    accept: Option<&str>,
    head: bool,
) -> anyhow::Result<Response> {
    let key = format!("tag:{}/{}:{}", mref.host, mref.repo, mref.reference);
    let upstream_path = format!("{}/manifests/{}", mref.repo, mref.reference);
    let accept = accept.unwrap_or(MANIFEST_ACCEPT);

    if let Some(entry) = st.cache.lookup(&key) {
        if now().saturating_sub(entry.fetched_at) <= st.tag_ttl_secs {
            return respond_file(
                st,
                &key,
                &entry.content_type,
                &entry.digest,
                entry.size,
                head,
            )
            .await;
        }
        // Stale: revalidate by digest with a HEAD to upstream.
        if let Ok(resp) = st
            .upstream
            .request(
                reqwest::Method::HEAD,
                &mref.host,
                &mref.repo,
                &upstream_path,
                Some(accept),
            )
            .await
            && resp.status().is_success()
            && resp
                .headers()
                .get("docker-content-digest")
                .and_then(|d| d.to_str().ok())
                == Some(entry.digest.as_str())
        {
            st.cache.refresh(&key);
            return respond_file(
                st,
                &key,
                &entry.content_type,
                &entry.digest,
                entry.size,
                head,
            )
            .await;
        }
        st.cache.remove(&key);
    }

    let resp = st
        .upstream
        .request(
            reqwest::Method::GET,
            &mref.host,
            &mref.repo,
            &upstream_path,
            Some(accept),
        )
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("upstream returned {}", resp.status());
    }
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|c| c.to_str().ok())
        .unwrap_or("application/vnd.oci.image.manifest.v1+json")
        .to_string();
    let upstream_digest = resp
        .headers()
        .get("docker-content-digest")
        .and_then(|d| d.to_str().ok())
        .map(String::from);

    let (temp, digest, size) = download_verified(st, resp, upstream_digest.as_deref()).await?;
    st.cache.commit(&key, &temp, &content_type, &digest)?;
    respond_file(st, &key, &content_type, &digest, size, head).await
}

/// Stream an upstream response body to a temp file, hashing as we go, and
/// verify against the expected digest when one is known. Returns the temp
/// path (ready for Cache::commit), the computed digest, and the size.
async fn download_verified(
    st: &AppState,
    resp: reqwest::Response,
    expected_digest: Option<&str>,
) -> anyhow::Result<(std::path::PathBuf, String, u64)> {
    let temp = st.cache.temp_path();
    let mut file = std::fs::File::create(&temp)?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk)?;
        size += chunk.len() as u64;
    }
    file.sync_all()?;
    drop(file);

    let digest = format!("sha256:{}", hex::encode(hasher.finalize()));
    if let Some(want) = expected_digest
        && want.starts_with("sha256:")
        && want != digest
    {
        let _ = std::fs::remove_file(&temp);
        anyhow::bail!("digest mismatch: expected {want}, got {digest}");
    }
    Ok((temp, digest, size))
}

/// Serve a cached file with distribution-spec headers.
async fn respond_file(
    st: &AppState,
    key: &str,
    content_type: &str,
    digest: &str,
    size: u64,
    head: bool,
) -> anyhow::Result<Response> {
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, size);
    if !digest.is_empty() {
        builder = builder.header("Docker-Content-Digest", digest);
    }
    if head {
        return Ok(builder.body(Body::empty())?);
    }
    let file = tokio::fs::File::open(st.cache.data_path(key)).await?;
    Ok(builder.body(Body::from_stream(ReaderStream::new(file)))?)
}
