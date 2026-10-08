#!/bin/sh
# Runs in a privileged pod with the node root mounted at /host. Idempotent:
# installs pinned gVisor, registers the `runsc` containerd handler, restarts
# containerd only when something changed, labels the node, then sleeps.
set -eu
BIN_DIR=${BIN_DIR:-/usr/local/bin}     # must be writable on the host (not COS/Flatcar /usr)
CONF=/etc/containerd/config.toml
RUNSC_CONF=/etc/containerd/runsc.toml
H=/host
apk add --no-cache curl >/dev/null

case "$(uname -m)" in
  x86_64) ARCH=x86_64; WANT=$GVISOR_SHA512_X86_64 ;;
  aarch64) ARCH=aarch64; WANT=$GVISOR_SHA512_AARCH64 ;;
  *) echo "unsupported arch" >&2; exit 1 ;;
esac
restart=0

have=$("$H$BIN_DIR/runsc" --version 2>/dev/null | head -1 || true)
if [ "$have" != "runsc version release-$GVISOR_VERSION" ] || [ ! -x "$H$BIN_DIR/containerd-shim-runsc-v1" ]; then
  tmp=$(mktemp -d)
  curl -fsSL -o "$tmp/g.tar.bz2" \
    "https://github.com/google/gvisor/releases/download/release-$GVISOR_VERSION/gvisor-$ARCH.tar.bz2"
  echo "$WANT  $tmp/g.tar.bz2" | sha512sum -c - >/dev/null
  mkdir "$tmp/x"; tar -xjf "$tmp/g.tar.bz2" -C "$tmp/x"
  # runsc looks for gvisor-bin/ next to itself; install the whole bundle.
  rm -rf "$H$BIN_DIR/gvisor-bin"
  cp -r "$tmp/x/gvisor-bin" "$H$BIN_DIR/gvisor-bin"
  for f in runsc containerd-shim-runsc-v1; do
    cp "$tmp/x/$f" "$H$BIN_DIR/$f.new" && chmod 755 "$H$BIN_DIR/$f.new" && mv "$H$BIN_DIR/$f.new" "$H$BIN_DIR/$f"
  done
  rm -rf "$tmp"
  echo "installed gVisor $GVISOR_VERSION"
fi

# Keep identical to hack/kind-install-gvisor.sh (Docker-in-gVisor, ADR-0012).
want_toml='[runsc_config]
  net-raw = "true"'
if [ "$(cat "$H$RUNSC_CONF" 2>/dev/null || true)" != "$want_toml" ]; then
  printf '%s\n' "$want_toml" > "$H$RUNSC_CONF"
  restart=1
fi

if ! grep -q 'containerd.runtimes.runsc\]' "$H$CONF" 2>/dev/null \
   && ! grep -rqs 'containerd.runtimes.runsc\]' "$H/etc/containerd/conf.d" 2>/dev/null; then
  if grep -q '^version = 3' "$H$CONF"; then PLUGIN=io.containerd.cri.v1.runtime
  elif grep -q '^version = 2' "$H$CONF"; then PLUGIN=io.containerd.grpc.v1.cri
  else echo "$CONF: unknown config version" >&2; exit 1; fi
  cat >> "$H$CONF" <<TOML

# enzarb: gVisor (added by gvisor-installer)
[plugins."$PLUGIN".containerd.runtimes.runsc]
  runtime_type = "io.containerd.runsc.v1"
[plugins."$PLUGIN".containerd.runtimes.runsc.options]
  TypeUrl = "io.containerd.runsc.v1.options"
  ConfigPath = "$RUNSC_CONF"
TOML
  restart=1
fi

if [ "$restart" = 1 ]; then
  nsenter -t 1 -m -u -i -n -p -- systemctl restart containerd
  echo "containerd restarted"
fi

# Label the node (in-cluster API; strategic-merge patch).
TOKEN=$(cat /var/run/secrets/kubernetes.io/serviceaccount/token)
curl -fsS -X PATCH --cacert /var/run/secrets/kubernetes.io/serviceaccount/ca.crt \
  -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/strategic-merge-patch+json' \
  -d '{"metadata":{"labels":{"enzarb.dev/runtime-gvisor":"true"}}}' \
  "https://kubernetes.default.svc/api/v1/nodes/$NODE_NAME" >/dev/null
echo "node $NODE_NAME labelled enzarb.dev/runtime-gvisor=true"

while true; do sleep 3600; done
