#!/usr/bin/env bash
# Per-component next-version diff between a PR and main, on the real tag history.
#
#   scripts/monorepo/next-version-diff.sh [--main-ref origin/main] [--sha HEAD]
#
# Runs .github/scripts/next-version.sh for every component twice over the same commit <sha>: with the
# checked-out (PR) script and graph, and with the ones from <main-ref>. Only the script and graph
# differ, so a difference is an effect of the change, not of other commits. Prints a markdown table on
# stdout (for $GITHUB_STEP_SUMMARY) and a ::warning:: per changed row on stderr. Exits non-zero only
# when a run itself fails, never on a difference. See docs/ci.md, "Next-version diff on release-path PRs".
set -euo pipefail

main_ref=origin/main
sha=HEAD
while [ $# -gt 0 ]; do
  case $1 in
    --main-ref) main_ref=$2; shift 2 ;;
    --sha) sha=$2; shift 2 ;;
    *) echo "usage: $0 [--main-ref <ref>] [--sha <sha>]" >&2; exit 2 ;;
  esac
done

cd "$(dirname "${BASH_SOURCE[0]}")/../.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# main's script and graph in the layout next-version.sh expects (it sources ../../scripts/monorepo/graph.generated.sh).
git archive "$main_ref" .github/scripts scripts/monorepo/graph.generated.sh | tar -x -C "$tmp"

graph_vars() { # <graph file>   DEPS_ variable names, one per line
  bash -c 'source "$1"; compgen -A variable DEPS_' _ "$1"
}

# Component dirs: top-level dirs of either side, matched to the variable names (_ stands for -).
mapfile -t dirs < <({ git ls-tree -d --name-only "$sha"; git ls-tree -d --name-only "$main_ref"; } | sort -u)
to_dir() { # <var suffix>
  local d
  for d in "${dirs[@]}"; do
    if [ "${d//-/_}" = "$1" ]; then echo "$d"; return; fi
  done
  echo "${1//_/-}"
}
mapfile -t pr_vars < <(graph_vars scripts/monorepo/graph.generated.sh)
mapfile -t main_vars < <(graph_vars "$tmp/scripts/monorepo/graph.generated.sh")
mapfile -t components < <(for v in "${pr_vars[@]}" "${main_vars[@]}"; do to_dir "${v#DEPS_}"; done | sort -u)
in_list() { local x; for x in "${@:2}"; do [ "$x" = "$1" ] && return 0; done; return 1; }

version_of() { # <script> <component> <graph var list...>   prints "<last> <next>" or "(absent)"
  local script=$1 comp=$2
  shift 2
  if ! in_list "DEPS_${comp//-/_}" "$@"; then echo "(absent) (absent)"; return; fi
  "$script" "$comp" "$sha"
}

short=$(git rev-parse --short "$sha")
echo "### Next-version diff: PR vs \`$main_ref\`"
echo
echo "Both columns are computed at \`$short\` with the full tag history; only \`next-version.sh\` and the graph differ."
echo
echo "| Component | main last tag | main next | PR last tag | PR next | Changed |"
echo "|---|---|---|---|---|---|"
changed=0
for c in "${components[@]}"; do
  read -r ml mn <<<"$(version_of "$tmp/.github/scripts/next-version.sh" "$c" "${main_vars[@]}")"
  read -r pl pn <<<"$(version_of .github/scripts/next-version.sh "$c" "${pr_vars[@]}")"
  mark=
  if [ "$ml $mn" != "$pl $pn" ]; then
    mark="**changed**"
    changed=$((changed + 1))
    echo "::warning title=next-version differs::$c: main '$ml $mn' -> PR '$pl $pn'" >&2
  fi
  echo "| $c | $ml | $mn | $pl | $pn | $mark |"
done
echo
if [ "$changed" -eq 0 ]; then echo "No differences."; else echo "$changed component(s) differ."; fi
