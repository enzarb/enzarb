#!/usr/bin/env bash
# Decide whether a component's CI jobs run: prints `run=true` or `run=false`.
#
#   scripts/monorepo/changes.sh <component> <base-sha> <head-sha>
#
# Used by .github/workflows/_changes.yml (docs/ci.md, "Changes job"). The component is selected when
# `git diff <base>...<head>` touches a path affected.sh maps to it (own dir, path-dependency dirs,
# root Cargo files, its workflow files). When there is no usable base (empty or all zeros: new branch,
# manual dispatch; or a sha missing from the clone: force push) everything runs.
set -euo pipefail

[ $# -eq 3 ] || { echo "usage: $0 <component> <base-sha> <head-sha>" >&2; exit 2; }
component=$1 base=$2 head=$3
here=$(dirname "${BASH_SOURCE[0]}")

all() { echo "run=true"; echo "changes: $1, running everything" >&2; exit 0; }

case $base in "" | 0000000000000000000000000000000000000000) all "no base commit" ;; esac
git cat-file -e "$base^{commit}" 2>/dev/null || all "base ${base:0:7} is not in the clone"
git merge-base "$base" "$head" >/dev/null 2>&1 || all "no merge base for ${base:0:7}...${head:0:7}"

selected=$(git diff --name-only "$base...$head" | bash "$here/affected.sh" --files-from -)
if grep -qx "$component" <<<"$selected"; then
  echo "run=true"
else
  echo "run=false"
fi
