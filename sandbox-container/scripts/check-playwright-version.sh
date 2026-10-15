#!/usr/bin/env bash
# Fails if the Dockerfile's PLAYWRIGHT_VERSION differs from the playwright version resolved in
# ui/package-lock.json. Usage: scripts/check-playwright-version.sh [Dockerfile] [package-lock.json]
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
dockerfile=${1:-$here/../Dockerfile}
lock=${2:-$here/../../ui/package-lock.json}

want=$(grep -E '^ARG PLAYWRIGHT_VERSION=' "$dockerfile" | head -n1 | cut -d= -f2)
got=$(grep -A1 -E '^\s*"node_modules/playwright": \{' "$lock" | grep -oE '"version": "[^"]+"' | head -n1 | cut -d'"' -f4)

[ -n "$want" ] || { echo "no ARG PLAYWRIGHT_VERSION in $dockerfile"; exit 1; }
[ -n "$got" ] || { echo "no node_modules/playwright in $lock"; exit 1; }
if [ "$want" != "$got" ]; then
  echo "playwright version drift: Dockerfile has $want, ui/package-lock.json has $got"
  echo "update ARG PLAYWRIGHT_VERSION in sandbox-container/Dockerfile"
  exit 1
fi
echo "playwright $got ok"
