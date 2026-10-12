mod conditions;
mod desired;
mod docker;
mod leader;
mod quantity;
#[cfg(test)]
mod rbac;
mod reconcile;
mod roll;

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

/// Reconciles enzarb `Project`s into a Secret, PVCs, NetworkPolicy and agent-sandbox Sandbox in
/// one shared sandbox namespace (ADR-0013).
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// Sandbox image used when a Project does not set `spec.sandbox.image`.
    #[arg(long, env = "ENZARB_SANDBOX_IMAGE", default_value = "")]
    sandbox_image: String,
    /// How long a pending pod-shape change (image, env, resources, ...) waits for the drain ack
    /// (`enzarb.dev/drained-generation`) or idleness before the pod is deleted anyway. Humantime-style: `30s`, `10m`.
    #[arg(
        long,
        env = "ENZARB_SANDBOX_ROLL_DRAIN_TIMEOUT",
        default_value = "2h",
        value_parser = parse_duration
    )]
    sandbox_roll_drain_timeout: Duration,
    /// Workspace PVC size used when a Project sets an empty `spec.sandbox.storage`.
    #[arg(long, env = "ENZARB_DEFAULT_STORAGE", default_value = "20Gi")]
    default_storage: String,
    /// Namespace enzarb is released into (where the sandbox clients run). Defaults to the
    /// pod's own namespace.
    #[arg(long, env = "ENZARB_RELEASE_NAMESPACE")]
    release_namespace: Option<String>,
    /// Namespace that holds every Project's sandbox objects (created by the Helm chart).
    #[arg(
        long,
        env = "ENZARB_SANDBOX_NAMESPACE",
        default_value = "enzarb-sandboxes"
    )]
    sandbox_namespace: String,
    /// ServiceAccount of every sandbox pod. It has no token and no RBAC (no Kubernetes API access).
    #[arg(
        long,
        env = "ENZARB_SANDBOX_SERVICE_ACCOUNT",
        default_value = "enzarb-sandbox"
    )]
    sandbox_service_account: String,
    /// TCP port of the workflows executor callback listener; sandboxes may reach it (egress).
    #[arg(long, env = "ENZARB_EXECUTOR_CALLBACK_PORT", default_value_t = 8081)]
    executor_callback_port: u16,
    /// Comma-separated CIDRs sandboxes must not reach on TCP 443: the cluster's pod, service
    /// and node networks (they contain the API server). Required.
    #[arg(
        long,
        env = "ENZARB_SANDBOX_EGRESS_EXCLUDE_CIDRS",
        value_delimiter = ','
    )]
    egress_exclude_cidrs: Vec<String>,
    /// Comma-separated extra DNS server CIDRs sandboxes may reach on port 53, e.g.
    /// `169.254.20.10/32` for NodeLocal DNSCache. kube-dns pods are always allowed.
    #[arg(long, env = "ENZARB_SANDBOX_DNS_CIDRS", value_delimiter = ',')]
    dns_cidrs: Vec<String>,
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
    /// RuntimeClass of every sandbox pod (required; recommended `gvisor`). Must be in
    /// `--docker-runtime-classes`, because every sandbox runs dockerd.
    #[arg(long, env = "ENZARB_SANDBOX_RUNTIME_CLASS", default_value = "")]
    sandbox_runtime_class: String,
    /// Image of the `dockerd` sidecar every sandbox gets.
    #[arg(long, env = "ENZARB_DOCKER_IMAGE", default_value = docker::DEFAULT_IMAGE)]
    docker_image: String,
    /// Comma-separated RuntimeClass names allowed to host dockerd.
    #[arg(
        long,
        env = "ENZARB_DOCKER_RUNTIME_CLASSES",
        value_delimiter = ',',
        default_value = docker::DEFAULT_RUNTIME_CLASSES
    )]
    docker_runtime_classes: Vec<String>,
    /// Size of each Project's PVC `docker` (`/var/lib/docker`).
    #[arg(long, env = "ENZARB_DOCKER_STORAGE", default_value = docker::DEFAULT_STORAGE)]
    docker_storage: String,
    /// dockerd sidecar CPU request, e.g. `250m`.
    #[arg(long, env = "ENZARB_DOCKER_CPU_REQUEST")]
    docker_cpu_request: Option<String>,
    /// dockerd sidecar CPU limit.
    #[arg(long, env = "ENZARB_DOCKER_CPU_LIMIT")]
    docker_cpu_limit: Option<String>,
    /// dockerd sidecar memory request, e.g. `512Mi`.
    #[arg(long, env = "ENZARB_DOCKER_MEMORY_REQUEST")]
    docker_memory_request: Option<String>,
    /// dockerd sidecar memory limit.
    #[arg(long, env = "ENZARB_DOCKER_MEMORY_LIMIT")]
    docker_memory_limit: Option<String>,
    /// YAML file adding or replacing runtime-class profiles (sidecar securityContext and
    /// dockerd args) over the built-in ones.
    #[arg(long, env = "ENZARB_DOCKER_PROFILES_FILE")]
    docker_profiles_file: Option<std::path::PathBuf>,
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

/// Parses `90`, `90s`, `10m`, `2h` (a bare number is seconds).
fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, mult) = match s.chars().last() {
        Some('s') => (&s[..s.len() - 1], 1),
        Some('m') => (&s[..s.len() - 1], 60),
        Some('h') => (&s[..s.len() - 1], 3600),
        _ => (s, 1),
    };
    num.trim()
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(mult))
        .map(Duration::from_secs)
        .ok_or_else(|| format!("invalid duration {s:?}; use e.g. 30s, 10m, 2h"))
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
    cfg.executor_callback_port = args.executor_callback_port;
    cfg.egress_exclude_cidrs =
        desired::parse_cidrs("--egress-exclude-cidrs", &args.egress_exclude_cidrs)?;
    anyhow::ensure!(
        !cfg.egress_exclude_cidrs.is_empty(),
        "--egress-exclude-cidrs (ENZARB_SANDBOX_EGRESS_EXCLUDE_CIDRS) is required: list the cluster's pod, service and node CIDRs"
    );
    cfg.dns_cidrs = desired::parse_cidrs("--dns-cidrs", &args.dns_cidrs)?;
    Ok(cfg)
}

fn docker_config(args: &Args) -> anyhow::Result<docker::DockerConfig> {
    let classes = args
        .docker_runtime_classes
        .iter()
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    let mut cfg = docker::DockerConfig::new(
        args.docker_image.clone(),
        classes,
        args.sandbox_runtime_class.trim().to_string(),
        args.docker_storage.clone(),
    );
    cfg.resources = docker_resources(args);
    if let Some(path) = &args.docker_profiles_file {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
        cfg.merge_profiles_yaml(&raw)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
    }
    cfg.validate().map_err(|e| anyhow::anyhow!(e))?;
    Ok(cfg)
}

fn docker_resources(args: &Args) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (key, cpu, mem) in [
        (
            "requests",
            &args.docker_cpu_request,
            &args.docker_memory_request,
        ),
        ("limits", &args.docker_cpu_limit, &args.docker_memory_limit),
    ] {
        let mut m = serde_json::Map::new();
        if let Some(c) = cpu {
            m.insert("cpu".into(), c.clone().into());
        }
        if let Some(c) = mem {
            m.insert("memory".into(), c.clone().into());
        }
        if !m.is_empty() {
            out.insert(key.into(), m.into());
        }
    }
    out.into()
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
    let docker = docker_config(&args)?;
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
        run_controller(client, &args, netpol.clone(), docker.clone()).await;
        return Ok(());
    };
    tokio::select! {
        _ = elector.acquire() => {}
        _ = shutdown_signal() => return Ok(()),
    }
    let lost = tokio::select! {
        _ = run_controller(client, &args, netpol.clone(), docker.clone()) => false,
        _ = elector.hold() => true,
    };
    elector.release().await;
    if lost {
        anyhow::bail!("lost leadership; exiting so a fresh process can rejoin as standby");
    }
    Ok(())
}

async fn run_controller(
    client: Client,
    args: &Args,
    netpol: desired::NetpolConfig,
    docker: docker::DockerConfig,
) {
    // Projects are cluster-scoped; everything else is watched in the sandbox namespace only.
    let projects: Api<Project> = Api::all(client.clone());
    let sandboxes: Api<Sandbox> = Api::namespaced(client.clone(), &args.sandbox_namespace);
    // The controller's own Projects reflector store backs the one-Project-per-repository check
    // (`reconcile` waits for its first list, so no reconcile sees an empty cache).
    let controller = Controller::new(projects.clone(), watcher::Config::default());
    let reader = controller.store();
    let related_store = reader.clone();
    let ctx = Arc::new(reconcile::Ctx::new(
        client,
        args.sandbox_roll_drain_timeout,
        Defaults {
            sandbox_image: args.sandbox_image.clone(),
            storage: args.default_storage.clone(),
            sandbox_namespace: args.sandbox_namespace.clone(),
            sandbox_service_account: args.sandbox_service_account.clone(),
            netpol,
            docker,
        },
        reader,
    ));
    info!("starting controller");
    controller
        .owns(sandboxes, watcher::Config::default())
        // A Project that is created, changed or deleted may change who wins a shared repository.
        .watches(projects, watcher::Config::default(), move |changed| {
            reconcile::related_projects(&changed, &related_store.state())
        })
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
