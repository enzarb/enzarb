#!/usr/bin/env bash
# Wait until every other component releasable from the same commit has published its tag (or is known not to).
# Used by the `settle` job of release.yml so that one commit yields one chart version (docs/ci.md "Releases").
#
# Environment:
#   SHA       commit being released (required)
#   SELF      component dir of this run, never waited on (default: none)
#   TIMEOUT   overall deadline in seconds (default 2400); on expiry a warning is logged and the script exits 0
#   GRACE     seconds to wait for a CI run / release run to appear before treating it as absent (default 300)
#   INTERVAL  poll interval in seconds (default 30)
#   GH_TOKEN  token for `gh` (needs actions: read)
#   GITHUB_RUN_ID (optional) this release run, ignored when looking at sibling release runs
#
# A sibling is *pending* when next_version says it has something to release and its tag does not exist yet.
# Pending siblings are settled when: the tag exists; its ci-<dir> run for SHA ended unsuccessfully;
# no ci-<dir> run for SHA exists after GRACE; or its CI succeeded and, after GRACE, no other release run for
# SHA is still active (so the image build failed or never started).
set -euo pipefail

: "${SHA:?SHA required}"
SELF=${SELF:-}
TIMEOUT=${TIMEOUT:-2400}
GRACE=${GRACE:-300}
INTERVAL=${INTERVAL:-30}
SUMMARY=${GITHUB_STEP_SUMMARY:-/dev/null}
root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# shellcheck source=/dev/null
source "$root/next-version.sh"

say() { echo "$*" | tee -a "$SUMMARY"; }

pending=()
while read -r dir _; do
  [ -n "$dir" ] && [ "$dir" != "$SELF" ] || continue
  v=$(next_version "$dir" "$SHA")
  [ -n "$v" ] || continue
  git rev-parse -q --verify "refs/tags/$dir/v$v" >/dev/null && continue
  pending+=("$dir:$v")
done < <("$root/image-components.sh")

if [ ${#pending[@]} -eq 0 ]; then say "settle: no sibling component pending for ${SHA:0:7}"; exit 0; fi
say "settle: waiting for ${pending[*]} (commit ${SHA:0:7})"

start=$SECONDS

ci_state() { # <dir> -> none | active | ok | failed
  local runs
  runs=$(gh run list --workflow "ci-$1.yml" --commit "$SHA" --event push --branch main --json status,conclusion --limit 20)
  jq -r '
    if length == 0 then "none"
    elif any(.[]; .status != "completed") then "active"
    elif any(.[]; .conclusion == "success") then "ok"
    else "failed" end' <<<"$runs"
}

release_active() { # any other release run for SHA still running?
  local runs
  runs=$(gh run list --workflow release.yml --commit "$SHA" --json databaseId,status --limit 50)
  jq -e --arg self "${GITHUB_RUN_ID:-}" 'any(.[]; .status != "completed" and (.databaseId | tostring) != $self)' <<<"$runs" >/dev/null
}

while :; do
  git fetch -q --tags origin || true
  left=()
  for p in "${pending[@]}"; do
    dir=${p%%:*} v=${p#*:}
    if git rev-parse -q --verify "refs/tags/$dir/v$v" >/dev/null; then say "settle: $dir/v$v published"; continue; fi
    elapsed=$((SECONDS - start))
    case $(ci_state "$dir") in
      failed) say "settle: ci-$dir failed for ${SHA:0:7}, not waiting for it"; continue ;;
      none) if [ "$elapsed" -ge "$GRACE" ]; then say "settle: ci-$dir not triggered for ${SHA:0:7}, not waiting for it"; continue; fi ;;
      ok) if [ "$elapsed" -ge "$GRACE" ] && ! release_active; then
            say "settle: no active release run for $dir/v$v, not waiting for it"; continue
          fi ;;
    esac
    left+=("$p")
  done
  pending=("${left[@]}")
  [ ${#pending[@]} -gt 0 ] || { say "settle: all siblings settled"; exit 0; }
  if [ $((SECONDS - start)) -ge "$TIMEOUT" ]; then
    echo "::warning::settle timed out after ${TIMEOUT}s still waiting for ${pending[*]}; publishing a partial chart"
    say "settle: timed out waiting for ${pending[*]}"
    exit 0
  fi
  sleep "$INTERVAL"
done
