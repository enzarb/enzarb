#!/usr/bin/env bash
# Fails if the Dockerfile's PLAYWRIGHT_VERSION differs from the playwright version resolved in
# ui/pnpm-lock.yaml. Usage: scripts/check-playwright-version.sh [Dockerfile] [pnpm-lock.yaml]
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
dockerfile=${1:-$here/../Dockerfile}
lock=${2:-$here/../../ui/pnpm-lock.yaml}

want=$(grep -E '^ARG PLAYWRIGHT_VERSION=' "$dockerfile" | head -n1 | cut -d= -f2)
# The importer entry of ui's direct `playwright` dependency: `      playwright:` / specifier / `version: X`.
got=$(grep -A2 -E '^      playwright:$' "$lock" | sed -n 's/^ *version: \([0-9][^ (]*\).*/\1/p' | head -n1)

[ -n "$want" ] || { echo "no ARG PLAYWRIGHT_VERSION in $dockerfile"; exit 1; }
[ -n "$got" ] || { echo "no playwright dependency in $lock"; exit 1; }
if [ "$want" != "$got" ]; then
  echo "playwright version drift: Dockerfile has $want, ui/pnpm-lock.yaml has $got"
  echo "update ARG PLAYWRIGHT_VERSION in sandbox-container/Dockerfile"
  exit 1
fi
echo "playwright $got ok"
