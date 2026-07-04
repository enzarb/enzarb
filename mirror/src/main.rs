mod cache;
mod proxy;
mod upstream;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use axum::routing::get;
use tower_http::trace::TraceLayer;

use cache::Cache;
use proxy::AppState;
use upstream::Upstream;

/// Parse a human-readable byte size: plain bytes, or Ki/Mi/Gi/Ti suffixes
/// (with or without a trailing "B", e.g. "50Gi", "512MiB", "1073741824").
fn parse_bytes(s: &str) -> Result<u64> {
    let s = s.trim().trim_end_matches('B').trim_end_matches('b');
    let (num, mult) = match s {
        _ if s.ends_with("Ki") => (&s[..s.len() - 2], 1u64 << 10),
        _ if s.ends_with("Mi") => (&s[..s.len() - 2], 1u64 << 20),
        _ if s.ends_with("Gi") => (&s[..s.len() - 2], 1u64 << 30),
        _ if s.ends_with("Ti") => (&s[..s.len() - 2], 1u64 << 40),
        _ => (s, 1),
    };
    let n: f64 = num
        .trim()
        .parse()
        .with_context(|| format!("invalid size {s:?}"))?;
    Ok((n * mult as f64) as u64)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cache_dir = env_or("MIRROR_CACHE_DIR", "/var/lib/mirror");
    let max_bytes = parse_bytes(&env_or("MIRROR_MAX_BYTES", "10Gi"))?;
    let tag_ttl_secs: u64 = env_or("MIRROR_TAG_TTL", "60")
        .trim_end_matches('s')
        .parse()?;
    let flush_secs: u64 = env_or("MIRROR_INDEX_FLUSH", "30")
        .trim_end_matches('s')
        .parse()?;
    let listen = env_or("MIRROR_LISTEN", "0.0.0.0:5000");
    let upstreams: Vec<String> = env_or(
        "MIRROR_UPSTREAMS",
        "registry-1.docker.io,ghcr.io,quay.io,registry.k8s.io",
    )
    .split(',')
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect();

    let cache = Arc::new(Cache::load(&cache_dir, max_bytes)?);
    tracing::info!(cache_dir, max_bytes, ?upstreams, "mirror starting");

    let state = Arc::new(AppState {
        cache: cache.clone(),
        upstream: Upstream::new(),
        upstreams,
        tag_ttl_secs,
    });

    // Periodically persist the access index so LRU history survives restarts.
    let flush_cache = cache.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(flush_secs.max(1)));
        loop {
            tick.tick().await;
            if let Err(err) = flush_cache.flush() {
                tracing::warn!(%err, "index flush failed");
            }
        }
    });

    let app = Router::new()
        .route("/v2/", get(proxy::ping))
        .route("/v2/{*path}", get(proxy::v2).head(proxy::v2))
        .route("/healthz", get(proxy::healthz))
        .route("/stats", get(proxy::stats))
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    tracing::info!(%listen, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    cache.flush()?;
    Ok(())
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = term.recv() => {},
    }
    tracing::info!("shutting down; flushing index");
}

#[cfg(test)]
mod tests {
    use super::parse_bytes;

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_bytes("50Gi").unwrap(), 50 << 30);
        assert_eq!(parse_bytes("512MiB").unwrap(), 512 << 20);
        assert_eq!(parse_bytes("1.5Ki").unwrap(), 1536);
        assert!(parse_bytes("nope").is_err());
    }
}
