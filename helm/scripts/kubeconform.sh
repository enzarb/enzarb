#!/usr/bin/env bash
# Validate rendered manifests (and the vendored CRD) with kubeconform.
# CRD schemas come from the datree CRDs catalog; kinds missing from it are skipped.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
for s in default dev; do
  echo "== $s"
  "$here/scripts/render.sh" "$s" | kubeconform -strict -summary -ignore-missing-schemas \
    -schema-location default \
    -schema-location 'https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json'
done
echo "== crds/"
kubeconform -strict -summary -ignore-missing-schemas "$here"/enzarb/crds/*.yaml
