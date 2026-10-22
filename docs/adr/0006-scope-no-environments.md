# 0006. Scope: no Environment CR, no deploys

## Context
The original design included an `Environment` CRD and user-driven deploys from the sandbox: rootless dockerd, and per-environment hostnames under `env.enzarb.local`.

## Decision
The current focus is **sandboxes against code repositories, and logging the events those repositories send**.
- The `Environment` CRD is removed. `Project` is the only enzarb CRD.
- The user deploy path, the `env.enzarb.local` routes and wildcard certificates are out of scope.
- Rootless dockerd in the sandbox is dropped by default. (Superseded by ADR-0012: Docker runs as an isolated sidecar.)

## Consequences
- Simpler operator, Gateway configuration and sandbox image.
- Deploys can come back later as a separate ADR.
