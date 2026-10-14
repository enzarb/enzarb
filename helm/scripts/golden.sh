#!/usr/bin/env bash
# Diff `helm template` output against tests/golden/<scenario>.yaml.
# `scripts/golden.sh --update` rewrites the snapshots.
set -euo pipefail
# Snapshots carry placeholder component versions, see render.sh.
export RENDER_PIN_VERSIONS=1
here="$(cd "$(dirname "$0")/.." && pwd)"
rc=0
for s in default dev oldie updater-webhook realtime realtime-replicas realtime-off realtime-btp realtime-origins; do
  out="$here/tests/golden/$s.yaml"
  if [ "${1:-}" = "--update" ]; then
    "$here/scripts/render.sh" "$s" > "$out"; echo "updated $out"
  elif ! diff -u "$out" <("$here/scripts/render.sh" "$s"); then
    echo "golden mismatch for '$s' (run scripts/golden.sh --update if intended)" >&2; rc=1
  fi
done
exit $rc
