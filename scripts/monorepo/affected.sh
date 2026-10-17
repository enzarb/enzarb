#!/usr/bin/env bash
# Print the components affected by a diff, one per line, in dependency order.
#
#   scripts/monorepo/affected.sh <base>            # git diff --name-only <base>...HEAD
#   scripts/monorepo/affected.sh --files-from -    # newline-separated paths on stdin
#   scripts/monorepo/affected.sh --files-from FILE
#
# The rules are generated (scripts/monorepo/graph.generated.sh, from cargo metadata, package.json
# `file:` deps and scripts/monorepo/components.json; `mise run graph:gen`). They mirror the `paths:`
# filters of .github/workflows/ci-<component>.yml (including downstream components via path deps,
# e.g. `proto/**` triggers ui); scripts/monorepo/test-affected.sh fails when the two drift apart.
# Files matching no rule (docs/**, root files such as mise.toml or lefthook.yml) trigger no
# component CI, so they print nothing; the root Cargo.toml, Cargo.lock and rust-toolchain.toml are
# the exception (one workspace): they trigger every Rust component. `monorepo` is the
# pseudo-component for the repo plumbing itself (scripts, mise tasks, any workflow file): shellcheck,
# actionlint and the drift test, see .github/workflows/ci-monorepo.yml.
set -euo pipefail

# RULES: component|space-separated globs (bash `case` patterns; `*` also matches `/`)
# shellcheck source=scripts/monorepo/graph.generated.sh
source "$(dirname "${BASH_SOURCE[0]}")/graph.generated.sh"

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
