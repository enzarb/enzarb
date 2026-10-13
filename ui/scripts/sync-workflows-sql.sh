#!/usr/bin/env bash
# Refresh src/lib/server/testdata/workflows/*.sql from the workflows/ migrations and record
# their sha256 in testdata/workflows-sql.sha256 (checked by testdata.spec.ts). Every migration is
# copied verbatim; the test setup applies them in filename order, so the fixture is their
# cumulative result.
# Usage: scripts/sync-workflows-sql.sh [path/to/workflows [git-ref]]
#   (default ../workflows). With a git-ref (or WORKFLOWS_REF), the migrations are read
#   from that ref (e.g. origin/main) instead of the working tree.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
repo="${1:-$root/../workflows}"
ref="${2:-${WORKFLOWS_REF:-}}"
dst="$root/src/lib/server/testdata"

if [ -n "$ref" ]; then
	mapfile -t names < <(git -C "$repo" ls-tree --name-only "$ref" migrations/ | sed 's|^migrations/||' | grep '\.sql$' | sort)
	read_migration() { git -C "$repo" show "$ref:./migrations/$1"; }
else
	[ -d "$repo/migrations" ] || { echo "no migrations dir at $repo/migrations" >&2; exit 1; }
	mapfile -t names < <(cd "$repo/migrations" && ls -1 -- *.sql | sort)
	read_migration() { cat "$repo/migrations/$1"; }
fi
[ "${#names[@]}" -gt 0 ] || { echo "no migrations found" >&2; exit 1; }

rm -rf "$dst/workflows"
mkdir -p "$dst/workflows"
: >"$dst/workflows-sql.sha256.tmp"
for name in "${names[@]}"; do
	read_migration "$name" >"$dst/workflows/$name"
	printf '%s  %s\n' "$(sha256sum "$dst/workflows/$name" | cut -d' ' -f1)" "$name" \
		>>"$dst/workflows-sql.sha256.tmp"
done
mv "$dst/workflows-sql.sha256.tmp" "$dst/workflows-sql.sha256"
echo "synced ${#names[@]} migrations from ${ref:-$repo}"
