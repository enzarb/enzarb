#!/usr/bin/env bash
# Render every overlay and validate with kubeconform. Needs kubectl + kubeconform
# on PATH and network access (remote kustomize bases, schema catalogs).
# CRD instances use the datree CRDs-catalog; kinds without a schema are skipped.
# CapsuleConfiguration is skipped because the catalog schema lags the vendored CRD.
set -euo pipefail
cd "$(dirname "$0")/.."
KUBERNETES_VERSION=${KUBERNETES_VERSION:-1.34.0}
for overlay in overlays/*/; do
  echo "== ${overlay}"
  kubectl kustomize "$overlay" > /dev/null
  kubectl kustomize "$overlay" | kubeconform -strict -summary \
    -kubernetes-version "$KUBERNETES_VERSION" \
    -ignore-missing-schemas -skip CapsuleConfiguration \
    -schema-location default \
    -schema-location 'https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json' -
done
# Each staged base from the README must render on its own.
for d in base/*/ base/envoy-gateway/install base/envoy-gateway/config base/capsule/crds base/pgop/crds; do
  kubectl kustomize "$d" > /dev/null && echo "rendered $d"
done
# Shell scripts must pass shellcheck (pinned in mise.toml).
shellcheck -x -P .:hack hack/*.sh installer/install.sh && echo "shellcheck ok"
# The installer image gets its gVisor pin from versions.env at build time (build args);
# the Dockerfile must not carry defaults that could drift.
if grep -nE '^ARG GVISOR_[A-Z0-9_]+=' installer/Dockerfile; then
  echo "installer/Dockerfile must not default GVISOR_* build args" >&2; exit 1
fi
echo "installer pins come from versions.env only"
