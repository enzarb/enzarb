# 0012. Docker inside every sandbox, isolated by gVisor or Kata

Partly reverses the dockerd part of [ADR-0006](0006-scope-no-environments.md). Environments and deploys stay out of scope.

## Context
Agents and users need `docker build`, `docker run` and `docker compose` inside a sandbox: integration tests (testcontainers), building images, running dependent services. A Docker daemon needs privileges that are unsafe on a shared node kernel.

## Decision
- **Always on, not user-selectable:** every sandbox gets Docker. Users and Projects cannot turn it off or choose the isolation runtime (user direction, 2026-10-08).
- **dockerd sidecar:** the operator always adds a `dockerd` sidecar container to the sandbox pod.
  - Image: pinned `docker:<ver>-dind` (`--docker-image`).
  - The sandbox container reaches it through a unix socket on a shared `emptyDir` (`/var/run/docker/docker.sock`, `DOCKER_HOST=unix:///var/run/docker/docker.sock`). No TCP listener.
  - Docker's data root (`/var/lib/docker`) is on a dedicated per-Project PVC `docker` (`--docker-storage`, default 50Gi), so image and layer caches survive restarts.
- **Mandatory isolation, cluster-admin owned:** the operator's `--sandbox-runtime-class` (env `ENZARB_SANDBOX_RUNTIME_CLASS`) sets the `runtimeClassName` of every sandbox pod.
  - It must be in `--docker-runtime-classes` (default `gvisor,kata`); the operator refuses to start otherwise.
  - The Project CRD has no runtime-class or Docker fields. Dockerd never runs on the node kernel.
  - **Kata (preferred for production):** a VM boundary. The sidecar may run `privileged`, because the privilege is confined to the guest VM. Nodes need KVM (bare metal or nested virtualization).
  - **gVisor (supported, and used in dev):** a user-space kernel. Docker-in-gVisor runs with the capabilities gVisor documents, not `privileged`, and uses gVisor's supported storage and networking. The exact settings are pinned in enzarb-sandbox-operator, with known limitations documented.
- **The sandbox image ships the Docker CLI only:** a static `docker` binary plus the `buildx` and `compose` plugins, copied from a build stage. No daemon in the main container, so the minimal-image rule still holds.
- **Agent permission policy:** allows `docker` and `docker compose` subcommands. It denies `--privileged`, `--pid=host`, `--network=host`, `--cap-add`, device flags, and bind mounts outside the worktree. Inner containers still sit inside the pod's isolation boundary; the policy just narrows what the agent asks for.
- **Dev (kind):** `dev:cluster` installs gVisor (`runsc` plus the containerd shim) into the kind nodes and creates RuntimeClass `gvisor`. Kata is production-only (documented in `enzarb-kubernetes/overlays/prod`).

## Consequences
- **Node requirements:** every sandbox only schedules onto nodes with the configured runtime. In production, label or taint those nodes and give RuntimeClass `scheduling.nodeSelector`.
- **Resources:** the sidecar adds CPU and memory and a second PVC to every sandbox, sized by operator flags.
- **NetworkPolicy:** the existing per-namespace egress (DNS and 443) covers registry pulls over HTTPS. Plain-HTTP registries aren't reachable by design.
- **gVisor limits:** compatibility gaps for some workloads (certain syscalls and filesystems) apply under gVisor. Kata avoids them, at the cost of nested virtualization.
