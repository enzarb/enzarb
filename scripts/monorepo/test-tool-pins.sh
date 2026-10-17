#!/usr/bin/env bash
# Tests for scripts/monorepo/check-tool-pins.sh against temp-directory copies of the real inputs.
set -euo pipefail

cd "$(dirname "$0")/../.."
CHECK=$PWD/scripts/monorepo/check-tool-pins.sh
src=$PWD
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0

# fresh <dir>: a copy of the files the check reads.
fresh() {
  mkdir -p "$1"
  (cd "$src" && cp --parents mise.toml ./*/mise.toml ./*/rust-toolchain.toml ./*/Dockerfile kubernetes/installer/Dockerfile \
    kubernetes/hack/versions.env sandbox-container/scripts/check-playwright-version.sh "$1") 2>/dev/null || true
  mkdir -p "$1/ui"
  cp "$src/ui/package-lock.json" "$1/ui/"
}

expect() { # expect <pass|fail> <description> <dir>
  local want=$1 what=$2 dir=$3 rc=0
  bash "$CHECK" "$dir" >/dev/null 2>&1 || rc=$?
  if { [ "$want" = pass ] && [ "$rc" -ne 0 ]; } || { [ "$want" = fail ] && [ "$rc" -eq 0 ]; }; then
    echo "FAIL: $what: expected $want, exit $rc" >&2
    fail=1
  fi
}

fresh "$tmp/ok"
expect pass "unmodified copy" "$tmp/ok"

fresh "$tmp/node"
sed -i 's/^node = .*/node = "24.99.0"/' "$tmp/node/mise.toml"
expect fail "root node bumped without the Dockerfile ARG" "$tmp/node"

fresh "$tmp/arg"
sed -i 's/^ARG GH_VERSION=.*/ARG GH_VERSION=0.0.1/' "$tmp/arg/sandbox-container/Dockerfile"
expect fail "Dockerfile ARG changed without the root pin" "$tmp/arg"

fresh "$tmp/tools"
printf '[tools]\nnode = "24.21.0"\n' >>"$tmp/tools/ui/mise.toml"
expect fail "[tools] in a component mise.toml" "$tmp/tools"

fresh "$tmp/latest"
sed -i 's/^kind = .*/kind = "latest"/' "$tmp/latest/mise.toml"
expect fail "latest pin" "$tmp/latest"

fresh "$tmp/newarg"
sed -i '0,/^ARG NODE_VERSION=.*/s//&\nARG NEWTOOL_VERSION=1.0.0/' "$tmp/newarg/sandbox-container/Dockerfile"
expect fail "unmapped new ARG *_VERSION" "$tmp/newarg"

fresh "$tmp/rust"
sed -i 's/^rust = .*/rust = "1.0.0"/' "$tmp/rust/mise.toml"
expect fail "root rust differs from rust-toolchain.toml" "$tmp/rust"

fresh "$tmp/helm"
sed -i 's/^HELM_VERSION=.*/HELM_VERSION=v9.9.9/' "$tmp/helm/kubernetes/hack/versions.env"
expect fail "HELM_VERSION differs from the root helm" "$tmp/helm"

[ "$fail" -eq 0 ] || exit 1
echo "ok: check-tool-pins.sh passes the real inputs and fails on drift"
