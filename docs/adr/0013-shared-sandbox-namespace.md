# 0013. One shared sandbox namespace; no Kubernetes API access from sandboxes

Supersedes the tenancy part of [ADR-0002](0002-sandbox-tenancy.md): one Capsule Tenant per owner, one namespace per Project, and a capsule-proxy kubeconfig inside each sandbox.

## Context
The first real deployment (oldie) showed that per-Project namespaces placed into per-owner Capsule Tenants need the operator to own every Tenant, and that the model buys little: sandboxes do not need the Kubernetes API for the current scope (ADR-0006), and the isolation that matters (gVisor, ADR-0012; NetworkPolicy; separate volumes and credentials) does not depend on namespaces.

## Decision
- **One namespace for all sandboxes:** `enzarb-sandboxes` by default (chart value `sandbox.namespace`), created by the Helm chart and separate from the release namespace (`enzarb`). The operator gets namespaced permissions there only; it no longer creates namespaces or Capsule Tenants.
- **Per-Project object names** in that namespace, all prefixed so they cannot collide and all labelled `enzarb.dev/project=<name>`:

  | Object | Name |
  |---|---|
  | Sandbox (and its headless Service) | `sb-<project>` |
  | Auth Secret | `sb-<project>-auth` |
  | Workspace PVC | `sb-<project>-workspace` |
  | Docker PVC | `sb-<project>-docker` |
  | NetworkPolicy | `sb-<project>` |

  Every one carries an ownerReference to its (cluster-scoped) Project, so deleting a Project garbage-collects them (including the volumes and their data). The longest name, `sb-<project>-workspace`, must fit a 63-character label, so **Project names are at most 50 characters** (DNS label).
- **No Kubernetes API access from sandboxes, for now:** sandbox pods use a dedicated ServiceAccount `enzarb-sandbox` with `automountServiceAccountToken: false` and no RBAC bindings, and no kubeconfig is provided. Egress policy does not allow the API server. **Any future API access will be limited exclusively to other namespaces** (for example per-Project work namespaces), granted through RoleBindings in those namespaces only; a sandbox will never get permissions in `enzarb-sandboxes` or in the release namespace.
- **Isolation inside the shared namespace:**
  - a chart-owned default-deny NetworkPolicy for the whole namespace (ingress and egress), so a pod without its own policy can do nothing and no single Project owns the namespace-wide rule;
  - a per-Project NetworkPolicy `sb-<project>` selecting that Project's pod: ingress on 7070 only from the sandbox-proxy and workflows executor pods in the release namespace; egress to DNS, TCP 443, and the workflows executor callback port in the release namespace. Sandboxes cannot reach each other.
- **Status and conditions** (contracts section 2): `status.namespace` is the shared namespace; `status.sandbox` names the per-Project Sandbox, endpoint and auth Secret. The boot conditions are `ResourcesReady` (auth Secret, volumes, NetworkPolicy), `SandboxReady`, `Ready`; `TenantReady` and `NamespaceReady` are removed. An invalid spec is reported as `ResourcesReady=False, reason InvalidSpec`.
- **Consumers** (sandbox-proxy, workflows executor) find the sandbox only through `status.namespace` and `status.sandbox`; there is no derived fallback name. Their Secret read permission becomes a Role in the sandbox namespace (Secret names are per Project, so `resourceNames` cannot pin them).

## Consequences
- Capsule and capsule-proxy are no longer used by enzarb at runtime. Whether to keep them as cluster prerequisites is decided together with the future "other namespaces" API access; until then they stay installed but unused.
- Users no longer get kubectl access to a Project namespace (ADR-0002 assumed scoped kubectl through capsule-proxy). Interactive access is the sandbox terminal.
- Resource quotas are per namespace in Kubernetes; per-Project limits come from the sandbox pod's own resources (and a LimitRange for defaults), not from per-Project quotas.
- Migration: the operator removes the old `enzarb.dev/cleanup` finalizer from existing Projects and deletes only the Capsule Tenants it created; old `enzarb-p-*` namespaces, if any, are deleted with their Projects.
