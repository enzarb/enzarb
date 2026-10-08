#!/usr/bin/env bash
# Smoke test for the sandbox image. Usage: scripts/smoke.sh [image]
set -euo pipefail

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
docker run -d --name "$name" -p 127.0.0.1::7070 \
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
check claude-agent-acp --version
check claude --version
check git --version
check node --version
check mise --version
check cc --version
check pkg-config --exists openssl
# mise shims must be on PATH for agents and shells.
docker exec "$name" sh -c 'echo "$PATH" | grep -q "/workspace/.toolchains/mise/shims"'
echo "ok: mise shims on PATH"
# Nothing else is baked in: toolchains come from mise.
for bin in cargo rustc rustup npm npx curl rg jq; do
  if docker exec "$name" sh -c "command -v $bin" >/dev/null 2>&1; then
    echo "FAIL: $bin must not be in the image"; exit 1
  fi
  echo "ok: no $bin"
done
uid=$(docker exec "$name" id -u)
[ "$uid" = 1000 ] || { echo "FAIL: uid is $uid, want 1000"; exit 1; }
echo "ok: uid 1000"
docker exec "$name" sh -c 'touch /workspace/.enzarb/.w "$HOME/.w" && rm /workspace/.enzarb/.w "$HOME/.w"'
echo "ok: /workspace and HOME writable"
echo "smoke passed"
