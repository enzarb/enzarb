//! Docker-in-sandbox (ADR-0012): the runtime-class table and the pure sidecar builder.
//!
//! The table maps a RuntimeClass name to the sidecar `securityContext` and the extra `dockerd`
//! arguments that runtime needs. Defaults for `gvisor` and `kata` are built in; a YAML file
//! (`--docker-profiles-file`) adds or replaces entries so new runtimes slot in without code.
//!
//! Sources for the defaults:
//! - gVisor, "Docker in gVisor": <https://gvisor.dev/docs/tutorials/docker-in-gvisor/>. Docker
//!   v27-v29 needs runsc `--net-raw` (v28+ also `--allow-packet-socket-write`); run dockerd with
//!   `--iptables=false --ip6tables=false`; v29 needs the classic graph driver
//!   (`--feature containerd-snapshotter=false`); a documented capability set, not privileged.
//! - Kata, privileged containers: <https://github.com/kata-containers/kata-containers/blob/main/docs/how-to/privileged.md>.
//!   Privileged is confined to the guest VM when `privileged_without_host_devices = true`
//!   (kata-deploy's default).

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Pinned default; override with `--docker-image`.
pub const DEFAULT_IMAGE: &str = "docker:29.8.2-dind";
pub const DEFAULT_RUNTIME_CLASSES: &str = "gvisor";
pub const DEFAULT_STORAGE: &str = "50Gi";
pub const SIDECAR_NAME: &str = "dockerd";
pub const SOCK_VOLUME: &str = "docker-sock";
pub const DATA_VOLUME: &str = "docker-data";
/// Shared with the sandbox container; holds only `docker.sock`.
pub const SOCK_DIR: &str = "/var/run/docker";
pub const DOCKER_HOST: &str = "unix:///var/run/docker/docker.sock";
pub const DATA_DIR: &str = "/var/lib/docker";
/// dockerd's exec-root (containerd socket, runtime state) is kept out of the shared socket dir.
pub const EXEC_ROOT: &str = "/var/run/dockerd";

/// Sidecar settings for one runtime class.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProfile {
    /// Container `securityContext` of the `dockerd` sidecar (core/v1 shape).
    pub security_context: Value,
    /// Extra `dockerd` arguments appended after the common ones.
    #[serde(default)]
    pub dockerd_args: Vec<String>,
}

/// Cluster-admin owned Docker settings (operator flags); Projects cannot change them.
#[derive(Clone, Debug)]
pub struct DockerConfig {
    pub image: String,
    /// RuntimeClass names allowed to host dockerd.
    pub runtime_classes: Vec<String>,
    /// RuntimeClass every sandbox pod runs with. Must be in `runtime_classes`.
    pub sandbox_runtime_class: String,
    /// Size of each Project's PVC `docker`.
    pub storage: String,
    /// Sidecar `ResourceRequirements` (core/v1 JSON).
    pub resources: Value,
    pub profiles: BTreeMap<String, RuntimeProfile>,
}

/// Capabilities gVisor documents for Docker-in-gVisor.
const GVISOR_CAPS: &[&str] = &[
    "AUDIT_WRITE",
    "CHOWN",
    "DAC_OVERRIDE",
    "FOWNER",
    "FSETID",
    "KILL",
    "MKNOD",
    "NET_BIND_SERVICE",
    "NET_ADMIN",
    "NET_RAW",
    "SETFCAP",
    "SETGID",
    "SETPCAP",
    "SETUID",
    "SYS_ADMIN",
    "SYS_CHROOT",
    "SYS_PTRACE",
];

pub fn default_profiles() -> BTreeMap<String, RuntimeProfile> {
    let gvisor = RuntimeProfile {
        security_context: json!({
            "privileged": false,
            "runAsUser": 0,
            "runAsNonRoot": false,
            "allowPrivilegeEscalation": true,
            "capabilities": {"drop": ["ALL"], "add": GVISOR_CAPS},
        }),
        dockerd_args: [
            "--iptables=false",
            "--ip6tables=false",
            // vfs works on any backing fs; overlay2 on a gofer-backed PVC is unverified.
            "--storage-driver=vfs",
            "--feature=containerd-snapshotter=false",
        ]
        .map(String::from)
        .to_vec(),
    };
    let kata = RuntimeProfile {
        security_context: json!({
            "privileged": true,
            "runAsUser": 0,
            "runAsNonRoot": false,
        }),
        dockerd_args: vec!["--storage-driver=overlay2".into()],
    };
    BTreeMap::from([("gvisor".into(), gvisor), ("kata".into(), kata)])
}

impl DockerConfig {
    pub fn new(
        image: String,
        runtime_classes: Vec<String>,
        sandbox_runtime_class: String,
        storage: String,
    ) -> Self {
        Self {
            image,
            runtime_classes,
            sandbox_runtime_class,
            storage,
            resources: json!({}),
            profiles: default_profiles(),
        }
    }

    /// Adds or replaces profiles from a YAML map `{<runtimeClass>: {securityContext, dockerdArgs}}`.
    pub fn merge_profiles_yaml(&mut self, yaml: &str) -> Result<(), serde_yaml::Error> {
        let extra: BTreeMap<String, RuntimeProfile> = serde_yaml::from_str(yaml)?;
        self.profiles.extend(extra);
        Ok(())
    }

    /// Startup check: the sandbox runtime class is set, allowed and has a profile, and every
    /// allowed class has a profile.
    pub fn validate(&self) -> Result<(), String> {
        let rc = self.sandbox_runtime_class.as_str();
        if rc.is_empty() {
            return Err(
                "--sandbox-runtime-class (ENZARB_SANDBOX_RUNTIME_CLASS) is required: dockerd only runs in an isolating runtime".into(),
            );
        }
        if !self.runtime_classes.iter().any(|c| c == rc) {
            return Err(format!(
                "--sandbox-runtime-class {rc:?} is not in --docker-runtime-classes [{}]",
                self.runtime_classes.join(", ")
            ));
        }
        let missing: Vec<&str> = self
            .runtime_classes
            .iter()
            .filter(|c| !self.profiles.contains_key(*c))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "--docker-runtime-classes has no dockerd profile for: {} (add them via --docker-profiles-file)",
                missing.join(", ")
            ));
        }
        Ok(())
    }

    /// Profile of the sandbox runtime class. Only meaningful after `validate`.
    pub fn profile(&self) -> Option<&RuntimeProfile> {
        self.profiles.get(&self.sandbox_runtime_class)
    }
}

/// Everything Docker adds to the sandbox pod.
pub struct Sidecar {
    pub container: Value,
    pub volumes: Vec<Value>,
}

/// The `dockerd` sidecar container and volumes. `claim_name` is the Project's docker PVC
/// (`sb-<name>-docker`).
pub fn sidecar(
    profile: &RuntimeProfile,
    image: &str,
    resources: &Value,
    gid: i64,
    claim_name: &str,
) -> Sidecar {
    let mut args = vec![
        format!("--host={DOCKER_HOST}"),
        // Socket group = the sandbox user's gid, so the unprivileged sandbox can connect.
        format!("--group={gid}"),
        format!("--exec-root={EXEC_ROOT}"),
        format!("--data-root={DATA_DIR}"),
    ];
    args.extend(profile.dockerd_args.iter().cloned());
    // `command` bypasses docker:dind's entrypoint, which would add a TCP listener (2375/2376).
    let container = json!({
        "name": SIDECAR_NAME,
        "image": image,
        "command": ["dockerd"],
        "args": args,
        "securityContext": profile.security_context,
        "resources": resources,
        "readinessProbe": {
            "exec": {"command": ["docker", "--host", DOCKER_HOST, "version"]},
            "periodSeconds": 5,
            "timeoutSeconds": 5,
        },
        "volumeMounts": [
            {"name": SOCK_VOLUME, "mountPath": SOCK_DIR},
            {"name": DATA_VOLUME, "mountPath": DATA_DIR},
        ],
    });
    let volumes = vec![
        json!({"name": SOCK_VOLUME, "emptyDir": {}}),
        json!({"name": DATA_VOLUME, "persistentVolumeClaim": {"claimName": claim_name}}),
    ];
    Sidecar { container, volumes }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(rc: &str) -> DockerConfig {
        DockerConfig::new(
            DEFAULT_IMAGE.into(),
            vec!["gvisor".into(), "kata".into()],
            rc.into(),
            "50Gi".into(),
        )
    }

    #[test]
    fn default_table() {
        let c = cfg("gvisor");
        let g = &c.profiles["gvisor"];
        assert_eq!(g.security_context["privileged"], false);
        assert_eq!(g.security_context["capabilities"]["drop"], json!(["ALL"]));
        assert!(g.dockerd_args.contains(&"--iptables=false".to_string()));
        assert_eq!(c.profiles["kata"].security_context["privileged"], true);
    }

    #[test]
    fn startup_validation_table() {
        assert_eq!(cfg("gvisor").validate(), Ok(()));
        assert_eq!(cfg("kata").validate(), Ok(()));
        let e = cfg("").validate().unwrap_err();
        assert!(e.contains("--sandbox-runtime-class"), "{e}");
        let e = cfg("runc").validate().unwrap_err();
        assert!(e.contains("not in --docker-runtime-classes"), "{e}");
        // Allowed but no profile.
        let mut c = cfg("gvisor");
        c.runtime_classes.push("kata-qemu".into());
        assert!(c.validate().unwrap_err().contains("kata-qemu"));
        c.merge_profiles_yaml("kata-qemu: {securityContext: {privileged: true}}")
            .unwrap();
        assert_eq!(c.validate(), Ok(()));
        // Only the configured class matters for the allowlist, and the default allows gvisor only.
        let d = DockerConfig::new(
            "i".into(),
            DEFAULT_RUNTIME_CLASSES
                .split(',')
                .map(String::from)
                .collect(),
            "kata".into(),
            "1Gi".into(),
        );
        assert!(d.validate().is_err());
    }

    #[test]
    fn profiles_file_adds_and_replaces() {
        let mut c = cfg("gvisor");
        c.merge_profiles_yaml(
            "kata-qemu:\n  securityContext: {privileged: true}\n  dockerdArgs: ['--storage-driver=overlay2']\n\
             gvisor:\n  securityContext: {privileged: false}\n",
        )
        .unwrap();
        assert!(c.profiles.contains_key("kata-qemu"));
        assert!(c.profile().unwrap().dockerd_args.is_empty());
        assert!(c.merge_profiles_yaml("gvisor: {dockerdArgs: []}").is_err());
    }

    #[test]
    fn sidecar_shape() {
        let c = cfg("gvisor");
        let sc = sidecar(
            c.profile().unwrap(),
            &c.image,
            &json!({}),
            1000,
            "sb-x-docker",
        );
        let k = &sc.container;
        assert_eq!(k["name"], "dockerd");
        assert_eq!(k["image"], DEFAULT_IMAGE);
        assert_eq!(k["command"], json!(["dockerd"]));
        let args: Vec<_> = k["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        assert!(args.contains(&"--host=unix:///var/run/docker/docker.sock"));
        assert!(args.contains(&"--group=1000"));
        assert!(args.contains(&"--storage-driver=vfs"));
        assert!(!args.iter().any(|a| a.contains("tcp://")));
        assert_eq!(sc.volumes[0]["emptyDir"], json!({}));
        assert_eq!(
            sc.volumes[1]["persistentVolumeClaim"]["claimName"],
            "sb-x-docker"
        );
    }
}
