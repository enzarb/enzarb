#!/usr/bin/env bash
# Self-test for compat.sh: it must skip without a tag, pass for compatible values and fail when
# the previous release's values set a key the current schema rejects.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fail=0

repo="$tmp/repo"
mkdir -p "$repo/helm"
cp -r "$here/enzarb" "$repo/helm/enzarb"
cp -r "$here/scripts" "$repo/helm/scripts"
# helm is a mise shim: the temp repo needs the same (root) tool pins.
cp "$here/mise.toml" "$repo/helm/mise.toml"
cp "$here/../mise.toml" "$repo/mise.toml"
export MISE_TRUSTED_CONFIG_PATHS="$repo"
git -C "$repo" init -q
git -C "$repo" config user.email t@example.com
git -C "$repo" config user.name t
git -C "$repo" config commit.gpgsign false
git -C "$repo" add -A
git -C "$repo" commit -qm base

run() { (cd "$repo/helm" && bash scripts/compat.sh) 2>&1; }

if ! out="$(run)" || ! grep -q "no helm/v\* tag, skipping" <<<"$out"; then
  echo "assert-compat: expected a clean skip without a tag, got: $out" >&2; fail=1
fi

git -C "$repo" tag helm/v0.0.1
if ! out="$(run)"; then
  echo "assert-compat: expected pass for identical values, got: $out" >&2; fail=1
fi

# A tagged release whose oldie values set a key the schema rejects. The schema root accepts unknown
# keys, so the key goes under `ui`, which is strict (values-oldie.yaml has no `ui` block).
printf '\nui:\n  bogusRemovedKey: true\n' >> "$repo/helm/enzarb/values-oldie.yaml"
git -C "$repo" commit -qam "old values with a removed key"
git -C "$repo" tag helm/v0.0.2
if out="$(run)"; then
  echo "assert-compat: expected failure for a key the schema rejects, got: $out" >&2; fail=1
elif ! grep -q "DEPRECATED" <<<"$out"; then
  echo "assert-compat: failure lacks the deprecation hint: $out" >&2; fail=1
fi

exit "$fail"
