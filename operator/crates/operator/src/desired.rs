//! Pure builders for every object a Project needs. Nothing here talks to the cluster.
//!
//! All objects live in one shared sandbox namespace (ADR-0013), carry per-Project names
//! (`sb-<project>`, ...), the labels `app.kubernetes.io/managed-by` and `enzarb.dev/project`,
//! and a controller ownerReference to the cluster-scoped Project, so deleting a Project
//! garbage-collects them.
//!
//! agent-sandbox v0.5.6 notes (checked against the upstream controller):
//! - With `spec.service: true` it creates a *headless* Service named after the Sandbox
//!   (`sb-<project>`), selecting the sandbox pod, with ports taken from the container ports.
//!   With the field unset it never creates one, so we set it explicitly instead of
//!   creating our own Service. The DNS name `sb-<project>.<ns>.svc` then resolves to the pod IP.
//! - The pod is described inline by `spec.podTemplate` (core/v1 PodSpec shape).
//! - `status.conditions[type=Ready]` is True when the pod is Ready and, if a Service is
//!   requested, the Service exists.

use crate::docker::{self, DockerConfig};
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{MAX_PROJECT_NAME_LEN, Project, SandboxStatus, SecretKeyRef};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Secret};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::Resource;
use serde_json::json;
use std::collections::BTreeMap;

pub const FIELD_MANAGER: &str = "enzarb-operator";
pub const LABEL_MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const LABEL_PROJECT: &str = "enzarb.dev/project";
pub const AUTH_SECRET_KEY: &str = "token";
pub const SANDBOX_PORT: i32 = 7070;
pub const AUTH_MOUNT_DIR: &str = "/var/run/enzarb/auth";
/// uid/gid of user `enzarb` in the sandbox image (enzarb-sandbox-container).
pub const SANDBOX_UID: i64 = 1000;
pub const WORKSPACE_MOUNT: &str = "/workspace";
/// `app.kubernetes.io/name` of the workflows executor pods (egress callback target).
pub const EXECUTOR_POD_NAME: &str = "enzarb-workflows";

/// Operator-wide defaults supplied by flags.
#[derive(Clone, Debug)]
pub struct Defaults {
    pub sandbox_image: String,
    pub storage: String,
    /// The shared namespace every Project's objects live in.
    pub sandbox_namespace: String,
    /// ServiceAccount of every sandbox pod (no token, no RBAC).
    pub sandbox_service_account: String,
    pub netpol: NetpolConfig,
    pub docker: DockerConfig,
}

/// Who may reach the sandbox port (7070), and what a sandbox may reach besides DNS and 443.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetpolConfig {
    /// `(key, value)` label that selects the namespace(s) the clients live in.
    pub ingress_namespace_label: (String, String),
    /// Allowed client pods: `app.kubernetes.io/name in (...)`.
    pub ingress_pod_names: Vec<String>,
    /// TCP port of the workflows executor callback listener (egress rule).
    pub executor_callback_port: u16,
    /// CIDRs excluded from the TCP 443 egress rule (cluster pod, service, node networks).
    pub egress_exclude_cidrs: Vec<String>,
    /// Extra DNS server CIDRs allowed on port 53 besides kube-dns (e.g. NodeLocal DNSCache).
    pub dns_cidrs: Vec<String>,
}

impl NetpolConfig {
    pub const DEFAULT_CALLBACK_PORT: u16 = 8081;

    pub fn default_pod_names() -> Vec<String> {
        vec!["enzarb-sandbox-proxy".into(), "enzarb-workflows".into()]
    }

    /// Default: clients live in the release namespace.
    pub fn for_release_namespace(ns: &str) -> Self {
        Self {
            ingress_namespace_label: ("kubernetes.io/metadata.name".into(), ns.into()),
            ingress_pod_names: Self::default_pod_names(),
            executor_callback_port: Self::DEFAULT_CALLBACK_PORT,
            egress_exclude_cidrs: vec![],
            dns_cidrs: vec![],
        }
    }
}

/// `ipBlock` peers for an address family, from `cidrs` (families are told apart by `:`).
fn ip_block(all: &str, except: &[&String]) -> serde_json::Value {
    if except.is_empty() {
        json!({"ipBlock": {"cidr": all}})
    } else {
        json!({"ipBlock": {"cidr": all, "except": except}})
    }
}

/// How an existing object relates to this Project instance.
#[derive(Debug, PartialEq, Eq)]
pub enum Ownership {
    /// Controller-owned by this Project (same uid) and not being deleted.
    Mine,
    /// Being deleted, or controlled by another Project uid (a predecessor of the same name):
    /// garbage collection will remove it.
    Terminating,
    /// No controller ownerReference at all: something else created it. Needs an admin.
    NotOwned,
}

pub fn ownership(
    meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    p: &Project,
) -> Ownership {
    if meta.deletion_timestamp.is_some() {
        return Ownership::Terminating;
    }
    match meta
        .owner_references
        .iter()
        .flatten()
        .find(|r| r.controller == Some(true))
    {
        None => Ownership::NotOwned,
        // Controlled by something other than a Project: nobody will ever clean it up.
        Some(r) if r.kind != "Project" || r.api_version != "enzarb.dev/v1alpha1" => {
            Ownership::NotOwned
        }
        Some(r) if p.metadata.uid.as_ref() == Some(&r.uid) => Ownership::Mine,
        // Another Project uid: a predecessor of the same name, which GC will remove.
        Some(_) => Ownership::Terminating,
    }
}

/// Parses and validates a comma-separated CIDR list: canonical (no host bits set) and not `/0`.
pub fn parse_cidrs(flag: &str, raw: &[String]) -> anyhow::Result<Vec<String>> {
    let mut out = vec![];
    for c in raw.iter().map(|c| c.trim()).filter(|c| !c.is_empty()) {
        let bad = |why: &str| anyhow::anyhow!("{flag}: {c:?} {why}");
        let (ip, len) = c
            .split_once('/')
            .ok_or_else(|| bad("is not a CIDR such as 10.0.0.0/8"))?;
        // Digits only, no sign and no leading zero ("+8", "08" are rejected).
        let canonical = !len.is_empty()
            && len.bytes().all(|b| b.is_ascii_digit())
            && (len == "0" || !len.starts_with('0'));
        if !canonical {
            return Err(bad(
                "has a non-canonical prefix length (digits only, no leading zero)",
            ));
        }
        let len: u32 = len
            .parse()
            .map_err(|_| bad("has an invalid prefix length"))?;
        let ip: std::net::IpAddr = ip.parse().map_err(|_| bad("has an invalid address"))?;
        let (bits, width) = match ip {
            std::net::IpAddr::V4(a) => (u128::from(u32::from(a)), 32),
            std::net::IpAddr::V6(a) => (u128::from(a), 128),
        };
        if len > width {
            return Err(bad("has a prefix length that is too large"));
        }
        if len == 0 {
            return Err(bad(
                "is a /0 range; it would match everything (the API server rejects an ipBlock that excludes its whole range)",
            ));
        }
        let host_bits = if len == width {
            0
        } else {
            bits & ((1u128 << (width - len)) - 1)
        };
        if host_bits != 0 {
            return Err(bad(
                "has host bits set; use the network address (e.g. 10.0.0.0/8)",
            ));
        }
        out.push(c.to_string());
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidProject {
    #[error("project has no name")]
    NoName,
    #[error("project name {0:?} is not a valid DNS label: {1}")]
    BadName(String, &'static str),
    #[error("spec.owner is empty")]
    NoOwner,
    #[error("no sandbox image: set spec.sandbox.image or run the operator with --sandbox-image")]
    NoImage,
}

fn dns_label(s: &str) -> Result<(), &'static str> {
    if s.is_empty() || s.len() > MAX_PROJECT_NAME_LEN {
        return Err("must be 1-50 characters");
    }
    if !s
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("only lowercase letters, digits and '-' are allowed");
    }
    if s.starts_with('-') || s.ends_with('-') {
        return Err("must start and end with an alphanumeric character");
    }
    Ok(())
}

/// Checks the Project name (a DNS label of at most 50 characters, so `sb-<name>-workspace`
/// fits 63), the owner, and that an image is available.
pub fn validate(p: &Project, d: &Defaults) -> Result<(), InvalidProject> {
    let name = project_name(p);
    if name.is_empty() {
        return Err(InvalidProject::NoName);
    }
    if p.spec.owner.is_empty() {
        return Err(InvalidProject::NoOwner);
    }
    dns_label(name).map_err(|e| InvalidProject::BadName(name.to_string(), e))?;
    if image(p, d).is_empty() {
        return Err(InvalidProject::NoImage);
    }
    Ok(())
}

pub fn project_name(p: &Project) -> &str {
    p.metadata.name.as_deref().unwrap_or_default()
}

pub fn image(p: &Project, d: &Defaults) -> String {
    if p.spec.sandbox.image.is_empty() {
        d.sandbox_image.clone()
    } else {
        p.spec.sandbox.image.clone()
    }
}

pub fn storage(p: &Project, d: &Defaults) -> String {
    if p.spec.sandbox.storage.is_empty() {
        d.storage.clone()
    } else {
        p.spec.sandbox.storage.clone()
    }
}

/// `ws://sb-<project>.<namespace>.svc:7070/v1/mux`.
pub fn endpoint(p: &Project, namespace: &str) -> String {
    format!(
        "ws://{}.{namespace}.svc:{SANDBOX_PORT}/v1/mux",
        p.sandbox_name()
    )
}

fn base_labels(p: &Project) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_MANAGED_BY.to_string(), FIELD_MANAGER.to_string()),
        (LABEL_PROJECT.to_string(), project_name(p).to_string()),
    ])
}

fn owner_refs(p: &Project) -> Vec<k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference> {
    p.controller_owner_ref(&())
        .map(|mut r| {
            // Needs `update` on projects/finalizers (see deploy/rbac.yaml).
            r.block_owner_deletion = Some(true);
            r
        })
        .into_iter()
        .collect()
}

fn owner_refs_json(p: &Project) -> serde_json::Value {
    json!(owner_refs(p))
}

/// Secret `sb-<name>-auth` holding `token`. Created once by the reconciler; never re-applied.
pub fn auth_secret(p: &Project, d: &Defaults, token: &str) -> Secret {
    Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(p.auth_secret_name()),
            namespace: Some(d.sandbox_namespace.clone()),
            labels: Some(base_labels(p)),
            owner_references: Some(owner_refs(p)),
            ..Default::default()
        },
        type_: Some("Opaque".into()),
        data: Some(BTreeMap::from([(
            AUTH_SECRET_KEY.to_string(),
            ByteString(token.as_bytes().to_vec()),
        )])),
        ..Default::default()
    }
}

fn pvc_named(p: &Project, d: &Defaults, name: String, size: String) -> PersistentVolumeClaim {
    serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": {
            "name": name,
            "namespace": d.sandbox_namespace,
            "labels": base_labels(p),
            "ownerReferences": owner_refs_json(p),
        },
        "spec": {
            "accessModes": ["ReadWriteOnce"],
            "resources": {"requests": {"storage": size}},
        },
    }))
    .expect("pvc JSON matches the PVC type")
}

/// PVC `sb-<name>-workspace` sized from `spec.sandbox.storage`.
pub fn pvc(p: &Project, d: &Defaults) -> PersistentVolumeClaim {
    pvc_named(p, d, p.workspace_pvc_name(), storage(p, d))
}

/// PVC `sb-<name>-docker` for `/var/lib/docker`, sized from `--docker-storage`.
pub fn docker_pvc(p: &Project, d: &Defaults) -> PersistentVolumeClaim {
    pvc_named(p, d, p.docker_pvc_name(), d.docker.storage.clone())
}

/// NetworkPolicy `sb-<name>`, selecting only this Project's sandbox pod.
///
/// Ingress: TCP 7070 only from the configured client pods/namespaces.
/// Egress: DNS to kube-dns (plus optional extra DNS CIDRs); TCP 443 to any address except the
/// cluster's pod, service and node CIDRs (`--egress-exclude-cidrs`, required, so the API
/// server endpoint is excluded); the workflows executor callback port.
///
/// The Kubernetes API server is deliberately not reachable (ADR-0013). Policies are evaluated
/// after DNAT, so the `kubernetes` Service address (443) is covered by the service CIDR
/// exclusion and the endpoint (6443) is not allowed by any rule. A chart-owned default-deny
/// policy for the namespace (enzarb-helm) covers pods without a policy of their own.
pub fn network_policy(p: &Project, d: &Defaults) -> NetworkPolicy {
    let c = &d.netpol;
    let (ns_key, ns_val) = &c.ingress_namespace_label;
    let (v6, v4): (Vec<&String>, Vec<&String>) =
        c.egress_exclude_cidrs.iter().partition(|c| c.contains(':'));
    let mut dns_to = vec![json!({
        "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
        "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}},
    })];
    dns_to.extend(
        c.dns_cidrs
            .iter()
            .map(|cidr| json!({"ipBlock": {"cidr": cidr}})),
    );
    serde_json::from_value(json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": {
            "name": p.network_policy_name(),
            "namespace": d.sandbox_namespace,
            "labels": base_labels(p),
            "ownerReferences": owner_refs_json(p),
        },
        "spec": {
            "podSelector": {"matchLabels": {LABEL_PROJECT: project_name(p)}},
            "policyTypes": ["Ingress", "Egress"],
            "ingress": [{
                // One peer with both selectors: namespace AND pod must match.
                "from": [{
                    "namespaceSelector": {"matchLabels": {ns_key: ns_val}},
                    "podSelector": {"matchExpressions": [{
                        "key": "app.kubernetes.io/name",
                        "operator": "In",
                        "values": c.ingress_pod_names,
                    }]},
                }],
                "ports": [{"protocol": "TCP", "port": SANDBOX_PORT}],
            }],
            "egress": [
                // DNS: kube-dns pods (and any configured extra DNS CIDRs) only.
                {
                    "to": dns_to,
                    "ports": [
                        {"protocol": "UDP", "port": 53},
                        {"protocol": "TCP", "port": 53},
                    ],
                },
                // HTTPS to the internet (GitHub, Anthropic, registries), not the cluster networks.
                {
                    "to": [ip_block("0.0.0.0/0", &v4), ip_block("::/0", &v6)],
                    "ports": [{"protocol": "TCP", "port": 443}],
                },
                // Run callbacks to the workflows executor.
                {
                    "to": [{
                        "namespaceSelector": {"matchLabels": {ns_key: ns_val}},
                        "podSelector": {"matchLabels": {"app.kubernetes.io/name": EXECUTOR_POD_NAME}},
                    }],
                    "ports": [{"protocol": "TCP", "port": c.executor_callback_port}],
                },
            ],
        },
    }))
    .expect("networkpolicy JSON matches the NetworkPolicy type")
}

/// Env var read by sandbox-init for the concurrent agent run limit; set only with `spec.maxRuns`.
pub const ENV_MAX_RUNS: &str = "ENZARB_MAX_RUNS";

/// agent-sandbox `Sandbox` `sb-<name>`: one container running sandbox-init on 7070.
pub fn sandbox(p: &Project, d: &Defaults) -> Sandbox {
    let sb = &p.spec.sandbox;
    let mut pod_spec = json!({
        // ADR-0013: no Kubernetes API access. Dedicated SA with no bindings, no token (this
        // pod-level setting also covers the dockerd sidecar; no projected token volumes), and
        // no Service env vars from the shared namespace.
        "serviceAccountName": d.sandbox_service_account,
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        // Matches user `enzarb` in the sandbox image; fsGroup lets that user read the token.
        "securityContext": {
            "runAsUser": SANDBOX_UID,
            "runAsGroup": SANDBOX_UID,
            "fsGroup": SANDBOX_UID,
            "runAsNonRoot": true,
            // Do not recursively chown the (large) docker PVC on every start.
            "fsGroupChangePolicy": "OnRootMismatch",
        },
        "containers": [{
            "name": "sandbox",
            "image": image(p, d),
            "ports": [{"name": "mux", "containerPort": SANDBOX_PORT, "protocol": "TCP"}],
            "env": [{"name": "ENZARB_PROJECT", "value": project_name(p)}],
            "resources": sb.resources,
            "readinessProbe": {"httpGet": {"path": "/healthz", "port": SANDBOX_PORT}},
            "volumeMounts": [
                {"name": "workspace", "mountPath": WORKSPACE_MOUNT},
                {"name": "auth", "mountPath": AUTH_MOUNT_DIR, "readOnly": true},
            ],
        }],
        "volumes": [
            {"name": "workspace", "persistentVolumeClaim": {"claimName": p.workspace_pvc_name()}},
            {"name": "auth", "secret": {
                "secretName": p.auth_secret_name(),
                "defaultMode": 0o400,
                "items": [{"key": AUTH_SECRET_KEY, "path": AUTH_SECRET_KEY}],
            }},
        ],
    });
    // Every sandbox runs in the cluster-admin chosen isolating runtime, and always gets dockerd.
    // `DockerConfig::validate` guarantees the class is allowed and has a profile.
    pod_spec["runtimeClassName"] = json!(d.docker.sandbox_runtime_class);
    let profile = d
        .docker
        .profile()
        .expect("DockerConfig::validate ran at startup");
    let sc = docker::sidecar(
        profile,
        &d.docker.image,
        &d.docker.resources,
        SANDBOX_UID,
        &p.docker_pvc_name(),
    );
    {
        let main = &mut pod_spec["containers"][0];
        main["env"]
            .as_array_mut()
            .expect("env is an array")
            .push(json!({"name": "DOCKER_HOST", "value": docker::DOCKER_HOST}));
        if let Some(n) = p.spec.max_runs {
            main["env"]
                .as_array_mut()
                .expect("env is an array")
                .push(json!({"name": ENV_MAX_RUNS, "value": n.to_string()}));
        }
        main["volumeMounts"]
            .as_array_mut()
            .expect("volumeMounts is an array")
            .push(json!({"name": docker::SOCK_VOLUME, "mountPath": docker::SOCK_DIR}));
        pod_spec["containers"]
            .as_array_mut()
            .expect("containers is an array")
            .push(sc.container);
        pod_spec["volumes"]
            .as_array_mut()
            .expect("volumes is an array")
            .extend(sc.volumes);
    }
    serde_json::from_value(json!({
        "apiVersion": "agents.x-k8s.io/v1beta1",
        "kind": "Sandbox",
        "metadata": {
            "name": p.sandbox_name(),
            "namespace": d.sandbox_namespace,
            "labels": base_labels(p),
            "ownerReferences": owner_refs_json(p),
        },
        "spec": {
            // Makes agent-sandbox create the headless Service `sb-<name>`.
            "service": true,
            // The pod labels are what the per-Project NetworkPolicy selects.
            "podTemplate": {
                "metadata": {"labels": base_labels(p)},
                "spec": pod_spec,
            },
        },
    }))
    .expect("sandbox JSON matches the Sandbox type")
}

/// The `status.sandbox` block.
pub fn sandbox_status(p: &Project, d: &Defaults) -> SandboxStatus {
    SandboxStatus {
        name: p.sandbox_name(),
        endpoint: endpoint(p, &d.sandbox_namespace),
        auth_secret_ref: SecretKeyRef {
            name: p.auth_secret_name(),
            key: AUTH_SECRET_KEY.into(),
        },
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use enzarb_crds::{ProjectSpec, SandboxSpec};
    use k8s_openapi::api::core::v1::ResourceRequirements;
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

    pub(crate) fn project(name: &str, owner: &str) -> Project {
        let mut p = Project::new(
            name,
            ProjectSpec {
                owner: owner.into(),
                display_name: String::new(),
                repositories: vec![],
                run_mode: Default::default(),
                triggers: Default::default(),
                sandbox: SandboxSpec::default(),
                max_runs: None,
            },
        );
        p.metadata.uid = Some("uid-1".into());
        p
    }

    pub(crate) fn defaults() -> Defaults {
        Defaults {
            sandbox_image: "ghcr.io/enzarb/sandbox:dev".into(),
            storage: "10Gi".into(),
            sandbox_namespace: "enzarb-sandboxes".into(),
            sandbox_service_account: "enzarb-sandbox".into(),
            netpol: NetpolConfig {
                egress_exclude_cidrs: vec![
                    "10.0.0.0/8".into(),
                    "192.168.0.0/16".into(),
                    "fd00::/8".into(),
                ],
                ..NetpolConfig::for_release_namespace("enzarb")
            },
            docker: DockerConfig::new(
                docker::DEFAULT_IMAGE.into(),
                vec!["gvisor".into(), "kata".into()],
                "gvisor".into(),
                "50Gi".into(),
            ),
        }
    }

    fn containers(s: &serde_json::Value) -> &Vec<serde_json::Value> {
        s["spec"]["podTemplate"]["spec"]["containers"]
            .as_array()
            .unwrap()
    }

    #[test]
    fn docker_sidecar_always_present_per_runtime_class() {
        for (rc, privileged) in [("gvisor", false), ("kata", true)] {
            let mut d = defaults();
            d.docker.sandbox_runtime_class = rc.into();
            let s = v(&sandbox(&project("enzarb", "ruckc"), &d));
            let pod = &s["spec"]["podTemplate"]["spec"];
            assert_eq!(pod["runtimeClassName"], rc);
            let cs = containers(&s);
            assert_eq!(cs.len(), 2);
            let main = &cs[0];
            assert!(main["env"].as_array().unwrap().contains(
                &json!({"name": "DOCKER_HOST", "value": "unix:///var/run/docker/docker.sock"})
            ));
            assert!(
                main["volumeMounts"]
                    .as_array()
                    .unwrap()
                    .contains(&json!({"name": "docker-sock", "mountPath": "/var/run/docker"}))
            );
            let side = &cs[1];
            assert_eq!(side["name"], "dockerd");
            assert_eq!(side["image"], "docker:29.8.2-dind");
            assert_eq!(side["securityContext"]["privileged"], privileged, "{rc}");
            assert_eq!(side["securityContext"]["runAsUser"], 0);
            let vols = pod["volumes"].as_array().unwrap();
            assert!(vols.contains(&json!({"name": "docker-sock", "emptyDir": {}})));
            assert!(vols.contains(&json!({
                "name": "docker-data",
                "persistentVolumeClaim": {"claimName": "sb-enzarb-docker"}
            })));
            // The sandbox container keeps its non-root context.
            assert_eq!(pod["securityContext"]["runAsNonRoot"], true);
        }
    }

    #[test]
    fn gvisor_sidecar_is_not_privileged_and_uses_documented_settings() {
        let s = v(&sandbox(&project("enzarb", "ruckc"), &defaults()));
        let side = &containers(&s)[1];
        let caps = side["securityContext"]["capabilities"]["add"]
            .as_array()
            .unwrap();
        for c in ["SYS_ADMIN", "NET_ADMIN", "NET_RAW", "SYS_CHROOT", "MKNOD"] {
            assert!(caps.contains(&json!(c)), "{c}");
        }
        let args = side["args"].as_array().unwrap();
        assert!(args.contains(&json!("--iptables=false")));
        assert!(args.contains(&json!("--storage-driver=vfs")));
    }

    #[test]
    fn docker_pvc_and_resources_come_from_operator_config() {
        let mut d = defaults();
        d.docker.storage = "80Gi".into();
        d.docker.image = "my/dind:1".into();
        d.docker.resources = json!({"limits": {"memory": "4Gi"}});
        let p = project("enzarb", "ruckc");
        let s = v(&sandbox(&p, &d));
        assert_eq!(containers(&s)[1]["image"], "my/dind:1");
        assert_eq!(containers(&s)[1]["resources"]["limits"]["memory"], "4Gi");
        assert_eq!(containers(&s)[0]["resources"], json!({}));
        let c = v(&docker_pvc(&p, &d));
        assert_eq!(c["metadata"]["name"], "sb-enzarb-docker");
        assert_eq!(c["metadata"]["namespace"], "enzarb-sandboxes");
        assert_owned_and_labelled(&c);
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "80Gi");
        assert_eq!(c["spec"]["accessModes"][0], "ReadWriteOnce");
    }

    fn v<T: serde::Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap()
    }

    /// Every object carries the managed-by and project labels and a controller ownerReference.
    fn assert_owned_and_labelled(o: &serde_json::Value) {
        let l = &o["metadata"]["labels"];
        assert_eq!(l["app.kubernetes.io/managed-by"], "enzarb-operator");
        assert_eq!(l["enzarb.dev/project"], "enzarb");
        let r = &o["metadata"]["ownerReferences"];
        assert_eq!(r.as_array().unwrap().len(), 1, "{o}");
        assert_eq!(r[0]["apiVersion"], "enzarb.dev/v1alpha1");
        assert_eq!(r[0]["kind"], "Project");
        assert_eq!(r[0]["name"], "enzarb");
        assert_eq!(r[0]["uid"], "uid-1");
        assert_eq!(r[0]["controller"], true);
        assert_eq!(r[0]["blockOwnerDeletion"], true);
        assert_eq!(o["metadata"]["namespace"], "enzarb-sandboxes");
    }

    #[test]
    fn secret_has_token_key() {
        let s = auth_secret(&project("enzarb", "ruckc"), &defaults(), "abc");
        assert_eq!(s.metadata.name.as_deref(), Some("sb-enzarb-auth"));
        assert_owned_and_labelled(&v(&s));
        assert_eq!(s.data.unwrap()["token"].0, b"abc");
    }

    #[test]
    fn pvc_uses_spec_storage_or_default() {
        let mut p = project("enzarb", "ruckc");
        p.spec.sandbox.storage = "50Gi".into();
        let c = v(&pvc(&p, &defaults()));
        assert_eq!(c["metadata"]["name"], "sb-enzarb-workspace");
        assert_owned_and_labelled(&c);
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "50Gi");
        assert_eq!(c["spec"]["accessModes"][0], "ReadWriteOnce");
        p.spec.sandbox.storage = String::new();
        let c = v(&pvc(&p, &defaults()));
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "10Gi");
    }

    #[test]
    fn raised_storage_is_requested_and_docker_pvc_is_unaffected() {
        let mut p = project("enzarb", "ruckc");
        p.spec.sandbox.storage = "40Gi".into();
        let c = v(&pvc(&p, &defaults()));
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "40Gi");
        let before = v(&docker_pvc(&project("enzarb", "ruckc"), &defaults()));
        assert_eq!(v(&docker_pvc(&p, &defaults())), before);
    }

    fn main_env(p: &Project) -> Vec<serde_json::Value> {
        let s = v(&sandbox(p, &defaults()));
        containers(&s)[0]["env"].as_array().unwrap().clone()
    }

    #[test]
    fn max_runs_unset_injects_nothing() {
        let env = main_env(&project("enzarb", "ruckc"));
        assert!(env.iter().all(|e| e["name"] != "ENZARB_MAX_RUNS"));
        assert_eq!(env[0], json!({"name": "ENZARB_PROJECT", "value": "enzarb"}));
        assert!(env.iter().any(|e| e["name"] == "DOCKER_HOST"));
    }

    #[test]
    fn max_runs_set_is_a_string_env_var() {
        for n in [1u32, 3, 16] {
            let mut p = project("enzarb", "ruckc");
            p.spec.max_runs = Some(n);
            let env = main_env(&p);
            assert!(env.contains(&json!({"name": "ENZARB_MAX_RUNS", "value": n.to_string()})));
            assert_eq!(env[0]["name"], "ENZARB_PROJECT");
            assert!(env.iter().any(|e| e["name"] == "DOCKER_HOST"));
        }
    }

    #[test]
    fn sandbox_wires_mounts_port_env_and_service() {
        let s = v(&sandbox(&project("enzarb", "ruckc"), &defaults()));
        assert_eq!(s["apiVersion"], "agents.x-k8s.io/v1beta1");
        assert_eq!(s["kind"], "Sandbox");
        assert_eq!(s["metadata"]["name"], "sb-enzarb");
        assert_owned_and_labelled(&s);
        assert_eq!(s["spec"]["service"], true);
        let tpl = &s["spec"]["podTemplate"];
        assert_eq!(tpl["metadata"]["labels"]["enzarb.dev/project"], "enzarb");
        let pod = &tpl["spec"];
        assert_eq!(pod["serviceAccountName"], "enzarb-sandbox");
        assert_eq!(pod["automountServiceAccountToken"], false);
        assert_eq!(pod["enableServiceLinks"], false);
        assert_eq!(pod["runtimeClassName"], "gvisor");
        let c = &pod["containers"][0];
        assert_eq!(c["image"], "ghcr.io/enzarb/sandbox:dev");
        assert_eq!(c["ports"][0]["containerPort"], 7070);
        assert_eq!(c["env"][0]["name"], "ENZARB_PROJECT");
        assert_eq!(c["env"][0]["value"], "enzarb");
        assert_eq!(c["readinessProbe"]["httpGet"]["path"], "/healthz");
        let mounts: Vec<_> = c["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                (
                    m["name"].as_str().unwrap(),
                    m["mountPath"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            mounts,
            [
                ("workspace", "/workspace"),
                ("auth", "/var/run/enzarb/auth"),
                ("docker-sock", "/var/run/docker"),
            ]
        );
        assert_eq!(
            pod["volumes"][0]["persistentVolumeClaim"]["claimName"],
            "sb-enzarb-workspace"
        );
        assert_eq!(pod["volumes"][1]["secret"]["secretName"], "sb-enzarb-auth");
        assert_eq!(pod["volumes"][1]["secret"]["items"][0]["path"], "token");
        assert_eq!(pod["volumes"][1]["secret"]["defaultMode"], 0o400);
        let sc = &pod["securityContext"];
        assert_eq!(sc["runAsUser"], 1000);
        assert_eq!(sc["runAsGroup"], 1000);
        assert_eq!(sc["fsGroup"], 1000);
        assert_eq!(sc["runAsNonRoot"], true);
    }

    #[test]
    fn sandbox_honours_overrides() {
        let mut p = project("enzarb", "ruckc");
        p.spec.sandbox.image = "my/image:1".into();
        p.spec.sandbox.resources = ResourceRequirements {
            limits: Some(BTreeMap::from([("memory".into(), Quantity("2Gi".into()))])),
            ..Default::default()
        };
        let s = v(&sandbox(&p, &defaults()));
        let pod = &s["spec"]["podTemplate"]["spec"];
        assert_eq!(pod["containers"][0]["image"], "my/image:1");
        assert_eq!(pod["containers"][0]["resources"]["limits"]["memory"], "2Gi");
    }

    #[test]
    fn network_policy_shape() {
        let n = v(&network_policy(&project("enzarb", "ruckc"), &defaults()));
        assert_eq!(n["metadata"]["name"], "sb-enzarb");
        assert_owned_and_labelled(&n);
        let spec = &n["spec"];
        // Only this Project's pod, never the whole namespace.
        assert_eq!(
            spec["podSelector"],
            json!({"matchLabels": {"enzarb.dev/project": "enzarb"}})
        );
        assert_eq!(spec["policyTypes"], json!(["Ingress", "Egress"]));
        assert_eq!(spec["ingress"].as_array().unwrap().len(), 1);
        let from = &spec["ingress"][0]["from"];
        assert_eq!(from.as_array().unwrap().len(), 1);
        assert_eq!(
            from[0]["namespaceSelector"]["matchLabels"],
            json!({"kubernetes.io/metadata.name": "enzarb"})
        );
        let expr = &from[0]["podSelector"]["matchExpressions"][0];
        assert_eq!(expr["key"], "app.kubernetes.io/name");
        assert_eq!(expr["operator"], "In");
        assert_eq!(
            expr["values"],
            json!(["enzarb-sandbox-proxy", "enzarb-workflows"])
        );
        assert_eq!(
            spec["ingress"][0]["ports"],
            json!([{"protocol": "TCP", "port": 7070}])
        );
        let eg = spec["egress"].as_array().unwrap();
        assert_eq!(eg.len(), 3);
        assert_eq!(
            eg[0],
            json!({
                "to": [{
                    "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
                    "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}},
                }],
                "ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}],
            })
        );
        assert_eq!(
            eg[1],
            json!({
                "to": [
                    {"ipBlock": {"cidr": "0.0.0.0/0", "except": ["10.0.0.0/8", "192.168.0.0/16"]}},
                    {"ipBlock": {"cidr": "::/0", "except": ["fd00::/8"]}},
                ],
                "ports": [{"protocol": "TCP", "port": 443}],
            })
        );
        assert_eq!(
            eg[2],
            json!({
                "to": [{
                    "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "enzarb"}},
                    "podSelector": {"matchLabels": {"app.kubernetes.io/name": "enzarb-workflows"}},
                }],
                "ports": [{"protocol": "TCP", "port": 8081}],
            })
        );
        // No rule opens 6443 or any other port to the API server.
        assert!(!v(&spec["egress"]).to_string().contains("6443"));
    }

    #[test]
    fn network_policy_honours_config() {
        let mut d = defaults();
        d.netpol = NetpolConfig {
            ingress_namespace_label: ("team".into(), "platform".into()),
            ingress_pod_names: vec!["a".into()],
            executor_callback_port: 9000,
            egress_exclude_cidrs: vec!["10.0.0.0/8".into()],
            dns_cidrs: vec!["169.254.20.10/32".into()],
        };
        let n = v(&network_policy(&project("enzarb", "ruckc"), &d));
        let from = &n["spec"]["ingress"][0]["from"][0];
        assert_eq!(
            from["namespaceSelector"]["matchLabels"],
            json!({"team": "platform"})
        );
        assert_eq!(
            from["podSelector"]["matchExpressions"][0]["values"],
            json!(["a"])
        );
        let eg = &n["spec"]["egress"];
        // Extra DNS CIDR next to kube-dns; no IPv6 exceptions means no `except` key.
        assert_eq!(
            eg[0]["to"][1],
            json!({"ipBlock": {"cidr": "169.254.20.10/32"}})
        );
        assert_eq!(
            eg[1]["to"],
            json!([
                {"ipBlock": {"cidr": "0.0.0.0/0", "except": ["10.0.0.0/8"]}},
                {"ipBlock": {"cidr": "::/0"}},
            ])
        );
        let cb = &eg[2];
        assert_eq!(cb["ports"], json!([{"protocol": "TCP", "port": 9000}]));
        assert_eq!(
            cb["to"][0]["namespaceSelector"]["matchLabels"],
            json!({"team": "platform"})
        );
    }

    #[test]
    fn ownership_cases() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference, Time};
        let p = project("enzarb", "ruckc");
        let s: Secret = serde_json::from_value(v(&auth_secret(&p, &defaults(), "t"))).unwrap();
        assert_eq!(ownership(&s.metadata, &p), Ownership::Mine);
        // A predecessor with the same name but another uid.
        let mut old = project("enzarb", "ruckc");
        old.metadata.uid = Some("uid-0".into());
        let theirs = auth_secret(&old, &defaults(), "t");
        assert_eq!(ownership(&theirs.metadata, &p), Ownership::Terminating);
        // Being deleted wins.
        let mut dying = s.metadata.clone();
        dying.deletion_timestamp =
            Some(Time(k8s_openapi::jiff::Timestamp::from_second(1).unwrap()));
        assert_eq!(ownership(&dying, &p), Ownership::Terminating);
        // Controlled by another kind or API group: not a predecessor Project, needs an admin.
        for (kind, api) in [("Deployment", "apps/v1"), ("Project", "other.dev/v1")] {
            let other = ObjectMeta {
                owner_references: Some(vec![OwnerReference {
                    uid: "uid-0".into(),
                    kind: kind.into(),
                    api_version: api.into(),
                    controller: Some(true),
                    ..Default::default()
                }]),
                ..Default::default()
            };
            assert_eq!(ownership(&other, &p), Ownership::NotOwned, "{kind}");
        }
        // No controller ownerReference: nobody will garbage-collect it.
        assert_eq!(ownership(&ObjectMeta::default(), &p), Ownership::NotOwned);
        let non_controller = ObjectMeta {
            owner_references: Some(vec![OwnerReference {
                uid: "uid-1".into(),
                controller: Some(false),
                ..Default::default()
            }]),
            ..Default::default()
        };
        assert_eq!(ownership(&non_controller, &p), Ownership::NotOwned);
    }

    #[test]
    fn pod_uses_on_root_mismatch() {
        let s = v(&sandbox(&project("enzarb", "ruckc"), &defaults()));
        assert_eq!(
            s["spec"]["podTemplate"]["spec"]["securityContext"]["fsGroupChangePolicy"],
            "OnRootMismatch"
        );
    }

    #[test]
    fn cidr_parsing() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_cidrs("f", &s(&[" 10.0.0.0/8 ", "", "fd00::/8"])).unwrap(),
            s(&["10.0.0.0/8", "fd00::/8"])
        );
        for bad in [
            "10.0.0.0",
            "10.0.0.0/33",
            "x/8",
            "fd00::/129",
            "10.0.0.0/",
            // /0 would make the API server reject the policy.
            "0.0.0.0/0",
            "::/0",
            // Host bits set.
            "10.0.0.1/8",
            "192.168.1.0/16",
            "fd00::1/8",
            // Non-canonical prefix lengths.
            "10.0.0.0/+8",
            "10.0.0.0/08",
            "10.0.0.0/-8",
            "10.0.0.0/ 8",
        ] {
            assert!(parse_cidrs("f", &s(&[bad])).is_err(), "{bad}");
        }
        assert_eq!(
            parse_cidrs("f", &s(&["169.254.20.10/32", "10.96.0.0/12", "::1/128"]))
                .unwrap()
                .len(),
            3
        );
        let e = parse_cidrs("--egress-exclude-cidrs", &s(&["0.0.0.0/0"])).unwrap_err();
        assert!(e.to_string().contains("--egress-exclude-cidrs") && e.to_string().contains("/0"));
        assert!(parse_cidrs("f", &[]).unwrap().is_empty());
    }

    #[test]
    fn two_projects_never_share_names_or_selectors() {
        let d = defaults();
        let (a, b) = (project("a", "o"), project("b", "o"));
        let (na, nb) = (v(&network_policy(&a, &d)), v(&network_policy(&b, &d)));
        assert_ne!(na["metadata"]["name"], nb["metadata"]["name"]);
        assert_ne!(na["spec"]["podSelector"], nb["spec"]["podSelector"]);
        assert_ne!(sandbox(&a, &d).metadata.name, sandbox(&b, &d).metadata.name);
        assert_ne!(pvc(&a, &d).metadata.name, pvc(&b, &d).metadata.name);
        assert_ne!(
            docker_pvc(&a, &d).metadata.name,
            docker_pvc(&b, &d).metadata.name
        );
    }

    #[test]
    fn status_block_matches_contract() {
        let s = sandbox_status(&project("enzarb", "ruckc"), &defaults());
        assert_eq!(s.name, "sb-enzarb");
        assert_eq!(
            s.endpoint,
            "ws://sb-enzarb.enzarb-sandboxes.svc:7070/v1/mux"
        );
        assert_eq!(s.auth_secret_ref.name, "sb-enzarb-auth");
        assert_eq!(s.auth_secret_ref.key, "token");
    }

    #[test]
    fn validation_table() {
        let d = defaults();
        assert_eq!(validate(&project("enzarb", "ruckc"), &d), Ok(()));
        assert_eq!(
            validate(&project("enzarb", ""), &d),
            Err(InvalidProject::NoOwner)
        );
        assert!(matches!(
            validate(&project("Bad_Name", "ruckc"), &d),
            Err(InvalidProject::BadName(..))
        ));
        // 50 is the longest allowed name, 51 is rejected.
        assert_eq!(validate(&project(&"a".repeat(50), "ruckc"), &d), Ok(()));
        assert!(matches!(
            validate(&project(&"a".repeat(51), "ruckc"), &d),
            Err(InvalidProject::BadName(..))
        ));
        for bad in ["-a", "a-", "A", "a.b", "a_b"] {
            assert!(
                matches!(
                    validate(&project(bad, "ruckc"), &d),
                    Err(InvalidProject::BadName(..))
                ),
                "{bad}"
            );
        }
        // The owner is no longer part of any object name, so it need not be a DNS label.
        assert_eq!(validate(&project("enzarb", "Ruck_C"), &d), Ok(()));
        let no_image = Defaults {
            sandbox_image: String::new(),
            ..d
        };
        assert_eq!(
            validate(&project("enzarb", "ruckc"), &no_image),
            Err(InvalidProject::NoImage)
        );
    }
}
