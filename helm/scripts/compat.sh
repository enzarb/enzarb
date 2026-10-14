#!/usr/bin/env bash
# Lint and template the current chart against the previous released chart's values files, so a
# values key removed in this change but still set by an existing deploy fails CI (docs/ci.md,
# "Values compatibility"). Env: COMPAT_TAG_PATTERN (default 'helm/v*') selects the baseline tags.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
pattern="${COMPAT_TAG_PATTERN:-helm/v*}"

tag="$(git tag --list "$pattern" --sort=-v:refname | head -1)"
if [ -z "$tag" ]; then
  echo "compat: no $pattern tag, skipping"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
prefix="$(git rev-parse --show-prefix)"
fail=0
for f in values.yaml values-oldie.yaml; do
  git show "$tag:${prefix}enzarb/$f" > "$tmp/$f"
  for cmd in lint template; do
    if [ "$cmd" = lint ]; then
      args=(lint "$here/enzarb")
    else
      args=(template enzarb "$here/enzarb" --namespace enzarb)
    fi
    if ! out="$(helm "${args[@]}" -f "$tmp/$f" --set 'sandbox.egress.clusterCIDRs={10.0.0.0/8}' 2>&1)"; then
      echo "compat: helm $cmd with $tag:$f failed:" >&2
      echo "$out" | head -20 >&2
      fail=1
    fi
  done
done
if [ "$fail" != 0 ]; then
  echo "compat: the chart no longer accepts values from $tag. Do not remove a values key in one release: keep it in values.schema.json as a DEPRECATED no-op (docs/ci.md, Values compatibility)." >&2
  exit 1
fi
echo "compat ok ($tag)"
