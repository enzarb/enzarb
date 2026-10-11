//! Every Kubernetes API call the operator makes, as `(apiGroup, resource, verb)`, and a test
//! that `deploy/rbac.yaml` grants exactly that. When you add or change an API call in
//! `reconcile.rs`, `leader.rs` or `main.rs`, update this list and `deploy/rbac.yaml`.
//!
//! Server-side apply (`Patch::Apply`) needs `patch` and, because the object may not exist yet,
//! `create`. The Project status is written with a merge patch (`projects/status` `patch`).

/// `(group, resource, verb)`. Cluster-scoped grants (ClusterRole): Projects only.
pub const CLUSTER_CALLS: &[(&str, &str, &str)] = &[
    // Controller watch. No get/patch: no finalizer, and only the status subresource is written.
    ("enzarb.dev", "projects", "list"),
    ("enzarb.dev", "projects", "watch"),
    ("enzarb.dev", "projects/status", "patch"),
    // Not an API call by the code: needed to set ownerReferences with blockOwnerDeletion=true
    // when the OwnerReferencesPermissionEnforcement admission plugin is enabled.
    ("enzarb.dev", "projects/finalizers", "update"),
];

/// Namespaced grants (Role in the sandbox namespace, ADR-0013). No namespaces, and no
/// cluster-wide secrets. `get` backs the ownership check before an object is reused; `patch`
/// is server-side apply (which also needs `create`).
pub const SANDBOX_NS_CALLS: &[(&str, &str, &str)] = &[
    ("", "secrets", "get"),
    ("", "secrets", "create"),
    ("", "persistentvolumeclaims", "get"),
    ("", "persistentvolumeclaims", "create"),
    ("", "persistentvolumeclaims", "patch"),
    ("networking.k8s.io", "networkpolicies", "get"),
    ("networking.k8s.io", "networkpolicies", "create"),
    ("networking.k8s.io", "networkpolicies", "patch"),
    // Sandboxes: Controller::owns watches them (namespaced Api).
    ("agents.x-k8s.io", "sandboxes", "get"),
    ("agents.x-k8s.io", "sandboxes", "list"),
    ("agents.x-k8s.io", "sandboxes", "watch"),
    ("agents.x-k8s.io", "sandboxes", "create"),
    ("agents.x-k8s.io", "sandboxes", "patch"),
    // Rolling a sandbox pod whose image is outdated (`roll.rs`): read it, then delete it so the
    // Sandbox controller recreates it. The Pod is the only object the operator ever deletes.
    ("", "pods", "get"),
    ("", "pods", "delete"),
];

/// Namespaced grants (Role in the operator's namespace): leader election.
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

    fn granted(kind: &str, name: &str) -> BTreeSet<Triple> {
        let raw = include_str!("../../../deploy/rbac.yaml");
        let mut out = BTreeSet::new();
        for doc in serde_yaml::Deserializer::from_str(raw) {
            let v = <serde_yaml::Value as serde::Deserialize>::deserialize(doc).unwrap();
            if v["kind"].as_str() != Some(kind) || v["metadata"]["name"].as_str() != Some(name) {
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
        let (have, want) = (
            granted("ClusterRole", "enzarb-sandbox-operator"),
            used(CLUSTER_CALLS),
        );
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
    fn sandbox_namespace_role_matches_code() {
        let (have, want) = (
            granted("Role", "enzarb-sandbox-operator"),
            used(SANDBOX_NS_CALLS),
        );
        assert_eq!(
            want.difference(&have).collect::<Vec<_>>(),
            Vec::<&Triple>::new(),
            "used by code but missing from deploy/rbac.yaml"
        );
        assert_eq!(
            have.difference(&want).collect::<Vec<_>>(),
            Vec::<&Triple>::new(),
            "granted in deploy/rbac.yaml but not used by code"
        );
    }

    #[test]
    fn leader_role_matches_code() {
        assert_eq!(
            granted("Role", "enzarb-sandbox-operator-leader"),
            used(LEADER_CALLS)
        );
    }

    /// The image roll deletes the Pod only: the Sandbox and the PVCs (the Project's data) must
    /// stay undeletable by the operator.
    #[test]
    fn only_pods_can_be_deleted() {
        let deletable: Vec<_> = SANDBOX_NS_CALLS
            .iter()
            .chain(CLUSTER_CALLS)
            .chain(LEADER_CALLS)
            .filter(|(_, _, v)| *v == "delete" || *v == "deletecollection")
            .map(|(_, r, _)| *r)
            .collect();
        assert_eq!(deletable, ["pods"]);
        let granted_delete: Vec<_> = granted("Role", "enzarb-sandbox-operator")
            .into_iter()
            .filter(|(_, _, v)| v == "delete" || v == "deletecollection")
            .map(|(_, r, _)| r)
            .collect();
        assert_eq!(granted_delete, ["pods"]);
    }

    #[test]
    fn no_namespace_or_cluster_wide_secret_access() {
        let cluster = granted("ClusterRole", "enzarb-sandbox-operator");
        assert!(
            !cluster
                .iter()
                .any(|(g, r, _)| g.is_empty() && (r == "namespaces" || r == "secrets"))
        );
    }
}
