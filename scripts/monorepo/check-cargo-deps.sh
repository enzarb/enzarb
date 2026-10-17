#!/usr/bin/env bash
# Cargo hygiene for the monorepo (docs/ci.md, "Rust repos"). Shared by
# .github/workflows/_rust.yml and the lefthook pre-commit hook.
#   - no [patch] and no git deps on enzarb repos: enzarb crates are path deps in the monorepo;
#   - exactly one Cargo.lock and one rust-toolchain.toml, both at the repo root;
#   - no member Cargo.toml declares its own [workspace];
#   - every Cargo.toml is a member of the root workspace (cargo metadata lists it).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
fail=0

# Tracked and untracked-but-not-ignored files (skips target/ and node_modules/ without walking them).
files() { git ls-files --cached --others --exclude-standard -- "$@"; }

if files '*Cargo.toml' '*Cargo.lock' | xargs -r grep -nE '^\[+patch|git\+(https|ssh)://[^"]*enzarb|git *= *"[^"]*enzarb'; then
  echo "::error::use path deps (../<dir>) for enzarb crates"
  fail=1
fi

for name in Cargo.lock rust-toolchain.toml; do
  extra=$(files "$name" "*/$name" | grep -vx "$name" || true)
  if [ -n "$extra" ] || [ ! -f "$name" ]; then
    echo "::error::exactly one $name, at the repo root (extra: ${extra:-none})"
    fail=1
  fi
done

nested=$(files '*/Cargo.toml' | xargs -r grep -l '^\[workspace' || true)
if [ -n "$nested" ]; then
  echo "::error::only the root Cargo.toml declares a [workspace]; found in: $nested"
  fail=1
fi

members=$(cargo metadata --no-deps --format-version 1 | grep -o '"manifest_path":"[^"]*"' | sed 's/^"manifest_path":"//; s/"$//')
for manifest in $(files '*/Cargo.toml'); do
  if ! grep -qxF "$PWD/$manifest" <<<"$members"; then
    echo "::error::$manifest is not a member of the root workspace (add it to Cargo.toml)"
    fail=1
  fi
done

exit "$fail"
