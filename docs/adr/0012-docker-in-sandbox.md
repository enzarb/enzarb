# 0012. Docker inside sandboxes, isolated by gVisor or Kata

Partly reverses the dockerd part of [ADR-0006](0006-scope-no-environments.md). Environments and deploys stay out of scope.

## Context
Agents and users need `docker build`, `docker run` and `docker compose` inside a sandbox: integration tests (testcontainers), building images, running dependent services. A Docker daemon needs privileges that are unsafe on a shared node kernel.

## Decision
- **Per-Project opt-in:** `spec.sandbox.docker.enabled` (default `false`).
- **dockerd sidecar:** when enabled, the operator adds a `dockerd` sidecar container to the sandbox pod.
  - Image: pinned `docker:<ver>-dind`.
  - The sandbox container reaches it through a unix socket on a shared `emptyDir` (`/var/run/docker/docker.sock`, `DOCKER_HOST=unix:///var/run/docker/docker.sock`). No TCP listener.
  - Docker's data root (`/var/lib/docker`) is on a dedicated per-Project PVC `docker` (separate from `workspace`), so image and layer caches survive restarts.
- **Mandatory isolation:** the pod's `runtimeClassName` must be in the operator's allowlist (`--docker-runtime-classes`, default `gvisor,kata`). Otherwise the operator refuses with condition `SandboxReady=False`, reason `DockerRequiresIsolation`. Dockerd is never scheduled on the node kernel.
  - **Kata (preferred for production):** a VM boundary. The sidecar may run `privileged`, because the privilege is confined to the guest VM. Nodes need KVM (bare metal or nested virtualization).
  - **gVisor (supported, and used in dev):** a user-space kernel. Docker-in-gVisor runs with the capabilities gVisor documents, not `privileged`, and uses gVisor's supported storage and networking. The exact settings are pinned in enzarb-sandbox-operator, with known limitations documented.
- **The sandbox image ships the Docker CLI only:** a static `docker` binary plus the `buildx` and `compose` plugins, copied from a build stage. No daemon in the main container, so the minimal-image rule still holds.
- **Agent permission policy:** allows `docker` and `docker compose` subcommands. It denies `--privileged`, `--pid=host`, `--network=host`, `--cap-add`, device flags, and bind mounts outside the worktree. Inner containers still sit inside the pod's isolation boundary; the policy just narrows what the agent asks for.
- **Dev (kind):** `dev:cluster` installs gVisor (`runsc` plus the containerd shim) into the kind nodes and creates RuntimeClass `gvisor`. Kata is production-only (documented in `enzarb-kubernetes/overlays/prod`).

## Consequences
- **Node requirements:** Docker-enabled Projects only schedule onto nodes with the chosen runtime. In production, label or taint those nodes and give RuntimeClass `scheduling.nodeSelector`.
- **Resources:** the sidecar adds CPU and memory and a second PVC per Docker-enabled Project. Pod resources get a separate `docker.resources` block.
- **NetworkPolicy:** the existing per-namespace egress (DNS and 443) covers registry pulls over HTTPS. Plain-HTTP registries aren't reachable by design.
- **gVisor limits:** compatibility gaps for some workloads (certain syscalls and filesystems) apply under gVisor. Kata avoids them, at the cost of nested virtualization.
