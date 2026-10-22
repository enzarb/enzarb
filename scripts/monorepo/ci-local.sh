#!/usr/bin/env bash
# Runs the GitHub CI checks locally with the same commands and flags as .github/workflows/ci-*.yml
# and the reusable _rust.yml / _docker.yml. Prints a PASS/FAIL table; exits 1 on any failure.
#
#   scripts/monorepo/ci-local.sh                 # everything
#   scripts/monorepo/ci-local.sh realtime ui    # selected groups
# Groups: monorepo proto operator connectors realtime sandbox-proxy sandbox-container
#         updater ui helm kubernetes docker
# Needs docker (shared Postgres, actionlint, image builds). nextest comes from mise
# (cargo:cargo-nextest). Skipped (CI only): kubernetes ansible-lint, vendored-drift, image smoke.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
ROOT=$PWD
LOG=${CI_LOCAL_LOG:-$ROOT/target/ci-local}
mkdir -p "$LOG"
results=()

step() { # step <name> <dir> <command...>
  local name=$1 dir=$2
  shift 2
  local f="$LOG/${name//[^A-Za-z0-9_.-]/_}.log"
  echo ">> [$name] (cd $dir && $*)"
  if (cd "$dir" && "$@") >"$f" 2>&1; then
    results+=("PASS  $name")
  else
    results+=("FAIL  $name  (log: $f)")
    tail -30 "$f"
  fi
}

nextest() { mise exec cargo:cargo-nextest@latest -- cargo nextest "$@"; }

rust() { # rust <name> <cargo-args...>   (_rust.yml: fmt, check-cargo-deps, clippy -D warnings, nextest, doctests; one root workspace, packages picked with -p)
  local name=$1
  shift
  step "$name fmt" . cargo fmt --all --check
  step "$name clippy" . cargo clippy --all-targets "$@" -- -D warnings
  step "$name nextest" . nextest run "$@"
  step "$name doctests" . cargo test --doc "$@"
}

pg() { [ -n "${ENZARB_TEST_PG_URL:-}" ] || eval "$(mise run test:pg)"; export ENZARB_TEST_PG_URL; }

groups=${*:-monorepo proto operator connectors realtime sandbox-proxy sandbox-container updater ui helm kubernetes docker}
for g in $groups; do
  case $g in
    monorepo)
      step "check-cargo-deps" . scripts/monorepo/check-cargo-deps.sh
      step "graph-fresh" . scripts/monorepo/gen-graph.sh --check
      step "test-graph" . scripts/monorepo/test-graph.sh
      step "test-affected" . scripts/monorepo/test-affected.sh
      step "test-mechanical" . bash scripts/monorepo/test-mechanical.sh
      step "check-tool-pins" . scripts/monorepo/check-tool-pins.sh
      step "test-tool-pins" . bash scripts/monorepo/test-tool-pins.sh
      step "shellcheck" . shellcheck --exclude=SC1091 scripts/monorepo/*.sh .github/scripts/*.sh .mise/tasks/test/pg .mise/tasks/graph/gen
      step "actionlint" . docker run --rm -v "$ROOT:/repo" -w /repo rhysd/actionlint:1.7.7 -color
      ;;
    proto)
      step "proto fmt" . cargo fmt --all --check
      step "proto clippy" . cargo clippy -p enzarb-proto --all-targets -- -D warnings
      step "proto ctrl.gen.ts" proto bash ../.mise/tasks/proto/gen --check
      step "proto clippy all-features" . cargo clippy -p enzarb-proto --all-targets --all-features -- -D warnings
      step "proto nextest" . nextest run -p enzarb-proto
      step "proto nextest all-features" . nextest run -p enzarb-proto --all-features
      step "proto doctests" . cargo test -p enzarb-proto --doc --all-features
      step "proto npm ci" proto npm ci
      step "proto npm check" proto npm run check
      step "proto npm test" proto npm test
      ;;
    operator) rust operator -p enzarb-crds -p enzarb-sandbox-operator --all-features ;;
    connectors) pg; rust connectors -p enzarb-receiver --all-features ;;
    realtime) pg; rust realtime -p enzarb-realtime ;;
    sandbox-proxy) rust sandbox-proxy -p enzarb-sandbox-proxy ;;
    sandbox-container)
      step "sandbox-container playwright version" . bash sandbox-container/scripts/check-playwright-version.sh
      rust sandbox-container --locked -p sandbox-init
      ;;
    updater) rust updater -p enzarb-updater ;;
    ui)
      step "ui npm ci" ui npm ci
      step "ui check" ui npm run check
      step "ui lint" ui npm run lint
      step "ui test client" ui npm run test:unit -- --run --project client
      step "ui test server" ui npm run test:unit -- --run --project server
      step "ui e2e" ui npm run test:e2e
      step "ui e2e prod" ui npm run test:e2e:prod
      ;;
    helm)
      for t in lint compat golden componentversions npmtoken kubeconform pgop; do
        step "helm $t" helm mise run "$t"
      done
      ;;
    kubernetes) step "kubernetes validate" kubernetes ./hack/validate.sh ;;
    docker)
      for d in operator connectors realtime sandbox-proxy sandbox-container ui updater; do
        ctx=$(bash .github/scripts/image-components.sh "$d" | jq -r .context)
        step "docker $d" . docker build -q -f "$d/Dockerfile" -t "ci-local/$d" "$ctx"
      done
      ;;
    *) echo "unknown group $g" >&2; exit 2 ;;
  esac
done

echo
printf '%s\n' "${results[@]}"
! printf '%s\n' "${results[@]}" | grep -q '^FAIL'
