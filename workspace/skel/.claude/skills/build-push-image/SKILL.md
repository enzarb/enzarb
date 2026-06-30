---
name: build-push-image
description: >
  Build and push a container image using the pre-configured BuildKit remote
  builder. Activate when the user asks to build, push, release, or deploy a
  container image (including when deploy implies a build). Also activate
  proactively after commits when a deploy is requested.
---

# build-push-image

The workspace has a BuildKit daemon running as a sidecar (`moby/buildkit:rootless`
on `tcp://localhost:1234`). On first boot, `project-agent` registers it as the
default `docker buildx` builder, so `docker buildx build` routes there automatically.

## Standard build + push

```bash
TAG=$(git rev-parse --short HEAD)

docker buildx build \
  --file deploy/Dockerfile \
  --tag "$REGISTRY/<image-name>:${TAG}" \
  --tag "$REGISTRY/<image-name>:latest" \
  --push \
  .
```

**`$REGISTRY`** is pre-set to `registry.enzarb.dev/{org}/{project}` — use it as
a prefix: `$REGISTRY/myimage:tag`, never `$REGISTRY:tag` alone.

## Rules

- **Always `--push`** — remote builders cannot load images locally; `--load` will fail.
- **Push credentials are automatic — do not wire them up.** `docker buildx` reads
  `$DOCKER_CONFIG`, resolves credentials via the `docker-credential-k8s-sa` helper,
  and forwards the auth token to buildkitd inline in the build request. No `--secret`,
  `docker login`, or extra flags are needed.
- **Never override `$DOCKER_CONFIG`** or pass `--config`. The credential helper is
  pre-wired via `/etc/enzarb/docker/config.json`. Overriding it loses the credentials
  and push will 401.
- **No `--builder` flag needed** — the `enzarb` builder is set as the default on
  pod start.

## Credential flow (for debugging)

Push auth is handled transparently:

1. `docker-credential-k8s-sa` reads the projected ServiceAccount token from
   `/var/run/secrets/enzarb/registry/token` (1h TTL, auto-rotated).
2. The token is presented to Zot as `username=sa-token, password=<token>`.
3. Zot validates it via `authd` (Kubernetes TokenReview).
4. The workspace SA token is only authorized for the `{org}/{project}` path prefix.

## Troubleshooting 401s on push

**1. Wrong `$DOCKER_CONFIG`** — most common cause. Something overrode `DOCKER_CONFIG`
or passed `--config`, bypassing the credential helper entirely. Verify:
```bash
echo $DOCKER_CONFIG   # must be /etc/enzarb/docker
cat $DOCKER_CONFIG/config.json  # must contain credHelpers entry
```

**2. Image path outside the authorized prefix** — the SA token is only authorized for
`registry.enzarb.dev/{org}/{project}/...`. Using `$ENZARB_REGISTRY` (the bare host)
instead of `$REGISTRY` (the scoped prefix), or constructing the tag manually with
the wrong org/project, will 401 at the Zot ACL layer even with a valid token.

**3. SA token missing or wrong audience** — the token must be projected with
`audience: registry.enzarb.dev`. Check the volume is present and the token is readable:
```bash
cat /var/run/secrets/enzarb/registry/token | cut -d. -f2 | base64 -d 2>/dev/null | jq .aud
# should show ["registry.enzarb.dev"]
```

**4. Token expired in the rotation window** — the 1h token is rotated by kubelet
before expiry, but there is a small gap. A simple retry resolves it.

**5. authd is down** — Zot validates tokens via authd (Kubernetes TokenReview). If
authd is unavailable, all pushes 401. Check with the platform operator.

**6. First-boot race** — if a build is run immediately after pod start, the `enzarb`
builder may not be registered yet and the default builder (which fails with an HTTP
protocol mismatch against the BuildKit gRPC port) may be used instead. Wait a few
seconds and retry, or run `docker buildx inspect enzarb` to confirm the builder exists.

## First-build note

The first build after pod start may take 5–15 minutes as BuildKit initializes its
rootless worker and pulls base layers. Subsequent builds hit the layer cache and
are fast.
