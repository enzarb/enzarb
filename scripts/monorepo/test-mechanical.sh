#!/usr/bin/env bash
# Tests for scripts/monorepo/mechanical.sh on fixture repositories: formatting-only,
# lockfile-only, mixed, oversized, new file and non-ancestor deltas.
set -euo pipefail

script=$(cd "$(dirname "$0")" && pwd)/mechanical.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail=0

repo() { # repo <name>: a fresh repository with an initial commit, cwd set to it
  mkdir -p "$tmp/$1"
  cd "$tmp/$1" || exit 1
  git init -q -b main
  git config user.email test@example.test
  git config user.name test
  git config commit.gpgsign false
  mkdir -p updater/crates/engine/src
  printf 'fn main() {\n    println!("hi");\n}\n' >updater/crates/engine/src/main.rs
  printf '{}\n' >ui-package-lock.json
  printf '# lock\n' >updater/Cargo.lock
  git add -A
  git commit -q -m init
}

commit() { git add -A && git commit -q -m "$1"; }

expect() { # expect <description> <first line> [args to the script]
  local what=$1 want=$2 got
  shift 2
  got=$(bash "$script" "$@")
  got=${got%%$'\n'*}
  if [ "$got" != "$want" ]; then
    echo "FAIL: $what: expected [$want], got [$got]" >&2
    fail=1
  fi
}

repo lock
base=$(git rev-parse HEAD)
printf '# lock changed\n' >updater/Cargo.lock
commit lock
expect "lockfile-only delta" mechanical "$base" HEAD
got=$(bash "$script" "$base" HEAD | tail -n +2 | paste -sd' ' -)
[ "$got" = "updater" ] || {
  echo "FAIL: lockfile-only affected components: [$got]" >&2
  fail=1
}

repo mixed
base=$(git rev-parse HEAD)
printf '# lock changed\n' >updater/Cargo.lock
printf 'fn main() {\n    println!("changed");\n}\n' >updater/crates/engine/src/main.rs
commit mixed
expect "lockfile plus a code change" not_mechanical "$base" HEAD

repo manifest
base=$(git rev-parse HEAD)
printf '[package]\n' >updater/Cargo.toml
printf '# lock changed\n' >updater/Cargo.lock
commit manifest
expect "a lockfile change together with a manifest change" not_mechanical "$base" HEAD

repo dts
mkdir -p ui/src
printf 'export {};\n' >ui/src/types.d.ts
commit dts
base=$(git rev-parse HEAD)
printf 'export type A = 1;\n' >ui/src/types.d.ts
commit dts2
expect "a hand-written .d.ts edit" not_mechanical "$base" HEAD

repo newfile
base=$(git rev-parse HEAD)
printf 'fn x() {}\n' >updater/crates/engine/src/new.rs
commit new
expect "a new source file" not_mechanical "$base" HEAD

repo big
base=$(git rev-parse HEAD)
seq 1 400 >updater/Cargo.lock
commit big
expect "a generated delta over the line limit" not_mechanical "$base" HEAD 300
expect "the same delta under a larger limit" mechanical "$base" HEAD 1000

repo ancestor
git checkout -q -b other
printf '# other\n' >updater/Cargo.lock
commit other
other=$(git rev-parse HEAD)
git checkout -q main
printf '# main\n' >updater/Cargo.lock
commit main
expect "a previous sha that is not an ancestor" not_mechanical "$other" HEAD
expect "an unknown previous sha" not_mechanical 0000000000000000000000000000000000000000 HEAD

repo same
expect "an empty delta" not_mechanical HEAD HEAD

if command -v rustfmt >/dev/null && printf 'fn main() {}\n' | rustfmt --edition 2024 --emit stdout >/dev/null 2>&1; then
  repo fmt
  base=$(git rev-parse HEAD)
  printf 'fn main() { println!("hi"); }\n' >updater/crates/engine/src/main.rs
  commit unformatted
  mid=$(git rev-parse HEAD)
  printf 'fn main() {\n    println!("hi");\n}\n' >updater/crates/engine/src/main.rs
  commit formatted
  expect "a formatting-only delta" mechanical "$mid" HEAD
  expect "a delta that nets out to nothing" not_mechanical "$base" HEAD
else
  echo "rustfmt not found: skipping the formatting-only cases" >&2
fi

[ "$fail" -eq 0 ] && echo "mechanical.sh: ok"
exit "$fail"
