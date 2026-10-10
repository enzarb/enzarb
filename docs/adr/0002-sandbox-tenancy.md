# 0002. Upstream agent-sandbox, Capsule tenancy, Project CRD

Status: Accepted; the tenancy decision (one Capsule Tenant per owner, a namespace per Project, capsule-proxy kubeconfig in sandboxes) is **superseded by [ADR-0013](0013-shared-sandbox-namespace.md)**.

## Context
Agents need isolated, long-lived workspaces on Kubernetes. Users need scoped kubectl access to their own namespaces.

## Decision
- **Sandboxes:** use upstream kubernetes-sigs/agent-sandbox `Sandbox` CRs (`agents.x-k8s.io/v1beta1`, pinned to v0.5.x). The Rust types are generated with kopium and committed.
- **Own CRD:** group `enzarb.dev/v1alpha1`, kind **`Project`** only (see 0006). A Project holds linked repo remotes, the sandbox template, and the run mode (`worktree` | `fresh`).
- **Tenancy:** one Capsule Tenant per owner. The sandbox's kubeconfig points at **capsule-proxy**, which is needed for a filtered `kubectl get ns`.
- **Production isolation:** a gVisor or Kata RuntimeClass.
- **Dev cluster:** kind + Envoy Gateway, domain `enzarb.local`.

## Consequences
- Upstream agent-sandbox is versioned and pinned; upgrades mean regenerating the kopium types.
- Capsule and capsule-proxy are cluster prerequisites (`enzarb-kubernetes`).
