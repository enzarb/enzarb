#!/usr/bin/env bash
# Tool versions are pinned in the root mise.toml only (docs/ci.md, "Tool pins"). Fails when:
#   - a component mise.toml has a [tools] section;
#   - a root pin is `latest`;
#   - a Dockerfile ARG tool version differs from the root pin (table below), or an `ARG *_VERSION`
#     is neither mapped nor listed as unmapped;
#   - root `rust` differs from a rust-toolchain.toml channel, or kubernetes/hack/versions.env
#     HELM_VERSION differs from root `helm`.
# Shared by .github/workflows/ci-monorepo.yml and the lefthook pre-commit hook.
# The root can be given as $1 (used by test-tool-pins.sh); default is the git toplevel.
set -euo pipefail
root=${1:-$(git rev-parse --show-toplevel)}
cd "$root"

# Dockerfile|ARG|mise tool (rows are added by hand)
MAPPED=(
  'sandbox-container/Dockerfile|NODE_VERSION|node'
  'sandbox-container/Dockerfile|GH_VERSION|gh'
)
# Dockerfile|ARG: checked by a dedicated script (playwright is an npm package, not a mise tool)
DELEGATED=(
  'sandbox-container/Dockerfile|PLAYWRIGHT_VERSION'
)
# Dockerfile ARGs deliberately not covered: tools only the image installs, no root pin.
UNMAPPED=(
  'sandbox-container/Dockerfile|CLAUDE_AGENT_ACP_VERSION'
  'sandbox-container/Dockerfile|MISE_VERSION'
  'sandbox-container/Dockerfile|DOCKER_VERSION'
  'sandbox-container/Dockerfile|BUILDX_VERSION'
  'sandbox-container/Dockerfile|COMPOSE_VERSION'
  'sandbox-container/Dockerfile|SCCACHE_VERSION'
  'kubernetes/installer/Dockerfile|GVISOR_VERSION'
)

fail=0
err() {
  echo "::error::$*" >&2
  fail=1
}

# pin <file> <tool>: the version pinned under [tools] (quotes around the key optional), or empty.
pin() {
  sed -n "/^\[tools\]/,/^\[/{s/^\"\{0,1\}$2\"\{0,1\} *= *\"\(.*\)\"\$/\1/p}" "$1" | head -n1
}

in_list() { # in_list <needle> <list entries...>
  local n=$1 e
  shift
  for e in "$@"; do [ "$e" = "$n" ] && return 0; done
  return 1
}

# 1. Component mise.toml files hold tasks only.
for f in */mise.toml; do
  [ -e "$f" ] || continue
  grep -q '^\[tools\]' "$f" && err "$f: tool pins belong in the root mise.toml (or mise.ansible.toml)"
done

# 2. No floating pins.
for f in mise.toml mise.*.toml; do
  [ -e "$f" ] || continue
  if grep -nE '^[^#]*=[[:space:]]*"(latest|lts)"' "$f" >&2; then
    err "$f: pin a concrete version, not latest/lts"
  fi
done

# 3. Dockerfile ARG tool versions match the root pins.
while IFS= read -r dockerfile; do
  dockerfile=${dockerfile#./}
  while IFS= read -r arg; do
    [ -n "$arg" ] || continue
    row="$dockerfile|$arg"
    if in_list "$row" "${UNMAPPED[@]}" "${DELEGATED[@]}"; then
      continue
    fi
    tool=
    for m in "${MAPPED[@]}"; do
      [ "${m%|*}" = "$row" ] && tool=${m##*|}
    done
    if [ -z "$tool" ]; then
      err "$dockerfile: ARG $arg is not in the table of scripts/monorepo/check-tool-pins.sh (map it to a root pin or list it as unmapped)"
      continue
    fi
    want=$(pin mise.toml "$tool")
    got=$(sed -n "s/^ARG $arg=//p" "$dockerfile" | head -n1)
    got=${got#v}
    if [ -z "$want" ]; then
      err "mise.toml: no pin for $tool (mapped from $dockerfile ARG $arg)"
    elif [ "$want" != "$got" ]; then
      err "$dockerfile: ARG $arg=$got but mise.toml pins $tool = $want"
    fi
  done < <(sed -n 's/^ARG \([A-Z0-9_]*_VERSION\)\(=.*\)\{0,1\}$/\1/p' "$dockerfile" | sort -u)
done < <(find . -name Dockerfile -not -path '*/node_modules/*' -not -path '*/target/*' | sort)

# Rows must point at ARGs that still exist, so the table cannot go stale in the other direction.
for row in "${MAPPED[@]}" "${DELEGATED[@]}" "${UNMAPPED[@]}"; do
  df=${row%%|*}
  arg=$(echo "$row" | cut -d'|' -f2)
  grep -q "^ARG $arg\b" "$df" 2>/dev/null || err "check-tool-pins.sh lists $df ARG $arg, which no longer exists"
done

# Playwright is resolved from ui/package-lock.json.
if [ -e sandbox-container/scripts/check-playwright-version.sh ]; then
  bash sandbox-container/scripts/check-playwright-version.sh sandbox-container/Dockerfile ui/package-lock.json >/dev/null ||
    err "sandbox-container/Dockerfile: PLAYWRIGHT_VERSION differs from ui/package-lock.json (sandbox-container/scripts/check-playwright-version.sh)"
fi

# 4. Root rust equals every rust-toolchain.toml channel.
rust=$(pin mise.toml rust)
for f in */rust-toolchain.toml; do
  [ -e "$f" ] || continue
  channel=$(sed -n 's/^channel *= *"\(.*\)"$/\1/p' "$f" | head -n1)
  [ "$channel" = "$rust" ] || err "$f: channel $channel but mise.toml pins rust = ${rust:-<none>}"
done

# 5. The installer image's helm (hack/versions.env) is the root helm.
if [ -e kubernetes/hack/versions.env ]; then
  hv=$(sed -n 's/^HELM_VERSION=\([^ #]*\).*/\1/p' kubernetes/hack/versions.env)
  [ "${hv#v}" = "$(pin mise.toml helm)" ] || err "kubernetes/hack/versions.env: HELM_VERSION=$hv but mise.toml pins helm = $(pin mise.toml helm)"
fi

[ "$fail" -eq 0 ] && echo "ok: tool pins consistent"
exit "$fail"
