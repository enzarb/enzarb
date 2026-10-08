//! Pure builders for every object a Project needs. Nothing here talks to the cluster.
//!
//! agent-sandbox v0.5.6 notes (checked against the upstream controller):
//! - With `spec.service: true` it creates a *headless* Service named after the Sandbox
//!   (`sandbox`), selecting the sandbox pod, with ports taken from the container ports.
//!   With the field unset it never creates one, so we set it explicitly instead of
//!   creating our own Service. The DNS name `sandbox.<ns>.svc` then resolves to the pod IP.
//! - The pod is described inline by `spec.podTemplate` (core/v1 PodSpec shape).
//! - `status.conditions[type=Ready]` is True when the pod is Ready and, if a Service is
//!   requested, the Service exists.

use crate::docker::{self, DockerConfig};
use enzarb_crds::upstream::capsule::Tenant;
use enzarb_crds::upstream::sandbox::Sandbox;
use enzarb_crds::{Project, SandboxStatus, SecretKeyRef};
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::{Namespace, PersistentVolumeClaim, Secret};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::Resource;
use serde_json::json;
use std::collections::BTreeMap;

pub const FIELD_MANAGER: &str = "enzarb-operator";
pub const FINALIZER: &str = "enzarb.dev/cleanup";
pub const LABEL_MANAGED_BY: &str = "app.kubernetes.io/managed-by";
pub const LABEL_PROJECT: &str = "enzarb.dev/project";
pub const LABEL_CAPSULE_TENANT: &str = "capsule.clastix.io/tenant";
pub const SANDBOX_NAME: &str = "sandbox";
pub const AUTH_SECRET_NAME: &str = "sandbox-auth";
pub const AUTH_SECRET_KEY: &str = "token";
pub const NETPOL_NAME: &str = "sandbox-isolation";
pub const PVC_NAME: &str = "workspace";
pub const SANDBOX_PORT: i32 = 7070;
pub const AUTH_MOUNT_DIR: &str = "/var/run/enzarb/auth";
/// uid/gid of user `enzarb` in the sandbox image (enzarb-sandbox-container).
pub const SANDBOX_UID: i64 = 1000;
pub const WORKSPACE_MOUNT: &str = "/workspace";

/// Operator-wide defaults supplied by flags.
#[derive(Clone, Debug)]
pub struct Defaults {
    pub sandbox_image: String,
    pub storage: String,
    pub netpol: NetpolConfig,
    pub docker: DockerConfig,
}

/// Who may reach the sandbox port (7070) in a project namespace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetpolConfig {
    /// `(key, value)` label that selects the namespace(s) the clients live in.
    pub ingress_namespace_label: (String, String),
    /// Allowed client pods: `app.kubernetes.io/name in (...)`.
    pub ingress_pod_names: Vec<String>,
}

impl NetpolConfig {
    pub fn default_pod_names() -> Vec<String> {
        vec!["enzarb-sandbox-proxy".into(), "enzarb-workflows".into()]
    }

    /// Default: clients live in the release namespace.
    pub fn for_release_namespace(ns: &str) -> Self {
        Self {
            ingress_namespace_label: ("kubernetes.io/metadata.name".into(), ns.into()),
            ingress_pod_names: Self::default_pod_names(),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidProject {
    #[error("project has no name")]
    NoName,
    #[error("{0} is not a valid DNS label: {1}")]
    BadName(String, &'static str),
    #[error("spec.owner is empty")]
    NoOwner,
    #[error("no sandbox image: set spec.sandbox.image or run the operator with --sandbox-image")]
    NoImage,
}

fn dns_label(s: &str) -> Result<(), &'static str> {
    if s.is_empty() || s.len() > 63 {
        return Err("must be 1-63 characters");
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

/// Checks that derived names are valid and an image is available.
pub fn validate(p: &Project, d: &Defaults) -> Result<(), InvalidProject> {
    if p.metadata.name.as_deref().unwrap_or_default().is_empty() {
        return Err(InvalidProject::NoName);
    }
    if p.spec.owner.is_empty() {
        return Err(InvalidProject::NoOwner);
    }
    for n in [p.namespace_name(), p.tenant_name()] {
        dns_label(&n).map_err(|e| InvalidProject::BadName(n.clone(), e))?;
    }
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

pub fn endpoint(namespace: &str) -> String {
    format!("ws://{SANDBOX_NAME}.{namespace}.svc:{SANDBOX_PORT}/v1/mux")
}

fn base_labels(p: &Project) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_MANAGED_BY.to_string(), FIELD_MANAGER.to_string()),
        (LABEL_PROJECT.to_string(), project_name(p).to_string()),
    ])
}

fn owner_refs(p: &Project) -> serde_json::Value {
    json!(p.controller_owner_ref(&()).into_iter().collect::<Vec<_>>())
}

/// Capsule Tenant `enzarb-<owner>`. The owner is a Capsule `User` named `<owner>`
/// (the GitHub login the UI authenticates). Shared by all of the owner's Projects, so
/// it carries no ownerReference and is never deleted by the operator.
pub fn tenant(p: &Project) -> Tenant {
    serde_json::from_value(json!({
        "apiVersion": "capsule.clastix.io/v1beta2",
        "kind": "Tenant",
        "metadata": {
            "name": p.tenant_name(),
            "labels": {LABEL_MANAGED_BY: FIELD_MANAGER},
        },
        "spec": { "owners": [{"kind": "User", "name": p.spec.owner}] },
    }))
    .expect("tenant JSON matches the Tenant type")
}

/// Namespace `enzarb-p-<name>`, assigned to the tenant through the Capsule label.
pub fn namespace(p: &Project) -> Namespace {
    let mut labels = base_labels(p);
    labels.insert(LABEL_CAPSULE_TENANT.to_string(), p.tenant_name());
    serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "Namespace",
        "metadata": {
            "name": p.namespace_name(),
            "labels": labels,
            "ownerReferences": owner_refs(p),
        },
    }))
    .expect("namespace JSON matches the Namespace type")
}

/// Whether a namespace was created for this Project (guards finalizer cleanup).
pub fn namespace_belongs_to(ns: &Namespace, project: &str) -> bool {
    ns.metadata
        .labels
        .as_ref()
        .and_then(|l| l.get(LABEL_PROJECT))
        .is_some_and(|v| v == project)
        && ns
            .metadata
            .labels
            .as_ref()
            .and_then(|l| l.get(LABEL_MANAGED_BY))
            .is_some_and(|v| v == FIELD_MANAGER)
}

/// Secret `sandbox-auth` holding `token`. Created once by the reconciler; never re-applied.
pub fn auth_secret(p: &Project, token: &str) -> Secret {
    Secret {
        metadata: kube::api::ObjectMeta {
            name: Some(AUTH_SECRET_NAME.into()),
            namespace: Some(p.namespace_name()),
            labels: Some(base_labels(p)),
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

/// PVC `workspace` sized from `spec.sandbox.storage`.
pub fn pvc(p: &Project, d: &Defaults) -> PersistentVolumeClaim {
    serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": {
            "name": PVC_NAME,
            "namespace": p.namespace_name(),
            "labels": base_labels(p),
        },
        "spec": {
            "accessModes": ["ReadWriteOnce"],
            "resources": {"requests": {"storage": storage(p, d)}},
        },
    }))
    .expect("pvc JSON matches the PVC type")
}

/// PVC `docker` for `/var/lib/docker`, sized from `--docker-storage`.
pub fn docker_pvc(p: &Project, d: &Defaults) -> PersistentVolumeClaim {
    serde_json::from_value(json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": {
            "name": docker::PVC_NAME,
            "namespace": p.namespace_name(),
            "labels": base_labels(p),
        },
        "spec": {
            "accessModes": ["ReadWriteOnce"],
            "resources": {"requests": {"storage": d.docker.storage}},
        },
    }))
    .expect("pvc JSON matches the PVC type")
}

/// NetworkPolicy `sandbox-isolation` for every pod in the project namespace.
/// Ingress: TCP 7070 only from the configured client pods/namespaces.
/// Egress: DNS (UDP/TCP 53) and TCP 443, to any destination. A narrower allow-list
/// (GitHub, Anthropic, registries) can replace the 443 rule later.
pub fn network_policy(p: &Project, c: &NetpolConfig) -> NetworkPolicy {
    let (ns_key, ns_val) = &c.ingress_namespace_label;
    serde_json::from_value(json!({
        "apiVersion": "networking.k8s.io/v1",
        "kind": "NetworkPolicy",
        "metadata": {
            "name": NETPOL_NAME,
            "namespace": p.namespace_name(),
            "labels": base_labels(p),
            "ownerReferences": owner_refs(p),
        },
        "spec": {
            "podSelector": {},
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
                {"ports": [
                    {"protocol": "UDP", "port": 53},
                    {"protocol": "TCP", "port": 53},
                ]},
                {"ports": [{"protocol": "TCP", "port": 443}]},
            ],
        },
    }))
    .expect("networkpolicy JSON matches the NetworkPolicy type")
}

/// agent-sandbox `Sandbox` `sandbox`: one container running sandbox-init on 7070.
pub fn sandbox(p: &Project, d: &Defaults) -> Sandbox {
    let sb = &p.spec.sandbox;
    let mut pod_spec = json!({
        // The sandbox gets cluster access through capsule-proxy, not a pod token.
        "automountServiceAccountToken": false,
        // Matches user `enzarb` in the sandbox image; fsGroup lets that user read the token.
        "securityContext": {
            "runAsUser": SANDBOX_UID,
            "runAsGroup": SANDBOX_UID,
            "fsGroup": SANDBOX_UID,
            "runAsNonRoot": true,
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
            {"name": "workspace", "persistentVolumeClaim": {"claimName": PVC_NAME}},
            {"name": "auth", "secret": {
                "secretName": AUTH_SECRET_NAME,
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
    let sc = docker::sidecar(profile, &d.docker.image, &d.docker.resources, SANDBOX_UID);
    {
        let main = &mut pod_spec["containers"][0];
        main["env"]
            .as_array_mut()
            .expect("env is an array")
            .push(json!({"name": "DOCKER_HOST", "value": docker::DOCKER_HOST}));
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
            "name": SANDBOX_NAME,
            "namespace": p.namespace_name(),
            "labels": base_labels(p),
            "ownerReferences": owner_refs(p),
        },
        "spec": {
            // Makes agent-sandbox create the headless Service `sandbox`.
            "service": true,
            "podTemplate": {
                "metadata": {"labels": base_labels(p)},
                "spec": pod_spec,
            },
        },
    }))
    .expect("sandbox JSON matches the Sandbox type")
}

/// The `status.sandbox` block.
pub fn sandbox_status(p: &Project) -> SandboxStatus {
    SandboxStatus {
        name: SANDBOX_NAME.into(),
        endpoint: endpoint(&p.namespace_name()),
        auth_secret_ref: SecretKeyRef {
            name: AUTH_SECRET_NAME.into(),
            key: AUTH_SECRET_KEY.into(),
        },
    }
}

#[cfg(test)]
mod tests {
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
            },
        );
        p.metadata.uid = Some("uid-1".into());
        p
    }

    fn defaults() -> Defaults {
        Defaults {
            sandbox_image: "ghcr.io/enzarb/sandbox:dev".into(),
            storage: "10Gi".into(),
            netpol: NetpolConfig::for_release_namespace("enzarb"),
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
                "persistentVolumeClaim": {"claimName": "docker"}
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
        assert_eq!(c["metadata"]["name"], "docker");
        assert_eq!(c["metadata"]["namespace"], "enzarb-p-enzarb");
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "80Gi");
        assert_eq!(c["spec"]["accessModes"][0], "ReadWriteOnce");
    }

    fn v<T: serde::Serialize>(t: &T) -> serde_json::Value {
        serde_json::to_value(t).unwrap()
    }

    #[test]
    fn tenant_named_after_owner() {
        let t = v(&tenant(&project("enzarb", "ruckc")));
        assert_eq!(t["apiVersion"], "capsule.clastix.io/v1beta2");
        assert_eq!(t["kind"], "Tenant");
        assert_eq!(t["metadata"]["name"], "enzarb-ruckc");
        assert_eq!(t["spec"]["owners"][0]["kind"], "User");
        assert_eq!(t["spec"]["owners"][0]["name"], "ruckc");
        assert!(t["metadata"].get("ownerReferences").is_none());
    }

    #[test]
    fn namespace_labelled_for_tenant_and_owned() {
        let n = v(&namespace(&project("enzarb", "ruckc")));
        assert_eq!(n["metadata"]["name"], "enzarb-p-enzarb");
        let l = &n["metadata"]["labels"];
        assert_eq!(l["capsule.clastix.io/tenant"], "enzarb-ruckc");
        assert_eq!(l["enzarb.dev/project"], "enzarb");
        assert_eq!(n["metadata"]["ownerReferences"][0]["kind"], "Project");
        assert_eq!(n["metadata"]["ownerReferences"][0]["uid"], "uid-1");
    }

    #[test]
    fn namespace_ownership_check() {
        let p = project("enzarb", "ruckc");
        let ns = namespace(&p);
        assert!(namespace_belongs_to(&ns, "enzarb"));
        assert!(!namespace_belongs_to(&ns, "other"));
        assert!(!namespace_belongs_to(&Namespace::default(), "enzarb"));
    }

    #[test]
    fn secret_has_token_key() {
        let s = auth_secret(&project("enzarb", "ruckc"), "abc");
        assert_eq!(s.metadata.name.as_deref(), Some("sandbox-auth"));
        assert_eq!(s.metadata.namespace.as_deref(), Some("enzarb-p-enzarb"));
        assert_eq!(s.data.unwrap()["token"].0, b"abc");
    }

    #[test]
    fn pvc_uses_spec_storage_or_default() {
        let mut p = project("enzarb", "ruckc");
        p.spec.sandbox.storage = "50Gi".into();
        let c = v(&pvc(&p, &defaults()));
        assert_eq!(c["metadata"]["name"], "workspace");
        assert_eq!(c["metadata"]["namespace"], "enzarb-p-enzarb");
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "50Gi");
        assert_eq!(c["spec"]["accessModes"][0], "ReadWriteOnce");
        p.spec.sandbox.storage = String::new();
        let c = v(&pvc(&p, &defaults()));
        assert_eq!(c["spec"]["resources"]["requests"]["storage"], "10Gi");
    }

    #[test]
    fn sandbox_wires_mounts_port_env_and_service() {
        let s = v(&sandbox(&project("enzarb", "ruckc"), &defaults()));
        assert_eq!(s["apiVersion"], "agents.x-k8s.io/v1beta1");
        assert_eq!(s["kind"], "Sandbox");
        assert_eq!(s["metadata"]["name"], "sandbox");
        assert_eq!(s["metadata"]["namespace"], "enzarb-p-enzarb");
        assert_eq!(s["metadata"]["ownerReferences"][0]["name"], "enzarb");
        assert_eq!(s["spec"]["service"], true);
        let pod = &s["spec"]["podTemplate"]["spec"];
        assert_eq!(pod["automountServiceAccountToken"], false);
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
            "workspace"
        );
        assert_eq!(pod["volumes"][1]["secret"]["secretName"], "sandbox-auth");
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
        let n = v(&network_policy(
            &project("enzarb", "ruckc"),
            &defaults().netpol,
        ));
        assert_eq!(n["metadata"]["name"], "sandbox-isolation");
        assert_eq!(n["metadata"]["namespace"], "enzarb-p-enzarb");
        assert_eq!(n["metadata"]["ownerReferences"][0]["kind"], "Project");
        let spec = &n["spec"];
        assert_eq!(spec["podSelector"], json!({}));
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
        assert_eq!(
            spec["egress"],
            json!([
                {"ports": [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]},
                {"ports": [{"protocol": "TCP", "port": 443}]},
            ])
        );
        // Egress rules have no `to`, i.e. any destination.
        assert!(spec["egress"][1].get("to").is_none());
    }

    #[test]
    fn network_policy_honours_config() {
        let c = NetpolConfig {
            ingress_namespace_label: ("team".into(), "platform".into()),
            ingress_pod_names: vec!["a".into()],
        };
        let n = v(&network_policy(&project("enzarb", "ruckc"), &c));
        let from = &n["spec"]["ingress"][0]["from"][0];
        assert_eq!(
            from["namespaceSelector"]["matchLabels"],
            json!({"team": "platform"})
        );
        assert_eq!(
            from["podSelector"]["matchExpressions"][0]["values"],
            json!(["a"])
        );
    }

    #[test]
    fn status_block_matches_contract() {
        let s = sandbox_status(&project("enzarb", "ruckc"));
        assert_eq!(s.name, "sandbox");
        assert_eq!(s.endpoint, "ws://sandbox.enzarb-p-enzarb.svc:7070/v1/mux");
        assert_eq!(s.auth_secret_ref.name, "sandbox-auth");
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
        assert!(matches!(
            validate(&project(&"a".repeat(60), "ruckc"), &d),
            Err(InvalidProject::BadName(..))
        ));
        assert!(matches!(
            validate(&project("enzarb", "Ruck_C"), &d),
            Err(InvalidProject::BadName(..))
        ));
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
