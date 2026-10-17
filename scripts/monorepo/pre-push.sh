#!/usr/bin/env bash
# lefthook pre-push: run the checks of the components affected by the commits being pushed
# (scripts/monorepo/affected.sh). Keep the commands in sync with docs/ci.md ("Git hooks").
#
# Base: the upstream branch, else origin/main. Postgres suites (workflows, connectors, realtime)
# use ENZARB_TEST_PG_URL when it is exported (`eval "$(mise run test:pg)"`), else one container
# per test.
set -euo pipefail

cd "$(dirname "$0")/../.."

base=origin/main
if upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null); then
  base=$upstream
fi

components=$(bash scripts/monorepo/affected.sh "$base")
if [ -z "$components" ]; then
  echo "pre-push: no component affected relative to $base"
  exit 0
fi
echo "pre-push: affected relative to $base: $(echo "$components" | paste -sd' ' -)"

# run <dir> <command...>: run a check inside a component directory.
run() {
  local dir=$1
  shift
  echo "pre-push: [$dir] $*"
  (cd "$dir" && "$@")
}

# JS checks need installed dependencies; install them with `npm ci` when missing rather than
# skipping the checks (a skip let a lint failure through to CI).
npm_ready() {
  [ -d "$1/node_modules" ] || {
    echo "pre-push: $1: node_modules missing, running npm ci"
    run "$1" npm ci || return 1
  }
}

for c in $components; do
  case "$c" in
    proto)
      run proto cargo clippy --all-targets -- -D warnings
      run proto cargo test
      run proto bash ../.mise/tasks/proto/gen --check
      ;;
    operator)
      run operator cargo clippy --all-targets --all-features -- -D warnings
      run operator cargo test --all-features
      ;;
    connectors)
      run connectors cargo clippy --all-targets --workspace --all-features -- -D warnings
      run connectors cargo test --workspace --all-features
      ;;
    workflows)
      run workflows cargo clippy --all-targets --workspace -- -D warnings
      run workflows cargo test --workspace
      ;;
    realtime | sandbox-proxy | updater)
      run "$c" cargo clippy --all-targets -- -D warnings
      run "$c" cargo test
      ;;
    sandbox-container)
      run sandbox-container cargo clippy --all-targets --locked -- -D warnings
      run sandbox-container cargo test --locked
      ;;
    ui)
      if npm_ready ui; then
        run ui npm run lint
        run ui npm run check
        run ui npm run test:unit -- --run --project server
      fi
      ;;
    helm)
      run helm mise run compat
      ;;
    kubernetes)
      # Needs a cluster-flavoured toolchain; CI only (as before).
      echo "pre-push: kubernetes: checked in CI only"
      ;;
    monorepo)
      bash scripts/monorepo/test-affected.sh
      ;;
  esac
done
