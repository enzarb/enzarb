#!/usr/bin/env bash
# Smoke test for the sandbox image. Usage: scripts/smoke.sh [image]
set -euo pipefail

bash "$(dirname "$0")/test-docker-wrapper.sh"

image="${1:-enzarb-sandbox:dev}"
name="enzarb-smoke-$$"
tmp=$(mktemp -d)
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  rm -rf "$tmp"
}
trap cleanup EXIT

echo "smoke-token" > "$tmp/token"
chmod 0644 "$tmp/token"
# The image starts as root (sandbox-init supervises dockerd and drops children to uid 1000, ADR-0016).
# Outside gVisor dockerd needs more than the gVisor capability set; the CI runner has no kernel
# restrictions we care about here, so give it the caps and lift the default seccomp/AppArmor profiles.
# If it still cannot start, the Docker server checks are skipped (see below).
docker run -d --name "$name" -p 127.0.0.1::7070 \
  --cap-add SYS_ADMIN --cap-add NET_ADMIN --cap-add MKNOD \
  --security-opt seccomp=unconfined --security-opt apparmor=unconfined \
  -v "$tmp/token:/var/run/enzarb/auth/token:ro" "$image" >/dev/null
port=$(docker port "$name" 7070/tcp | head -1 | sed 's/.*://')

for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; then ok=1; break; fi
  sleep 1
done
[ "${ok:-0}" = 1 ] || { echo "FAIL: /healthz never answered"; docker logs "$name"; exit 1; }
echo "ok: /healthz"

check() {
  echo "-- $*"
  docker exec "$name" "$@"
}
check ez --version
check claude-agent-acp --version
check claude --version
check git --version
check node --version
check mise --version
check cc --version
check curl --version
# `docker version` exits non-zero without a daemon but still prints the client version.
v=$(docker exec "$name" sh -c "docker version --format '{{.Client.Version}}' 2>/dev/null; true")
echo "docker client: $v"
case "$v" in 29.*) ;; *) echo "FAIL: docker client version '$v'"; exit 1;; esac
check docker buildx version
check docker compose version
check dockerd --version
check containerd --version
check runc --version
# The wrapper passes plain subcommands through to the real client untouched.
check docker --version
check pkg-config --exists openssl
# mise shims must be on PATH for agents and shells.
docker exec "$name" sh -c 'echo "$PATH" | grep -q "/workspace/.toolchains/mise/shims"'
echo "ok: mise shims on PATH"
# Nothing else is baked in: toolchains come from mise.
for bin in cargo rustc rustup npm npx rg jq; do
  if docker exec "$name" sh -c "command -v $bin" >/dev/null 2>&1; then
    echo "FAIL: $bin must not be in the image"; exit 1
  fi
  echo "ok: no $bin"
done
uid=$(docker exec -u enzarb "$name" id -u)
[ "$uid" = 1000 ] || { echo "FAIL: uid is $uid, want 1000"; exit 1; }
echo "ok: user enzarb is uid 1000"
docker exec -u enzarb "$name" sh -c 'touch /workspace/.enzarb/.w "$HOME/.w" && rm /workspace/.enzarb/.w "$HOME/.w"'
echo "ok: /workspace and HOME writable"
# pid 1 is root; the drop-privileges exec mode (used for PTY children) gives uid/gid 1000, no new privileges.
who=$(docker exec "$name" sandbox-init --exec-as-sandbox-user sh -c 'echo "$(id -u):$(id -g):$(id -G)"; grep NoNewPrivs /proc/self/status')
case "$who" in "1000:1000:1000"*NoNewPrivs*1*) echo "ok: privilege drop ($who)";; *) echo "FAIL: privilege drop: $who"; exit 1;; esac

# dockerd (supervised by sandbox-init): /healthz reports its state and stays 200 either way.
state=
for _ in $(seq 1 60); do
  body=$(curl -fsS "http://127.0.0.1:$port/healthz")
  case "$body" in *'"state":"running"'*) state=running; break;; esac
  sleep 1
done
echo "healthz: $body"
if [ "$state" = running ]; then
  # As the sandbox user, over the group-1000 socket.
  sv=$(docker exec -u enzarb "$name" docker version --format '{{.Server.Version}}')
  case "$sv" in 29.*) echo "ok: docker server $sv reachable as enzarb";; *) echo "FAIL: docker server version '$sv'"; exit 1;; esac
  # Needs registry access; do not fail the smoke on a flaky network.
  if docker exec -u enzarb "$name" docker run --rm hello-world >/dev/null 2>&1; then
    echo "ok: docker run hello-world"
  else
    echo "skip: docker run hello-world failed (registry unreachable?)"
  fi
else
  # The runner cannot host dockerd: it must say so, and the sandbox must still be fully usable.
  case "$body" in *'"state":"failed"'*) ;; *) echo "FAIL: dockerd neither running nor reported failed: $body"; exit 1;; esac
  echo "skip: dockerd cannot start on this runner; Docker server checks skipped"
  docker logs "$name" 2>&1 | grep -i dockerd | tail -5 || true
fi
echo "smoke passed"
