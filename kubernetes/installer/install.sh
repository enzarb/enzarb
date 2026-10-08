#!/bin/sh
# Entrypoint of the enzarb-gvisor-installer image (installer/Dockerfile). Runs in a
# privileged pod with the node root mounted at /host. No network access beyond the
# in-cluster API: the sha512-verified gVisor bundle is baked into the image at
# $BUNDLE_DIR, and runsc.toml at $RUNSC_TOML (hack/runsc.toml). Idempotent:
# installs pinned gVisor, registers the `runsc` containerd handler, restarts
# containerd only when something changed, labels the node, then sleeps.
set -eu
BIN_DIR=${BIN_DIR:-/usr/local/bin}     # must be writable on the host (not COS/Flatcar /usr)
CONF=/etc/containerd/config.toml
RUNSC_CONF=/etc/containerd/runsc.toml
BUNDLE_DIR=${BUNDLE_DIR:-/opt/gvisor}  # runsc, containerd-shim-runsc-v1, gvisor-bin/
RUNSC_TOML=${RUNSC_TOML:-/opt/enzarb/runsc.toml}
: "${GVISOR_VERSION:?set by the image}"
H=/host
restart=0

have=$("$H$BIN_DIR/runsc" --version 2>/dev/null | head -1 || true)
if [ "$have" != "runsc version release-$GVISOR_VERSION" ] || [ ! -x "$H$BIN_DIR/containerd-shim-runsc-v1" ]; then
  # runsc looks for gvisor-bin/ next to itself; install the whole bundle.
  rm -rf "$H$BIN_DIR/gvisor-bin"
  cp -r "$BUNDLE_DIR/gvisor-bin" "$H$BIN_DIR/gvisor-bin"
  for f in runsc containerd-shim-runsc-v1; do
    cp "$BUNDLE_DIR/$f" "$H$BIN_DIR/$f.new" && chmod 755 "$H$BIN_DIR/$f.new" && mv "$H$BIN_DIR/$f.new" "$H$BIN_DIR/$f"
  done
  echo "installed gVisor $GVISOR_VERSION"
fi

if ! cmp -s "$RUNSC_TOML" "$H$RUNSC_CONF" 2>/dev/null; then
  cp "$RUNSC_TOML" "$H$RUNSC_CONF"
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
