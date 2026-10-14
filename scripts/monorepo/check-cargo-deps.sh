#!/usr/bin/env bash
# No [patch] and no git deps on enzarb repos: enzarb crates are path deps (../<dir>) in the monorepo.
# Shared by .github/workflows/_rust.yml and the lefthook pre-commit hook.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if grep -rnE --include=Cargo.toml --include=Cargo.lock --exclude-dir=target --exclude-dir=node_modules '^\[+patch|git\+(https|ssh)://[^"]*enzarb|git *= *"[^"]*enzarb' .; then
  echo "::error::use path deps (../<dir>) for enzarb crates"
  exit 1
fi
