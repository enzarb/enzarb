#!/usr/bin/env bash
# shellcheck disable=SC2034  # variables are used by the sourcing tasks
# Shared helpers for the dev:* mise tasks. Sourced, not executed.

ROOT="${MISE_PROJECT_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
CLUSTER_NAME="${ENZARB_DEV_CLUSTER:-enzarb}"
KCTX="kind-${CLUSTER_NAME}"
NS="enzarb"
RELEASE="enzarb"
# Where the Calico manifest comes from when ENZARB_DEV_CNI=calico (see docs/dev.md).
CALICO_VERSION="${ENZARB_DEV_CALICO_VERSION:-v3.30.3}"

log() { printf '\033[1m==> %s\033[0m\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
need() { for c in "$@"; do command -v "$c" >/dev/null 2>&1 || die "missing required tool: $c"; done; }

# kubectl/helm pinned to the dev cluster so a stray current-context is never touched.
k() { kubectl --context "$KCTX" "$@"; }
kapply() { k apply --server-side --force-conflicts "$@"; }

cluster_exists() { kind get clusters 2>/dev/null | grep -qx "$CLUSTER_NAME"; }

load_env() {
  local f="$ROOT/.env.dev"
  [ -f "$f" ] || die "$f not found; copy .env.dev.example to .env.dev and fill it in (docs/dev.md)"
  set -a
  # shellcheck disable=SC1090
  . "$f"
  set +a
}

# component -> "dir image-ref"
image_spec() {
  case "$1" in
    ui) echo "ui ghcr.io/enzarb/enzarb-ui" ;;
    sandbox-proxy) echo "sandbox-proxy ghcr.io/enzarb/enzarb-sandbox-proxy" ;;
    sandbox) echo "sandbox-container ghcr.io/enzarb/enzarb-sandbox" ;;
    operator) echo "operator ghcr.io/enzarb/enzarb-sandbox-operator" ;;
    receiver) echo "connectors ghcr.io/enzarb/enzarb-connectors-receiver" ;;
    workflows) echo "workflows ghcr.io/enzarb/enzarb-workflows" ;;
    *) return 1 ;;
  esac
}
ALL_COMPONENTS="ui sandbox-proxy sandbox operator receiver workflows"
