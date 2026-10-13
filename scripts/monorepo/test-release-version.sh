#!/usr/bin/env bash
# Local check of the monorepo release plumbing (no refs or files are changed).
#
#   scripts/monorepo/test-release-version.sh [<sha>]
#
# 1. Per released component: its last <dir>/v* tag, the next version for the real history since that
#    tag, and the next version after a synthetic `feat:` and a synthetic `fix:` commit touching the dir
#    (commits are built with a temporary index + commit-tree, never referenced).
#    Fails unless feat/fix continue from the latest imported tag.
# 2. Path filtering: which ci-<dir> workflows a change to sample paths triggers on push/pull_request.
set -euo pipefail
root=$(git rev-parse --show-toplevel)
cd "$root"
# shellcheck source=.github/scripts/next-version.sh
source .github/scripts/next-version.sh
base=$(git rev-parse "${1:-HEAD}")

synthetic() { # <dir> <subject>   prints a dangling commit on top of $base that touches <dir>
  local idx blob tree
  idx=$(mktemp); trap 'rm -f "$idx"' RETURN
  GIT_INDEX_FILE=$idx git read-tree "$base"
  blob=$(printf 'synthetic\n' | git hash-object -w --stdin)
  GIT_INDEX_FILE=$idx git update-index --add --cacheinfo "100644,$blob,$1/.synthetic-release-test"
  tree=$(GIT_INDEX_FILE=$idx git write-tree)
  git commit-tree "$tree" -p "$base" -m "$2"
}
bump() { # <version> minor|patch   (0.x rule: feat bumps minor)
  local M m p; IFS=. read -r M m p <<<"$1"
  if [ "$2" = minor ]; then echo "$M.$((m + 1)).0"; else echo "$M.$m.$((p + 1))"; fi
}

rc=0
printf '%-18s %-26s %-10s %-10s %-10s\n' component last-tag now +feat +fix
for dir in operator connectors workflows realtime sandbox-proxy sandbox-container ui updater helm; do
  last=$(last_tag "$dir" "$base"); now=$(next_version "$dir" "$base")
  f=$(next_version "$dir" "$(synthetic "$dir" "feat: synthetic")")
  x=$(next_version "$dir" "$(synthetic "$dir" "fix: synthetic")")
  printf '%-18s %-26s %-10s %-10s %-10s\n' "$dir" "${last:--}" "${now:--}" "$f" "$x"
  if [ -z "$last" ]; then echo "  FAIL: $dir has no imported tag"; rc=1; continue; fi
  lv=${last#"$dir"/v}
  # The real history since the tag may already hold a feat/fix; the synthetic one must not lower it.
  ef=$(bump "$lv" minor); ex=$(bump "$lv" patch)
  [ "$f" = "$ef" ] || { echo "  FAIL: $dir +feat gave $f, want $ef"; rc=1; }
  [ "$x" = "$ex" ] || [ "$x" = "$ef" ] || { echo "  FAIL: $dir +fix gave $x, want $ex (or $ef)"; rc=1; }
done
# A path-dep change releases the dependents: a feat in proto/ bumps ui, but not helm.
pf=$(synthetic proto "feat: synthetic proto change")
echo "after a synthetic proto feat: ui -> $(next_version ui "$pf"), workflows -> $(next_version workflows "$pf"), helm -> $(next_version helm "$pf" || true)"
[ "$(next_version ui "$pf")" = "$(bump "$(last_tag ui "$base" | sed 's#^ui/v##')" minor)" ] || { echo "  FAIL: proto feat did not bump ui"; rc=1; }
[ -z "$(next_version helm "$pf")" ] || { echo "  FAIL: proto feat bumped helm"; rc=1; }
helm_last=$(last_tag helm "$base")
[ "$(printf '%s\n%s\n' "helm/v0.69.0" "$helm_last" | sort -V | head -1)" = helm/v0.69.0 ] \
  || { echo "FAIL: helm continues below 0.69.0 ($helm_last)"; rc=1; }

echo
echo "path filtering (push to main / pull_request):"
patterns() { # <workflow file>  prints its push paths
  awk '/paths: &paths/{f=1;next} f&&/^ *- /{sub(/^ *- */,""); gsub(/\x27/,""); print; next} f{exit}' "$1"
}
for f in proto/src/lib.rs ui/src/x.ts helm/enzarb/values.yaml docs/x.md operator/src/main.rs \
         kubernetes/hack/validate.sh .github/workflows/_rust.yml .github/scripts/next-version.sh; do
  hits=()
  for wf in .github/workflows/ci-*.yml; do
    while IFS= read -r pat; do
      # GitHub's `**` matches across `/`; bash [[ == ]] `*` already does, so map ** -> *.
      # shellcheck disable=SC2053
      if [[ $f == ${pat//\*\*/*} ]]; then hits+=("$(basename "$wf" .yml)"); break; fi
    done < <(patterns "$wf")
  done
  printf '  %-36s -> %s\n' "$f" "${hits[*]:-(none)}"
done
exit $rc
