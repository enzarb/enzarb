#!/usr/bin/env bash
# Classify the delta between an earlier reviewed head and the current head as mechanical or not.
#
#   scripts/monorepo/mechanical.sh <previous_reviewed_sha> [<head>=HEAD] [<max_lines>=300]
#
# Prints `mechanical` or `not_mechanical` on the first line; for `mechanical`, the affected
# components of the delta follow (scripts/monorepo/affected.sh), one per line. Always exits 0
# for a verdict; exit 2 is a usage error.
#
# A delta is mechanical only when every changed file is either
#   - generated or regenerated output (lockfiles, goldens, snapshots, generated types, kopium
#     output), or
#   - unchanged by the formatter once both versions are formatted (`rustfmt` for *.rs, `prettier`
#     for the UI's files), that is, a formatting-only change.
# The check is conservative: anything uncertain (the previous sha is not an ancestor, the delta is
# over <max_lines> changed lines, a file was added or deleted, a formatter is missing or fails)
# is `not_mechanical` and gets a normal agent review. The caller still runs the component checks;
# this script only classifies (docs/contracts.md, "change request review").
set -euo pipefail

[ $# -ge 1 ] && [ $# -le 3 ] || {
  echo "usage: $0 <previous_reviewed_sha> [<head>] [<max_lines>]" >&2
  exit 2
}
prev=$1
head=${2:-HEAD}
max_lines=${3:-300}

no() {
  echo "not_mechanical"
  [ $# -eq 0 ] || echo "# $*"
  exit 0
}

git rev-parse --verify --quiet "$prev^{commit}" >/dev/null || no "unknown commit $prev"
git rev-parse --verify --quiet "$head^{commit}" >/dev/null || no "unknown commit $head"
git merge-base --is-ancestor "$prev" "$head" || no "$prev is not an ancestor of $head"

lines=$(git diff --numstat --no-renames "$prev..$head" | awk '$1 == "-" || $2 == "-" { b = 1 } { s += $1 + $2 } END { print (b ? "binary" : s + 0) }')
[ "$lines" != "binary" ] || lines=0 # binary files are only fine when generated; checked per file
[ "$lines" -le "$max_lines" ] || no "$lines changed lines, over $max_lines"

files=$(git diff --name-only --no-renames "$prev..$head")
[ -n "$files" ] || no "no changed files"

is_generated() {
  case "$1" in
    package-lock.json | */package-lock.json | Cargo.lock | */Cargo.lock | pnpm-lock.yaml | */pnpm-lock.yaml | yarn.lock | */yarn.lock) return 0 ;;
    *.golden | */__snapshots__/* | *.snap) return 0 ;;
    *.generated.* | */generated/* | */upstream/*.rs | *.d.ts) return 0 ;;
  esac
  return 1
}

# True when `git show <rev>:<file>` formats to the same text in both revisions.
format_only() {
  local file=$1 a b
  case "$file" in
    *.rs)
      command -v rustfmt >/dev/null || return 1
      a=$(git show "$prev:$file" | rustfmt --edition 2024 --emit stdout 2>/dev/null) || return 1
      b=$(git show "$head:$file" | rustfmt --edition 2024 --emit stdout 2>/dev/null) || return 1
      ;;
    ui/*.ts | ui/*.js | ui/*.svelte | ui/*.css | ui/*.scss | ui/*.json | ui/*.md)
      [ -d ui/node_modules ] || return 1
      local rel=${file#ui/}
      a=$(git show "$prev:$file" | (cd ui && npx --no-install prettier --stdin-filepath "$rel") 2>/dev/null) || return 1
      b=$(git show "$head:$file" | (cd ui && npx --no-install prettier --stdin-filepath "$rel") 2>/dev/null) || return 1
      ;;
    *) return 1 ;;
  esac
  [ "$a" = "$b" ]
}

while IFS= read -r f; do
  [ -n "$f" ] || continue
  if is_generated "$f"; then
    continue
  fi
  # A formatter comparison needs the file on both sides.
  git cat-file -e "$prev:$f" 2>/dev/null || no "$f is new"
  git cat-file -e "$head:$f" 2>/dev/null || no "$f was deleted"
  format_only "$f" || no "$f changed beyond formatting"
done <<<"$files"

echo "mechanical"
printf '%s\n' "$files" | bash "$(dirname "$0")/affected.sh" --files-from -
