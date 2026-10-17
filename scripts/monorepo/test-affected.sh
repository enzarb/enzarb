#!/usr/bin/env bash
# Tests for scripts/monorepo/affected.sh and scripts/monorepo/changes.sh.
#
#   1. sample diffs -> expected components;
#   2. the ci-*.yml files carry no `paths:` filters, run the `changes` job for their own component and
#      gate every job on it (docs/ci.md, "Changes job");
#   3. changes.sh picks the right components for a diff and runs everything when the base is unknown.

set -euo pipefail

cd "$(dirname "$0")/../.."
AFFECTED=scripts/monorepo/affected.sh
fail=0

affected() { printf '%s\n' "$@" | bash "$AFFECTED" --files-from - | paste -sd' ' -; }

check() { # check <description> <expected> <paths...>
  local what=$1 expected=$2 got
  shift 2
  got=$(affected "$@")
  if [ "$got" != "$expected" ]; then
    echo "FAIL: $what: expected [$expected], got [$got]" >&2
    fail=1
  fi
}

# --- 1. sample diffs -------------------------------------------------------------------------
check "docs only" "" docs/architecture.md
check "root files" "" mise.toml lefthook.yml CLAUDE.md
check "generated graph is read by the monorepo checks" "monorepo" scripts/monorepo/graph.generated.sh
check "root cargo files select every rust component" "proto operator connectors workflows realtime sandbox-proxy sandbox-container updater monorepo" Cargo.lock
check "root toolchain file" "proto operator connectors workflows realtime sandbox-proxy sandbox-container updater" rust-toolchain.toml
check "workflows only" "workflows" workflows/crates/engine/src/lib.rs
check "proto fans out" "proto workflows sandbox-proxy sandbox-container ui" proto/crates/enzarb-proto/src/lib.rs
check "operator fans out" "operator workflows sandbox-proxy" operator/crates/enzarb-crds/src/lib.rs
check "connectors fans out" "connectors workflows" connectors/bin/receiver/src/main.rs
check "realtime only" "realtime" realtime/src/lib.rs
check "ui only" "ui" ui/src/routes/+page.svelte
check "engine prompts feed the ui tests" "workflows ui" workflows/crates/engine/tests/stdlib/prompts/plan.md
check "graph fixtures feed the ui tests" "connectors workflows ui" workflows/crates/flow/tests/fixtures/app/x.yaml
check "workflow examples feed the ui tests" "ui" docs/workflows/examples/issue.yaml
check "engine migrations feed the ui tests" "workflows ui" workflows/migrations/20261016000002_engine_api.sql
check "flow crate is compiled into connectors" "connectors workflows" workflows/crates/flow/src/lib.rs
check "helm only" "helm" helm/enzarb/values.yaml
check "dependency order" "proto operator workflows sandbox-proxy sandbox-container ui" ui/a.ts operator/b.rs proto/c.rs
check "shared rust workflow" "operator connectors workflows realtime sandbox-proxy sandbox-container updater monorepo" .github/workflows/_rust.yml
check "own ci file (also re-runs the drift check)" "ui monorepo" .github/workflows/ci-ui.yml
check "ci change runs the drift check" "monorepo" scripts/monorepo/affected.sh
check "release workflow only needs actionlint" "monorepo" .github/workflows/release.yml

# --- 2. the ci-*.yml files are wired to the graph ----------------------------------------------
# No hand-written `paths:` filters; each file runs the `changes` job for its own component and
# gates every job on it.
source scripts/monorepo/graph.generated.sh
have=
for wf in .github/workflows/ci-*.yml; do
  component=$(basename "$wf" .yml)
  component=${component#ci-}
  have="$have $component"
  if grep -q '^ *paths:' "$wf"; then
    echo "FAIL: $wf: has a paths: filter; selection comes from the graph (_changes.yml)" >&2
    fail=1
  fi
  if ! grep -q "^      component: $component\$" "$wf"; then
    echo "FAIL: $wf: the changes job does not pass component: $component" >&2
    fail=1
  fi
  jobs=$(awk '/^jobs:/ { in_jobs = 1; next } in_jobs && /^  [a-z][a-z-]*:$/ { n++ } END { print n + 0 }' "$wf")
  gated=$(grep -c "^    if: needs.changes.outputs.run == 'true'\$" "$wf" || true)
  if [ "$gated" -ne $((jobs - 1)) ]; then
    echo "FAIL: $wf: $gated of $((jobs - 1)) jobs are gated on needs.changes.outputs.run" >&2
    fail=1
  fi
done

# Every component of the graph has a ci-<component>.yml and vice versa.
known=$(printf '%s\n' "${COMPONENT_ORDER[@]}" | sort | paste -sd' ' -)
have=$(tr ' ' '\n' <<<"$have" | sed '/^$/d' | sort | paste -sd' ' -)
if [ "$known" != "$have" ]; then
  echo "FAIL: components in graph.generated.sh [$known] differ from ci-*.yml [$have]" >&2
  fail=1
fi

# --- 3. changes.sh (the _changes.yml decision) ------------------------------------------------
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
git init -q "$tmp/r"
(
  cd "$tmp/r"
  git config user.email t@t
  git config user.name t
  mkdir -p docs proto ui
  echo a >docs/a.md
  echo a >proto/a.rs
  echo a >ui/a.ts
  git add -A
  git commit -qm base
  echo b >>docs/a.md
  git commit -qam docs
  echo b >>proto/a.rs
  git commit -qam proto
)
cp -r scripts "$tmp/r/scripts"
cd "$tmp/r"
base=$(git rev-parse HEAD~2) docs=$(git rev-parse HEAD~1) head=$(git rev-parse HEAD)
changes() { bash scripts/monorepo/changes.sh "$@" 2>/dev/null; }
expect() { # expect <description> <expected> <component> <base> <head>
  local got
  got=$(changes "$3" "$4" "$5")
  [ "$got" = "run=$2" ] || { echo "FAIL: changes.sh $1: expected run=$2, got [$got]" >&2; fail=1; }
}
expect "docs-only diff selects nothing" false proto "$base" "$docs"
expect "docs-only diff selects no ui" false ui "$base" "$docs"
expect "proto change selects proto" true proto "$docs" "$head"
expect "proto change fans out to ui" true ui "$docs" "$head"
expect "proto change leaves helm alone" false helm "$docs" "$head"
expect "all-zero base runs everything" true helm 0000000000000000000000000000000000000000 "$head"
expect "empty base runs everything" true helm "" "$head"
expect "unreachable base runs everything" true helm 1111111111111111111111111111111111111111 "$head"

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "ok: affected.sh matches the sample diffs, ci-*.yml use the graph and changes.sh selects correctly"
