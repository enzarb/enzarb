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
shellcheck -x -P .:hack hack/*.sh base/gvisor-installer/install.sh && echo "shellcheck ok"
# The installer base duplicates the gVisor pin; it must match versions.env.
# shellcheck source=versions.env
source hack/versions.env
for pin in "GVISOR_VERSION=$GVISOR_VERSION" "GVISOR_SHA512_X86_64=$GVISOR_SHA512_X86_64" \
           "GVISOR_SHA512_AARCH64=$GVISOR_SHA512_AARCH64"; do
  grep -q "$pin" base/gvisor-installer/kustomization.yaml \
    || { echo "base/gvisor-installer pin != hack/versions.env ($pin)"; exit 1; }
done
echo "gvisor pins in sync"
