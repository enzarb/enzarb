#!/usr/bin/env bash
# Tests for scripts/monorepo/next-version-diff.sh on a scratch git repo (components a and b, "main" is
# the initial state, the PR is a commit on top).
#
#   1. identical scripts and graph: no changed rows, no warnings;
#   2. a PR edit of next-version.sh changes one component's version: only that row is flagged;
#   3. a component added to the graph is (absent) on the main side and flagged;
#   4. a new path dependency in the graph changes the dependent component;
#   5. a failing next-version.sh is an error (exit 1, ::error), never "No differences";
#   6. header and row count match the union of the components.
set -euo pipefail

src=$(cd "$(dirname "$0")/../.." && pwd)
fail=0
tmps=()
trap 'rm -rf "${tmps[@]}"' EXIT

ok() { # ok <description> <expected> <got>
  if [ "$2" != "$3" ]; then echo "FAIL: $1: expected [$2], got [$3]" >&2; fail=1; fi
}

new_repo() { # prints the path of a repo at the "main" state
  local d
  d=$(mktemp -d)
  tmps+=("$d")
  mkdir -p "$d/.github/scripts" "$d/scripts/monorepo" "$d/a" "$d/b" "$d/c"
  cp "$src/.github/scripts/next-version.sh" "$d/.github/scripts/"
  cp "$src/scripts/monorepo/next-version-diff.sh" "$d/scripts/monorepo/"
  cat >"$d/scripts/monorepo/graph.generated.sh" <<'G'
#!/usr/bin/env bash
# shellcheck disable=SC2034
DEPS_a=( )
DEPS_b=( )
G
  (
    cd "$d"
    git init -q -b main . && git config user.email t@t && git config user.name t
    touch a/f b/f c/f
    git add -A && git commit -qm "feat: base"
    git tag a/v0.1.0 && git tag b/v0.2.0
    echo 1 >a/f && git add -A && git commit -qm "fix(a): a"
    echo 1 >b/f && git add -A && git commit -qm "feat(b): b"
    echo 1 >c/f && git add -A && git commit -qm "chore(c): c"
    git branch base
  )
  echo "$d"
}

run() { # <repo>   sets out (stdout) and err (stderr)
  local errf
  errf=$(mktemp)
  rc=0
  out=$(cd "$1" && bash scripts/monorepo/next-version-diff.sh --main-ref base --sha HEAD 2>"$errf") || rc=$?
  err=$(cat "$errf")
  rm -f "$errf"
}
row() { grep "^| $1 |" <<<"$out"; }

# 1. identical
r=$(new_repo)
run "$r"
ok "identical: no warnings" "" "$err"
ok "identical: no changed rows" "0" "$(grep -c 'changed\*\*' <<<"$out" || true)"
ok "identical: a" "| a | a/v0.1.0 | 0.1.1 | a/v0.1.0 | 0.1.1 |  |" "$(row a)"
ok "identical: b" "| b | b/v0.2.0 | 0.3.0 | b/v0.2.0 | 0.3.0 |  |" "$(row b)"

# 2. script change: fix bumps minor
r=$(new_repo)
(
  cd "$r"
  sed -i 's/patch=1$/minor=1/' .github/scripts/next-version.sh
  git add -A && git commit -qm "ci: fix bumps minor"
)
if git -C "$r" diff --quiet base HEAD; then echo "FAIL: sed did not edit next-version.sh" >&2; fail=1; fi
run "$r"
ok "script change: a changed" "| a | a/v0.1.0 | 0.1.1 | a/v0.1.0 | 0.2.0 | **changed** |" "$(row a)"
ok "script change: b unchanged" "| b | b/v0.2.0 | 0.3.0 | b/v0.2.0 | 0.3.0 |  |" "$(row b)"
ok "script change: one warning" "1" "$(grep -c '^::warning' <<<"$err")"

# 3. component added to the graph
r=$(new_repo)
(cd "$r" && echo 'DEPS_c=( )' >>scripts/monorepo/graph.generated.sh && git add -A && git commit -qm "ci: add c")
run "$r"
ok "added: c absent on main" "| c | (absent) | (absent) | - | 0.1.0 | **changed** |" "$(row c)"
ok "added: 3 rows" "3" "$(grep -c '^| [a-c] |' <<<"$out")"

# 4. new path dependency: a depends on c, which has a feat commit
r=$(new_repo)
(
  cd "$r"
  echo 2 >c/f && git add -A && git commit -qm "feat(c): c"
  git branch -f base
  sed -i "s/^DEPS_a=.*/DEPS_a=( 'c' )/" scripts/monorepo/graph.generated.sh
  git add -A && git commit -qm "ci: a depends on c"
)
run "$r"
ok "deps: a changed" "| a | a/v0.1.0 | 0.1.1 | a/v0.1.0 | 0.2.0 | **changed** |" "$(row a)"
ok "deps: b unchanged" "| b | b/v0.2.0 | 0.3.0 | b/v0.2.0 | 0.3.0 |  |" "$(row b)"

# 5. next-version.sh fails on both sides: an error, never "No differences"
r=$(new_repo)
(
  cd "$r"
  sed -i '2i exit 3' .github/scripts/next-version.sh
  git add -A && git commit -qm "ci: break next-version"
  git branch -f base
)
run "$r"
ok "failing: exit status" "1" "$rc"
ok "failing: not reported as no differences" "0" "$(grep -c 'No differences' <<<"$out" || true)"
ok "failing: errors raised" "1" "$( [ "$(grep -c '^::error' <<<"$err")" -gt 0 ] && echo 1 || echo 0)"

# 6. table shape
ok "header" "| Component | main last tag | main next | PR last tag | PR next | Changed |" "$(grep '^| Component' <<<"$out")"

if [ "$fail" = 0 ]; then echo "next-version-diff: all tests passed"; fi
exit "$fail"
