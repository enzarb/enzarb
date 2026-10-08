#!/usr/bin/env bash
# Refresh src/lib/server/testdata/*.sql from a sibling enzarb-workflows checkout and record
# their sha256 in testdata/workflows-sql.sha256 (checked by testdata.spec.ts).
# Usage: scripts/sync-workflows-sql.sh [path/to/enzarb-workflows]   (default ../enzarb-workflows)
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src="${1:-$root/../enzarb-workflows}/migrations"
dst="$root/src/lib/server/testdata"

# copy name -> source migration
declare -A files=(
	[workflows_schema.sql]=20261008000001_workflows_schema.sql
	[workflows_api.sql]=20261008000002_workflows_api.sql
)

[ -d "$src" ] || { echo "no migrations dir at $src" >&2; exit 1; }

: >"$dst/workflows-sql.sha256.tmp"
for name in workflows_schema.sql workflows_api.sql; do
	cp "$src/${files[$name]}" "$dst/$name"
	# hash of the source file, keyed by the copy's name
	printf '%s  %s\n' "$(sha256sum "$src/${files[$name]}" | cut -d' ' -f1)" "$name" \
		>>"$dst/workflows-sql.sha256.tmp"
done
mv "$dst/workflows-sql.sha256.tmp" "$dst/workflows-sql.sha256"
echo "synced from $src"
