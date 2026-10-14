# 0012. Docker inside every sandbox, isolated by gVisor

Partly reverses the dockerd part of [ADR-0006](0006-scope-no-environments.md). Environments and deploys stay out of scope.

> **Status:** the `dockerd` sidecar and the shared `docker-sock` volume are superseded by [ADR-0016](0016-dockerd-in-the-sandbox-container.md) (dockerd runs in the sandbox container, supervised by sandbox-init; the CLI-only image and the sidecar resource flags are gone). The rest of this ADR still applies. The text below is left as written.

## Context
Agents and users need `docker build`, `docker run` and `docker compose` inside a sandbox: integration tests (testcontainers), building images, running dependent services. A Docker daemon needs privileges that are unsafe on a shared node kernel.

## Decision
- **Always on, not user-selectable:** every sandbox gets Docker. Users and Projects cannot turn it off or choose the isolation runtime (user direction, 2026-10-08).
- **dockerd sidecar:** the operator always adds a `dockerd` sidecar container to the sandbox pod.
  - Image: pinned `docker:<ver>-dind` (`--docker-image`).
  - The sandbox container reaches it through a unix socket on a shared `emptyDir` (`/var/run/docker/docker.sock`, `DOCKER_HOST=unix:///var/run/docker/docker.sock`). No TCP listener.
  - Docker's data root (`/var/lib/docker`) is on a dedicated per-Project PVC `docker` (`--docker-storage`, default 50Gi), so image and layer caches survive restarts.
- **Mandatory isolation, cluster-admin owned:** the operator's `--sandbox-runtime-class` (env `ENZARB_SANDBOX_RUNTIME_CLASS`) sets the `runtimeClassName` of every sandbox pod.
  - It must be in `--docker-runtime-classes` (default `gvisor`); the operator refuses to start otherwise.
  - The Project CRD has no runtime-class or Docker fields. Dockerd never runs on the node kernel.
  - **gVisor is the one runtime enzarb installs, in dev and prod.** User decision (2026-10-08): install one, whichever is better.
    - It's a user-space kernel that intercepts syscalls. Docker-in-gVisor runs with the capabilities gVisor documents, not `privileged`, and uses gVisor's supported storage and networking.
    - The exact settings are pinned in enzarb-sandbox-operator, with known limitations documented.
  - **Why not Kata:** Kata's VM boundary is stronger, but it needs KVM on every node (bare metal or nested virtualization). Many cloud instance types don't offer that, and kind can't run it. gVisor works on kind and on ordinary VMs, so dev and prod match.
  - The operator keeps a runtime-class → settings table, so a Kata option could be added later without redesign.
- **The sandbox image ships the Docker CLI only:** a static `docker` binary plus the `buildx` and `compose` plugins, copied from a build stage. No daemon in the main container, so the minimal-image rule still holds.
- **Agent permission policy:** allows a fixed set of `docker` and `docker compose` subcommands.
  - **Denies:** `--privileged`, `--cap-add`, `--device`, `--security-opt`, host PID/IPC/UTS/userns/cgroupns, bind mounts or build paths outside the worktree (including the docker socket), `login`/`push`, and `-H`/`--context`.
  - **Allows `--network=host`:** under gVisor dockerd runs with `--iptables=false`, so `-p` can't publish ports. The "host" network is the dockerd sidecar's namespace inside gVisor, not the node's.
  - **Limit:** the policy sees only command lines (not compose file contents); gVisor is the real boundary.
- **Docker under gVisor:** dockerd uses `--iptables=false` and the `vfs` storage driver (amended 2026-10-14: now `overlay2`, see ADR-0016), and the runsc config enables `net-raw` and `allow-packet-socket-write` (Docker 28+).
- **Install:** enzarb-kubernetes installs gVisor (`runsc` plus the containerd shim) and RuntimeClass `gvisor`.
  - kind: a script run by `dev:cluster`.
  - Real clusters: a node-installer DaemonSet that labels capable nodes `enzarb.dev/runtime-gvisor=true`.

## Consequences
- **Node requirements:** every sandbox only schedules onto nodes with the configured runtime. In production, label or taint those nodes and give RuntimeClass `scheduling.nodeSelector`.
- **Resources:** the sidecar adds CPU and memory and a second PVC to every sandbox, sized by operator flags.
- **NetworkPolicy:** the per-Project egress policy (DNS and 443; ADR-0013) covers registry pulls over HTTPS. Plain-HTTP registries aren't reachable by design.
- **gVisor limits:** compatibility gaps for some workloads (certain syscalls and filesystems) apply. These are documented in the operator README. Revisit Kata if they block real workloads.
