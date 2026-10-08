mod conditions;
mod desired;
mod leader;
#[cfg(test)]
mod rbac;
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
use std::time::Duration;
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
    /// Namespace enzarb is released into (where the sandbox clients run). Defaults to the
    /// pod's own namespace.
    #[arg(long, env = "ENZARB_RELEASE_NAMESPACE")]
    release_namespace: Option<String>,
    /// Namespace label `key=value` allowed to reach sandboxes (NetworkPolicy ingress).
    /// Defaults to `kubernetes.io/metadata.name=<release namespace>`.
    #[arg(long, env = "ENZARB_NETPOL_INGRESS_NAMESPACE_LABEL")]
    netpol_ingress_namespace_label: Option<String>,
    /// Comma-separated `app.kubernetes.io/name` values of pods allowed to reach sandboxes.
    #[arg(
        long,
        env = "ENZARB_NETPOL_INGRESS_POD_NAMES",
        value_delimiter = ',',
        default_value = "enzarb-sandbox-proxy,enzarb-workflows"
    )]
    netpol_ingress_pod_names: Vec<String>,
    /// Address for /healthz and /readyz.
    #[arg(long, env = "ENZARB_HEALTH_ADDR", default_value = "0.0.0.0:8080")]
    health_addr: SocketAddr,
    /// Only the replica holding the Lease runs the controller. Disable for a single replica
    /// outside a cluster (e.g. local development).
    #[arg(
        long,
        env = "ENZARB_LEADER_ELECTION",
        default_value_t = true,
        action = clap::ArgAction::Set,
        value_name = "BOOL"
    )]
    leader_election: bool,
    /// Name of the coordination Lease.
    #[arg(
        long,
        env = "ENZARB_LEADER_ELECTION_NAME",
        default_value = "enzarb-sandbox-operator"
    )]
    leader_election_name: String,
    /// Namespace of the Lease. Defaults to the pod's own namespace.
    #[arg(long, env = "ENZARB_LEADER_ELECTION_NAMESPACE")]
    leader_election_namespace: Option<String>,
    /// Identity written to the Lease. Defaults to $POD_NAME, then the hostname.
    #[arg(long, env = "POD_NAME")]
    identity: Option<String>,
    /// Seconds a Lease stays valid without renewal.
    #[arg(long, env = "ENZARB_LEASE_DURATION_SECS", default_value_t = 15)]
    lease_duration_secs: u64,
    /// Seconds between renewals (and between acquire attempts for standbys).
    #[arg(long, env = "ENZARB_LEASE_RENEW_SECS", default_value_t = 5)]
    lease_renew_secs: u64,
}

fn pod_namespace() -> std::io::Result<String> {
    std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
        .map(|s| s.trim().to_string())
}

fn netpol_config(args: &Args) -> anyhow::Result<desired::NetpolConfig> {
    let release = match &args.release_namespace {
        Some(ns) => ns.clone(),
        None => pod_namespace().map_err(|e| {
            anyhow::anyhow!("cannot determine release namespace (set --release-namespace): {e}")
        })?,
    };
    let mut cfg = desired::NetpolConfig::for_release_namespace(&release);
    if let Some(l) = &args.netpol_ingress_namespace_label {
        let (k, v) = l
            .split_once('=')
            .filter(|(k, _)| !k.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("--netpol-ingress-namespace-label must be key=value, got {l:?}")
            })?;
        cfg.ingress_namespace_label = (k.into(), v.into());
    }
    let names: Vec<String> = args
        .netpol_ingress_pod_names
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    anyhow::ensure!(
        !names.is_empty(),
        "--netpol-ingress-pod-names must not be empty"
    );
    cfg.ingress_pod_names = names;
    Ok(cfg)
}

fn leader_config(args: &Args) -> anyhow::Result<leader::Config> {
    let namespace = match &args.leader_election_namespace {
        Some(ns) => ns.clone(),
        None => pod_namespace().map_err(|e| {
            anyhow::anyhow!(
                "cannot determine lease namespace (set --leader-election-namespace): {e}"
            )
        })?,
    };
    let base = args
        .identity
        .clone()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "enzarb-sandbox-operator".into());
    // The suffix keeps a restarted pod with the same name from inheriting a stale lease.
    let suffix: u32 = rand::random();
    anyhow::ensure!(
        args.lease_renew_secs > 0 && args.lease_duration_secs > args.lease_renew_secs,
        "lease duration must exceed the renew interval"
    );
    Ok(leader::Config {
        namespace,
        name: args.leader_election_name.clone(),
        identity: format!("{base}_{suffix:08x}"),
        lease_duration: Duration::from_secs(args.lease_duration_secs),
        renew_interval: Duration::from_secs(args.lease_renew_secs),
        renew_deadline: Duration::from_secs(args.lease_duration_secs - args.lease_renew_secs),
    })
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
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
    let health_addr = args.health_addr;
    tokio::spawn({
        let ready = ready.clone();
        async move {
            if let Err(e) = serve_health(health_addr, ready).await {
                error!(error = %e, "health server failed");
            }
        }
    });

    let netpol = netpol_config(&args)?;
    let client = Client::try_default().await?;
    let elector = if args.leader_election {
        Some(leader::Elector::new(client.clone(), leader_config(&args)?))
    } else {
        info!("leader election disabled");
        None
    };
    // Standbys are healthy and ready: they just have nothing to do until they win the Lease.
    ready.store(true, Ordering::Relaxed);

    let Some(elector) = elector else {
        run_controller(client, &args, netpol.clone()).await;
        return Ok(());
    };
    tokio::select! {
        _ = elector.acquire() => {}
        _ = shutdown_signal() => return Ok(()),
    }
    let lost = tokio::select! {
        _ = run_controller(client, &args, netpol.clone()) => false,
        _ = elector.hold() => true,
    };
    elector.release().await;
    if lost {
        anyhow::bail!("lost leadership; exiting so a fresh process can rejoin as standby");
    }
    Ok(())
}

async fn run_controller(client: Client, args: &Args, netpol: desired::NetpolConfig) {
    let projects: Api<Project> = Api::all(client.clone());
    let sandboxes: Api<Sandbox> = Api::all(client.clone());
    let ctx = Arc::new(reconcile::Ctx::new(
        client,
        Defaults {
            sandbox_image: args.sandbox_image.clone(),
            storage: args.default_storage.clone(),
            netpol,
        },
    ));
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
}
