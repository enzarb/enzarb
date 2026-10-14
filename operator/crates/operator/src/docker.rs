//! Docker-in-sandbox (ADR-0016, superseding the sidecar of ADR-0012): the runtime-class table and
//! the pure builder for what Docker adds to the `sandbox` container.
//!
//! dockerd runs inside the `sandbox` container, supervised by sandbox-init (pid 1), so there is no
//! sidecar and no shared socket volume (gVisor's gofer cannot carry Unix sockets across
//! containers, and a sidecar has no `/workspace` for bind mounts). The table maps a RuntimeClass
//! name to the `sandbox` container's `securityContext` and the extra `dockerd` arguments that
//! runtime needs. Defaults for `gvisor` and `kata` are built in; a YAML file
//! (`--docker-profiles-file`) adds or replaces entries so new runtimes slot in without code.
//!
//! Sources for the defaults:
//! - gVisor, "Docker in gVisor": <https://gvisor.dev/docs/tutorials/docker-in-gvisor/>. Docker
//!   v27-v29 needs runsc `--net-raw` (v28+ also `--allow-packet-socket-write`); run dockerd with
//!   `--iptables=false --ip6tables=false`; v29 needs the classic graph driver
//!   (`--feature=containerd-snapshotter=false`); a documented capability set, not privileged.
//! - Kata, privileged containers: <https://github.com/kata-containers/kata-containers/blob/main/docs/how-to/privileged.md>.
//!   Privileged is confined to the guest VM when `privileged_without_host_devices = true`
//!   (kata-deploy's default).

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const DEFAULT_RUNTIME_CLASSES: &str = "gvisor";
pub const DEFAULT_STORAGE: &str = "50Gi";
pub const DATA_VOLUME: &str = "docker-data";
pub const DOCKER_HOST: &str = "unix:///var/run/docker.sock";
pub const DATA_DIR: &str = "/var/lib/docker";
/// Env var read by sandbox-init: the runtime profile's `dockerdArgs`, space separated.
pub const ENV_DOCKERD_ARGS: &str = "ENZARB_DOCKERD_ARGS";
/// Env vars of sandbox-init's automatic Docker pruning, passed through when set.
pub const ENV_PRUNE: &str = "ENZARB_DOCKER_PRUNE";
pub const ENV_PRUNE_AGE: &str = "ENZARB_DOCKER_PRUNE_AGE";
pub const ENV_PRUNE_MIN_FREE: &str = "ENZARB_DOCKER_PRUNE_MIN_FREE";

/// Docker settings for one runtime class.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeProfile {
    /// `securityContext` of the `sandbox` container (core/v1 shape). It overrides the pod-level
    /// non-root settings: sandbox-init starts as root to run dockerd and drops every child it
    /// spawns to uid 1000.
    pub security_context: Value,
    /// Extra `dockerd` arguments appended after the common ones (no spaces inside an argument).
    #[serde(default)]
    pub dockerd_args: Vec<String>,
}

/// Cluster-admin owned Docker settings (operator flags); Projects cannot change them.
#[derive(Clone, Debug)]
pub struct DockerConfig {
    /// RuntimeClass names allowed to host dockerd.
    pub runtime_classes: Vec<String>,
    /// RuntimeClass every sandbox pod runs with. Must be in `runtime_classes`.
    pub sandbox_runtime_class: String,
    /// Size of each Project's PVC `docker`.
    pub storage: String,
    pub profiles: BTreeMap<String, RuntimeProfile>,
    /// Extra sandbox env for automatic Docker pruning (`ENZARB_DOCKER_PRUNE*`); only the settings
    /// the cluster admin chose, so sandbox-init's own defaults apply to the rest.
    pub prune_env: Vec<(String, String)>,
}

/// Capabilities gVisor documents for Docker-in-gVisor. They only exist inside the sandbox kernel,
/// never on the host. `SETUID`, `SETGID` and `CHOWN` also let sandbox-init drop to uid 1000 and
/// hand it files.
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
            // overlay2 works on the gofer-backed docker volume (verified in gVisor, issue #119);
            // vfs copies every layer in full. Override via `dockerdArgs` to go back.
            "--storage-driver=overlay2",
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
        runtime_classes: Vec<String>,
        sandbox_runtime_class: String,
        storage: String,
    ) -> Self {
        Self {
            runtime_classes,
            sandbox_runtime_class,
            storage,
            profiles: default_profiles(),
            prune_env: Vec::new(),
        }
    }

    /// Sets the pruning env from the operator flags (unset or blank = left to sandbox-init's
    /// defaults). Rejects values sandbox-init would refuse, which would crash-loop every sandbox.
    pub fn set_prune(
        &mut self,
        enabled: Option<&str>,
        age: Option<&str>,
        min_free: Option<&str>,
    ) -> Result<(), String> {
        fn set(v: Option<&str>) -> Option<&str> {
            v.map(str::trim).filter(|s| !s.is_empty())
        }
        self.prune_env.clear();
        if let Some(v) = set(enabled) {
            if !matches!(
                v.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "0" | "false" | "no" | "off"
            ) {
                return Err(format!("--docker-prune {v:?}: must be on or off"));
            }
            self.prune_env.push((ENV_PRUNE.into(), v.into()));
        }
        if let Some(v) = set(age) {
            let ok = !v.is_empty()
                && v.ends_with(|c: char| c.is_ascii_alphabetic())
                && v.starts_with(|c: char| c.is_ascii_digit());
            if !ok {
                return Err(format!(
                    "--docker-prune-age {v:?}: must be a duration such as 24h"
                ));
            }
            self.prune_env.push((ENV_PRUNE_AGE.into(), v.into()));
        }
        if let Some(v) = set(min_free) {
            let n = v.strip_suffix('%').unwrap_or(v).trim().parse::<u8>();
            if !n.is_ok_and(|n| (1..=99).contains(&n)) {
                return Err(format!(
                    "--docker-prune-min-free {v:?}: must be a percentage from 1 to 99"
                ));
            }
            self.prune_env.push((ENV_PRUNE_MIN_FREE.into(), v.into()));
        }
        Ok(())
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
        for (rc, p) in &self.profiles {
            if let Some(a) = p
                .dockerd_args
                .iter()
                .find(|a| a.contains(char::is_whitespace))
            {
                return Err(format!(
                    "docker profile {rc:?}: dockerdArgs entry {a:?} contains whitespace (the args reach sandbox-init as one space separated variable)"
                ));
            }
        }
        Ok(())
    }

    /// Profile of the sandbox runtime class. Only meaningful after `validate`.
    pub fn profile(&self) -> Option<&RuntimeProfile> {
        self.profiles.get(&self.sandbox_runtime_class)
    }
}

/// Everything Docker adds to the sandbox pod. There is no extra container.
pub struct DockerParts {
    /// `securityContext` of the `sandbox` container.
    pub security_context: Value,
    /// Env vars for the `sandbox` container.
    pub env: Vec<Value>,
    /// Mount of the `docker-data` volume in the `sandbox` container.
    pub volume_mount: Value,
    /// The `docker-data` pod volume.
    pub volume: Value,
}

/// The Docker parts of the `sandbox` container and pod. `claim_name` is the Project's docker PVC
/// (`sb-<name>-docker`).
pub fn parts(
    profile: &RuntimeProfile,
    claim_name: &str,
    prune_env: &[(String, String)],
) -> DockerParts {
    let mut env = vec![
        json!({"name": "DOCKER_HOST", "value": DOCKER_HOST}),
        json!({"name": ENV_DOCKERD_ARGS, "value": profile.dockerd_args.join(" ")}),
    ];
    env.extend(
        prune_env
            .iter()
            .map(|(k, v)| json!({"name": k, "value": v})),
    );
    DockerParts {
        security_context: profile.security_context.clone(),
        env,
        volume_mount: json!({"name": DATA_VOLUME, "mountPath": DATA_DIR}),
        volume: json!({"name": DATA_VOLUME, "persistentVolumeClaim": {"claimName": claim_name}}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(rc: &str) -> DockerConfig {
        DockerConfig::new(
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
        // sandbox-init drops to uid 1000 and chowns: it needs these as root.
        let caps = g.security_context["capabilities"]["add"]
            .as_array()
            .unwrap();
        for c in [
            "SETUID",
            "SETGID",
            "CHOWN",
            "SYS_ADMIN",
            "NET_ADMIN",
            "NET_RAW",
        ] {
            assert!(caps.contains(&json!(c)), "{c}");
        }
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
    fn args_with_whitespace_are_rejected() {
        let mut c = cfg("gvisor");
        c.merge_profiles_yaml("gvisor: {securityContext: {}, dockerdArgs: ['--log-level debug']}")
            .unwrap();
        assert!(c.validate().unwrap_err().contains("whitespace"));
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
    fn prune_env_is_passed_through_when_set() {
        let mut c = cfg("gvisor");
        c.set_prune(Some("off"), Some(" 12h "), Some("20%"))
            .unwrap();
        let p = parts(c.profile().unwrap(), "sb-x-docker", &c.prune_env);
        assert_eq!(
            p.env[2..],
            [
                json!({"name": "ENZARB_DOCKER_PRUNE", "value": "off"}),
                json!({"name": "ENZARB_DOCKER_PRUNE_AGE", "value": "12h"}),
                json!({"name": "ENZARB_DOCKER_PRUNE_MIN_FREE", "value": "20%"}),
            ]
        );
        c.set_prune(None, Some(""), None).unwrap();
        assert!(c.prune_env.is_empty());
        for bad in [
            (Some("maybe"), None, None),
            (None, Some("soon"), None),
            (None, None, Some("0")),
            (None, None, Some("100%")),
        ] {
            assert!(c.set_prune(bad.0, bad.1, bad.2).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parts_shape() {
        let c = cfg("gvisor");
        let p = parts(c.profile().unwrap(), "sb-x-docker", &[]);
        assert_eq!(p.env.len(), 2, "no pruning env unless configured");
        assert_eq!(p.security_context, c.profile().unwrap().security_context);
        assert_eq!(
            p.env[0],
            json!({"name": "DOCKER_HOST", "value": "unix:///var/run/docker.sock"})
        );
        assert_eq!(p.env[1]["name"], "ENZARB_DOCKERD_ARGS");
        let args = p.env[1]["value"].as_str().unwrap();
        assert!(args.contains("--storage-driver=overlay2"), "{args}");
        assert!(args.contains("--iptables=false"), "{args}");
        assert!(!args.contains("tcp://"), "{args}");
        assert_eq!(
            p.volume_mount,
            json!({"name": "docker-data", "mountPath": "/var/lib/docker"})
        );
        assert_eq!(
            p.volume,
            json!({"name": "docker-data", "persistentVolumeClaim": {"claimName": "sb-x-docker"}})
        );
    }
}
