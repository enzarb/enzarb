#!/usr/bin/env bash
# Install gVisor (runsc + containerd-shim-runsc-v1) into every node of a kind
# cluster, register the `runsc` containerd runtime handler, and label the nodes
# enzarb.dev/runtime-gvisor=true (the nodeSelector of RuntimeClass `gvisor`).
# Idempotent: re-running is a no-op once nodes are up to date.
# Usage: hack/kind-install-gvisor.sh [cluster-name]   (default: enzarb)
# Needs: docker, kubectl, curl, sha512sum, tar (bzip2). Pinned in hack/versions.env.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=hack/versions.env
source hack/versions.env

CLUSTER=${1:-enzarb}
CACHE=${XDG_CACHE_HOME:-$HOME/.cache}/enzarb/gvisor/$GVISOR_VERSION
BIN_DIR=/usr/local/bin
RUNSC_CONF=/etc/containerd/runsc.toml
CONTAINERD_CONF=/etc/containerd/config.toml
LABEL=enzarb.dev/runtime-gvisor=true

# runsc.toml is shared with the installer image (installer/Dockerfile).
runsc_toml() { cat hack/runsc.toml; }

# Appended to containerd's config. kind's node image ships a `version = 2`
# config (containerd 2.x still reads it, plugin id io.containerd.grpc.v1.cri).
containerd_block() {
  cat <<TOML

# enzarb: gVisor (added by hack/kind-install-gvisor.sh)
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]
  runtime_type = "io.containerd.runsc.v1"
[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc.options]
  TypeUrl = "io.containerd.runsc.v1.options"
  ConfigPath = "$RUNSC_CONF"
TOML
}

mapfile -t NODES < <(docker ps --filter "label=io.x-k8s.kind.cluster=$CLUSTER" --format '{{.Names}}' | sort)
[ "${#NODES[@]}" -gt 0 ] || { echo "no kind node containers for cluster '$CLUSTER'" >&2; exit 1; }

# Download + verify + unpack once on the host.
case "$(docker exec "${NODES[0]}" uname -m)" in
  x86_64) ARCH=x86_64; WANT=$GVISOR_SHA512_X86_64 ;;
  aarch64|arm64) ARCH=aarch64; WANT=$GVISOR_SHA512_AARCH64 ;;
  *) echo "unsupported node architecture" >&2; exit 1 ;;
esac
STAGE=$CACHE/$ARCH
if [ ! -x "$STAGE/runsc" ]; then
  mkdir -p "$CACHE"
  tarball=$CACHE/gvisor-$ARCH.tar.bz2
  curl -fsSL -o "$tarball" \
    "https://github.com/google/gvisor/releases/download/release-$GVISOR_VERSION/gvisor-$ARCH.tar.bz2"
  echo "$WANT  $tarball" | sha512sum -c - >/dev/null || { rm -f "$tarball"; echo "sha512 mismatch for $tarball" >&2; exit 1; }
  rm -rf "$STAGE.tmp"; mkdir -p "$STAGE.tmp"
  tar -xjf "$tarball" -C "$STAGE.tmp"
  rm -rf "$STAGE"; mv "$STAGE.tmp" "$STAGE"
fi

for node in "${NODES[@]}"; do
  echo "== $node"
  restart=0

  have=$(docker exec "$node" sh -c "$BIN_DIR/runsc --version 2>/dev/null | head -1" || true)
  if [ "$have" != "runsc version release-$GVISOR_VERSION" ] \
     || ! docker exec "$node" test -x "$BIN_DIR/containerd-shim-runsc-v1"; then
    # runsc looks for gvisor-bin/ next to itself; copy the whole bundle.
    docker cp "$STAGE/." "$node:$BIN_DIR/"
    echo "installed gVisor $GVISOR_VERSION"
  fi

  if [ "$(docker exec "$node" cat "$RUNSC_CONF" 2>/dev/null || true)" != "$(runsc_toml)" ]; then
    runsc_toml | docker exec -i "$node" sh -c "cat > $RUNSC_CONF"
    restart=1
  fi

  if ! docker exec "$node" grep -q 'containerd.runtimes.runsc\]' "$CONTAINERD_CONF"; then
    docker exec "$node" grep -q '^version = 2' "$CONTAINERD_CONF" \
      || { echo "$CONTAINERD_CONF is not 'version = 2'; update containerd_block()" >&2; exit 1; }
    containerd_block | docker exec -i "$node" sh -c "cat >> $CONTAINERD_CONF"
    restart=1
  fi

  if [ "$restart" = 1 ]; then
    docker exec "$node" systemctl restart containerd
    for _ in $(seq 1 30); do
      docker exec "$node" ctr version >/dev/null 2>&1 && break
      sleep 1
    done
    docker exec "$node" ctr version >/dev/null || { echo "containerd did not come back on $node" >&2; exit 1; }
    echo "containerd configured and restarted"
  fi

  kubectl --context "kind-$CLUSTER" label node "$node" "$LABEL" --overwrite
done
echo "done. Verify: kubectl --context kind-$CLUSTER apply -k overlays/dev && see README 'gVisor in the dev cluster'"
