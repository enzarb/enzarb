#!/usr/bin/env bash
# Print the components affected by a diff, one per line, in dependency order.
#
#   scripts/monorepo/affected.sh <base>            # git diff --name-only <base>...HEAD
#   scripts/monorepo/affected.sh --files-from -    # newline-separated paths on stdin
#   scripts/monorepo/affected.sh --files-from FILE
#
# The table below is the single source of truth for "which checks does this change need". It
# mirrors the `paths:` filters of .github/workflows/ci-<component>.yml (including downstream
# components via path deps, e.g. `proto/**` triggers ui); scripts/monorepo/test-affected.sh fails
# when the two drift apart. Files matching no row (docs/**, root files such as mise.toml or
# lefthook.yml) trigger no component CI, so they print nothing. `monorepo` is the pseudo-component
# for the repo plumbing itself (scripts, mise tasks, any workflow file): shellcheck, actionlint
# and the drift test, see .github/workflows/ci-monorepo.yml.
set -euo pipefail

# component|space-separated globs (bash `case` patterns; `*` also matches `/`)
RULES=(
  'proto|proto/** .github/workflows/ci-proto.yml'
  'operator|operator/** .github/workflows/ci-operator.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'connectors|connectors/** .github/workflows/ci-connectors.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'workflows|workflows/** operator/** connectors/** proto/** .github/workflows/ci-workflows.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'realtime|realtime/** .github/workflows/ci-realtime.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'sandbox-proxy|sandbox-proxy/** proto/** operator/** .github/workflows/ci-sandbox-proxy.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'sandbox-container|sandbox-container/** proto/** ui/package-lock.json .github/workflows/ci-sandbox-container.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'ui|ui/** proto/** workflows/crates/graph/tests/fixtures/** workflows/crates/engine/prompts/** workflows/crates/engine/src/machine/prompt.rs docs/workflows/examples/** .github/workflows/ci-ui.yml .github/workflows/_docker.yml .github/actions/ui-install/**'
  'updater|updater/** .github/workflows/ci-updater.yml .github/workflows/_rust.yml .github/workflows/_docker.yml'
  'kubernetes|kubernetes/** .github/workflows/ci-kubernetes.yml'
  'helm|helm/** .github/workflows/ci-helm.yml'
  'monorepo|scripts/** .mise/tasks/** .mise/lib/** .github/workflows/**'
)

usage() {
  echo "usage: $0 <base> | --files-from <file|->" >&2
  exit 2
}

[ $# -ge 1 ] || usage
case "$1" in
  --files-from)
    [ $# -eq 2 ] || usage
    if [ "$2" = "-" ]; then files=$(cat); else files=$(cat "$2"); fi
    ;;
  -*) usage ;;
  *)
    [ $# -eq 1 ] || usage
    files=$(git diff --name-only "$1...HEAD")
    ;;
esac

for rule in "${RULES[@]}"; do
  component=${rule%%|*}
  read -r -a globs <<<"${rule#*|}"
  matched=0
  while IFS= read -r file && [ "$matched" -eq 0 ]; do
    [ -n "$file" ] || continue
    for glob in "${globs[@]}"; do
      # shellcheck disable=SC2254 # the glob is meant to be a pattern
      case "$file" in
        $glob)
          matched=1
          break
          ;;
      esac
    done
  done <<<"$files"
  [ "$matched" -eq 1 ] && echo "$component"
done
exit 0
