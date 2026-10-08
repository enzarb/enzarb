mod conditions;
mod desired;
mod reconcile;

use clap::Parser;
use desired::Defaults;
use enzarb_crds::Project;
use enzarb_crds::upstream::sandbox::Sandbox;
use futures::StreamExt;
use kube::runtime::{Controller, watcher};
use kube::{Api, Client};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, info};

/// Reconciles enzarb `Project`s into a Capsule Tenant, Namespace, PVC and agent-sandbox Sandbox.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Sandbox image used when a Project does not set `spec.sandbox.image`.
    #[arg(long, env = "ENZARB_SANDBOX_IMAGE", default_value = "")]
    sandbox_image: String,
    /// Workspace PVC size used when a Project sets an empty `spec.sandbox.storage`.
    #[arg(long, env = "ENZARB_DEFAULT_STORAGE", default_value = "20Gi")]
    default_storage: String,
    /// Address for /healthz and /readyz.
    #[arg(long, env = "ENZARB_HEALTH_ADDR", default_value = "0.0.0.0:8080")]
    health_addr: SocketAddr,
}

async fn serve_health(addr: SocketAddr, ready: Arc<AtomicBool>) -> anyhow::Result<()> {
    use axum::{Router, http::StatusCode, routing::get};
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(move || {
                let ready = ready.clone();
                async move {
                    if ready.load(Ordering::Relaxed) {
                        (StatusCode::OK, "ok")
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, "starting")
                    }
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let ready = Arc::new(AtomicBool::new(false));
    tokio::spawn({
        let ready = ready.clone();
        async move {
            if let Err(e) = serve_health(args.health_addr, ready).await {
                error!(error = %e, "health server failed");
            }
        }
    });

    let client = Client::try_default().await?;
    let projects: Api<Project> = Api::all(client.clone());
    let sandboxes: Api<Sandbox> = Api::all(client.clone());
    let ctx = Arc::new(reconcile::Ctx::new(
        client,
        Defaults {
            sandbox_image: args.sandbox_image,
            storage: args.default_storage,
        },
    ));

    ready.store(true, Ordering::Relaxed);
    info!("starting controller");
    Controller::new(projects, watcher::Config::default())
        .owns(sandboxes, watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile::reconcile, reconcile::error_policy, ctx)
        .for_each(|res| async move {
            match res {
                Ok((obj, _)) => tracing::debug!(project = %obj.name, "reconciled"),
                Err(e) => tracing::debug!(error = %e, "controller event error"),
            }
        })
        .await;
    info!("shut down");
    Ok(())
}
