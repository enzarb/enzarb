//! Every Kubernetes API call the operator makes, as `(apiGroup, resource, verb)`, and a test
//! that `deploy/rbac.yaml` grants exactly that. When you add or change an API call in
//! `reconcile.rs`, `leader.rs` or `main.rs`, update this list and `deploy/rbac.yaml`.
//!
//! Server-side apply (`Patch::Apply`) needs `patch` and, because the object may not exist yet,
//! `create`. The finalizer helper uses JSON patch on the Project (`patch`).

/// `(group, resource, verb)`. Cluster-scoped grants (ClusterRole).
pub const CLUSTER_CALLS: &[(&str, &str, &str)] = &[
    // Controller watch, finalizer.
    ("enzarb.dev", "projects", "get"),
    ("enzarb.dev", "projects", "list"),
    ("enzarb.dev", "projects", "watch"),
    ("enzarb.dev", "projects", "patch"),
    ("enzarb.dev", "projects/status", "patch"),
    // desired::tenant, SSA.
    ("capsule.clastix.io", "tenants", "create"),
    ("capsule.clastix.io", "tenants", "patch"),
    // desired::namespace SSA; cleanup get_opt + delete.
    ("", "namespaces", "get"),
    ("", "namespaces", "create"),
    ("", "namespaces", "patch"),
    ("", "namespaces", "delete"),
    // sandbox-auth: get_opt then create.
    ("", "secrets", "get"),
    ("", "secrets", "create"),
    // desired::pvc SSA.
    ("", "persistentvolumeclaims", "create"),
    ("", "persistentvolumeclaims", "patch"),
    // desired::sandbox SSA; Controller::owns watches Sandboxes.
    ("agents.x-k8s.io", "sandboxes", "list"),
    ("agents.x-k8s.io", "sandboxes", "watch"),
    ("agents.x-k8s.io", "sandboxes", "create"),
    ("agents.x-k8s.io", "sandboxes", "patch"),
];

/// Namespaced grants (Role in the operator namespace): leader election.
pub const LEADER_CALLS: &[(&str, &str, &str)] = &[
    ("coordination.k8s.io", "leases", "get"),
    ("coordination.k8s.io", "leases", "create"),
    ("coordination.k8s.io", "leases", "update"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    type Triple = (String, String, String);

    fn granted(kind: &str) -> BTreeSet<Triple> {
        let raw = include_str!("../../../deploy/rbac.yaml");
        let mut out = BTreeSet::new();
        for doc in serde_yaml::Deserializer::from_str(raw) {
            let v = <serde_yaml::Value as serde::Deserialize>::deserialize(doc).unwrap();
            if v["kind"].as_str() != Some(kind) {
                continue;
            }
            for rule in v["rules"].as_sequence().unwrap() {
                let list = |k: &str| -> Vec<String> {
                    rule[k]
                        .as_sequence()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_string())
                        .collect()
                };
                for g in list("apiGroups") {
                    for r in list("resources") {
                        for verb in list("verbs") {
                            out.insert((g.clone(), r.clone(), verb));
                        }
                    }
                }
            }
        }
        out
    }

    fn used(calls: &[(&str, &str, &str)]) -> BTreeSet<Triple> {
        calls
            .iter()
            .map(|(g, r, v)| (g.to_string(), r.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn cluster_role_matches_code() {
        let (have, want) = (granted("ClusterRole"), used(CLUSTER_CALLS));
        assert_eq!(
            want.difference(&have).collect::<Vec<_>>(),
            Vec::<&Triple>::new(),
            "used by code but missing from deploy/rbac.yaml"
        );
        assert_eq!(
            have.difference(&want).collect::<Vec<_>>(),
            Vec::<&Triple>::new(),
            "granted in deploy/rbac.yaml but not used by code (update rbac.rs or drop the grant)"
        );
    }

    #[test]
    fn leader_role_matches_code() {
        assert_eq!(granted("Role"), used(LEADER_CALLS));
    }
}
