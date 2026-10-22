#!/usr/bin/env bash
# lefthook pre-push: run the checks of the components affected by the commits being pushed
# (scripts/monorepo/affected.sh). Keep the commands in sync with docs/ci.md ("Git hooks").
#
# Base: the upstream branch, else origin/main. Postgres suites (connectors, realtime)
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

# run <dir> <command...>: run a check inside a directory (`.` for the root Cargo workspace, whose
# packages are picked with -p).
run() {
  local dir=$1
  shift
  echo "pre-push: [$dir] $*"
  (cd "$dir" && "$@")
}

# JS checks need installed dependencies; install them with `pnpm install --frozen-lockfile` when missing rather than
# skipping the checks (a skip let a lint failure through to CI).
npm_ready() {
  [ -d "$1/node_modules" ] || {
    echo "pre-push: $1: node_modules missing, running pnpm install"
    run "$1" pnpm install --frozen-lockfile || return 1
  }
}

for c in $components; do
  case "$c" in
    proto)
      run . cargo clippy -p enzarb-proto --all-targets -- -D warnings
      run . cargo test -p enzarb-proto
      run proto bash ../.mise/tasks/proto/gen --check
      ;;
    operator)
      run . cargo clippy -p enzarb-crds -p enzarb-sandbox-operator --all-targets --all-features -- -D warnings
      run . cargo test -p enzarb-crds -p enzarb-sandbox-operator --all-features
      ;;
    connectors)
      run . cargo clippy -p enzarb-receiver --all-targets --all-features -- -D warnings
      run . cargo test -p enzarb-receiver --all-features
      ;;
    realtime)
      run . cargo clippy -p enzarb-realtime --all-targets -- -D warnings
      run . cargo test -p enzarb-realtime
      ;;
    sandbox-proxy)
      run . cargo clippy -p enzarb-sandbox-proxy --all-targets -- -D warnings
      run . cargo test -p enzarb-sandbox-proxy
      ;;
    updater)
      run . cargo clippy -p enzarb-updater --all-targets -- -D warnings
      run . cargo test -p enzarb-updater
      ;;
    tracks)
      run . cargo clippy -p enzarb-tracks --all-targets -- -D warnings
      run . cargo test -p enzarb-tracks
      ;;
    ez)
      run . cargo clippy -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow --all-targets -- -D warnings
      run . cargo test -p ez-acp -p ez-claude -p ez-cli -p ez-core -p ez-hooks -p ez-llm -p ez-mcp -p ez-mise -p ez-tools -p ez-workflow
      ;;
    sandbox-container)
      run . cargo clippy -p sandbox-init --all-targets --locked -- -D warnings
      run . cargo test -p sandbox-init --locked
      ;;
    ui)
      if npm_ready ui; then
        run ui pnpm run lint
        run ui pnpm run check
        run ui pnpm run test:unit --run --project server
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
      bash scripts/monorepo/gen-graph.sh --check
      bash scripts/monorepo/test-affected.sh
      ;;
  esac
done
