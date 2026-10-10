#!/usr/bin/env bash
# shellcheck disable=SC2034  # variables are used by the sourcing tasks
# Shared helpers for the oldie:* mise tasks (single-node RKE2 install). Sourced, not executed.
# Reuses log/die/need/k/kapply from dev.sh, pointed at the oldie cluster instead of kind.

# shellcheck source=/dev/null
. "${MISE_PROJECT_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}/.mise/lib/dev.sh"

KCTX="${ENZARB_OLDIE_CONTEXT:-enzarb-oldie}"          # context written by the Ansible role
KUBECONFIG="${ENZARB_OLDIE_KUBECONFIG:-$HOME/.kube/enzarb-oldie.yaml}"
export KUBECONFIG
OLDIE_HOST="${ENZARB_OLDIE_HOST:-192.168.33.11}"  # LAN address; public access is a Cloudflare Tunnel, no inbound ports
PUBLIC_HOST="enzarb.dev"
KDIR="$ROOT/enzarb-kubernetes"
RKE2_DIR="$KDIR/rke2"

# Optional for most tasks; individual tasks require the variables they need.
load_oldie_env() {
  local f="$ROOT/.env.oldie"
  [ -f "$f" ] || return 0
  set -a
  # shellcheck disable=SC1090
  . "$f"
  set +a
}
require_oldie_env() {
  [ -f "$ROOT/.env.oldie" ] || die "$ROOT/.env.oldie not found; create it with: mise run oldie:init-env (then fill it in, docs/oldie.md)"
  [ "$(stat -c %a "$ROOT/.env.oldie")" = 600 ] || echo "warning: $ROOT/.env.oldie holds secrets but is not mode 600 (chmod 600 it)" >&2
  load_oldie_env
  local v; for v in "$@"; do [ -n "${!v:-}" ] || die "$v is empty in .env.oldie"; done
}
cluster_reachable() { k get --raw /readyz >/dev/null 2>&1; }
