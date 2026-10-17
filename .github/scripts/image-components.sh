#!/usr/bin/env bash
# Released image components and how to build them (mirrors the _docker.yml inputs of each ci-<dir>.yml;
# generated into scripts/monorepo/graph.generated.sh from scripts/monorepo/components.json).
#
#   .github/scripts/image-components.sh            prints `dir image context smoke timeout` lines
#   .github/scripts/image-components.sh <dir>      prints that component as one JSON object
#                                                  ({dir,image,context,smoke,timeout}); exit 1 if unknown
# smoke is "-" in the table when the component has no smoke script (empty in the JSON).
set -euo pipefail
# shellcheck source=scripts/monorepo/graph.generated.sh
source "$(dirname "${BASH_SOURCE[0]}")/../../scripts/monorepo/graph.generated.sh"
table=$IMAGE_TABLE

if [ $# -eq 0 ]; then echo "$table"; exit 0; fi
while read -r dir image context smoke timeout; do
  [ "$dir" = "$1" ] || continue
  [ "$smoke" = - ] && smoke=""
  jq -cn --arg d "$dir" --arg i "$image" --arg c "$context" --arg s "$smoke" --argjson t "$timeout" \
    '{dir:$d,image:$i,context:$c,smoke:$s,timeout:$t}'
  exit 0
done <<<"$table"
echo "unknown image component: $1" >&2
exit 1
