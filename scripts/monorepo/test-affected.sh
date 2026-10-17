#!/usr/bin/env bash
# Tests for scripts/monorepo/affected.sh, including the drift check against the `paths:` filters
# of .github/workflows/ci-*.yml (docs/ci.md, "Affected components").
#
#   1. sample diffs -> expected components;
#   2. for every path filter in every ci-*.yml, a probe path matching it must map to that
#      component, and for every probe path (taken from all filters) affected.sh must print exactly
#      the components whose CI filters match it. Drift in either direction fails.
#
# The workflow files are parsed without yq: `on.push.paths` and `on.pull_request.paths` are block
# lists and must be identical (no YAML anchors: actionlint rejects them).
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
check "root files" "" lefthook.yml CLAUDE.md
check "tool pins feed the pin check" "monorepo" mise.toml
check "component Dockerfile ARGs feed the pin check" "sandbox-container monorepo" sandbox-container/Dockerfile
check "workflows only" "workflows" workflows/crates/engine/src/lib.rs
check "proto fans out" "proto workflows sandbox-proxy sandbox-container ui" proto/crates/enzarb-proto/src/lib.rs
check "operator fans out" "operator workflows sandbox-proxy" operator/crates/enzarb-crds/src/lib.rs
check "connectors fans out" "connectors workflows" connectors/bin/receiver/src/main.rs
check "realtime only" "realtime" realtime/src/lib.rs
check "ui only" "ui" ui/src/routes/+page.svelte
check "engine migrations feed the ui tests" "workflows ui" workflows/migrations/20261016000002_engine_api.sql
check "flow crate is compiled into connectors" "connectors workflows" workflows/crates/flow/src/lib.rs
check "helm only" "helm" helm/enzarb/values.yaml
check "dependency order" "proto operator workflows sandbox-proxy sandbox-container ui" ui/a.ts operator/b.rs proto/c.rs
check "shared rust workflow" "operator connectors workflows realtime sandbox-proxy sandbox-container updater monorepo" .github/workflows/_rust.yml
check "own ci file (also re-runs the drift check)" "ui monorepo" .github/workflows/ci-ui.yml
check "ui/proto/sandbox node setup action" "proto sandbox-container ui" .github/actions/setup-node/action.yml
check "ci change runs the drift check" "monorepo" scripts/monorepo/affected.sh
check "release workflow only needs actionlint" "monorepo" .github/workflows/release.yml

# --- 2. drift against the CI path filters -----------------------------------------------------
# Print the `on.push.paths` entries of a workflow, one per line (quotes stripped).
push_paths() {
  awk '
    /^  push:/ { in_push = 1; next }
    /^  [a-z_]+:/ { in_push = 0 }
    in_push && /^    paths:/ { listing = 1; next }
    listing && /^      - / { sub(/^      - /, ""); gsub(/^'\''|'\''$|^"|"$/, ""); print; next }
    listing { listing = 0 }
  ' "$1"
}

# The pull_request filter must be the same list (alias `*paths`, or an identical list).
pr_paths() {
  awk '
    /^  pull_request:/ { in_pr = 1; next }
    /^  [a-z_]+:/ { in_pr = 0 }
    in_pr && /^    paths: \*/ { print "ALIAS"; next }
    in_pr && /^    paths:/ { listing = 1; next }
    listing && /^      - / { sub(/^      - /, ""); gsub(/^'\''|'\''$|^"|"$/, ""); print; next }
    listing { listing = 0 }
  ' "$1"
}

probe() { # a concrete path matched by a filter glob
  local p=${1//\*\*/probe\/x.txt}
  printf '%s\n' "${p//\*/x}"
}

declare -A ci_globs=() # component -> newline-separated globs
probes=()
for wf in .github/workflows/ci-*.yml; do
  component=$(basename "$wf" .yml)
  component=${component#ci-}
  globs=$(push_paths "$wf")
  if [ -z "$globs" ]; then
    echo "FAIL: $wf: no on.push.paths found" >&2
    fail=1
    continue
  fi
  pr=$(pr_paths "$wf")
  if [ "$pr" != "ALIAS" ] && [ "$pr" != "$globs" ]; then
    echo "FAIL: $wf: pull_request.paths differs from push.paths" >&2
    fail=1
  fi
  ci_globs[$component]=$globs
  while IFS= read -r g; do
    probes+=("$(probe "$g")")
  done <<<"$globs"
done

for path in $(printf '%s\n' "${probes[@]}" | sort -u); do
  expected=
  for component in proto operator connectors workflows realtime sandbox-proxy sandbox-container ui updater kubernetes helm monorepo; do
    [ -n "${ci_globs[$component]:-}" ] || continue
    while IFS= read -r g; do
      # shellcheck disable=SC2254 # the glob is meant to be a pattern
      case "$path" in
        $g)
          expected="$expected $component"
          break
          ;;
      esac
    done <<<"${ci_globs[$component]}"
  done
  check "CI filters vs affected.sh for $path" "${expected# }" "$path"
done

# Every component with a ci-<component>.yml must be known to affected.sh (and vice versa).
known=$(grep -o "^  '[a-z-]*|" "$AFFECTED" | tr -d " '|" | sort | paste -sd' ' -)
have=$(for c in "${!ci_globs[@]}"; do echo "$c"; done | sort | paste -sd' ' -)
if [ "$known" != "$have" ]; then
  echo "FAIL: components in affected.sh [$known] differ from ci-*.yml [$have]" >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "ok: affected.sh matches the sample diffs and the ci-*.yml path filters"
