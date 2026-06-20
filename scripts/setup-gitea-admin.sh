#!/usr/bin/env bash
# Provision the Gitea admin used by the operator/app to create orgs, repos, and
# per-project users (the identities reverse-proxy auth maps X-Gitea-User to).
#
# Two secrets:
#   - enzarb-gitea-admin-creds : username + password. The gitea chart
#     (deploy/system/gitea.yaml, gitea.admin.existingSecret) bootstraps an admin
#     user from this. Create BEFORE deploying gitea.
#   - enzarb-gitea-admin       : an admin API token minted from that user,
#     consumed by the operator (GITEA_ADMIN_TOKEN) and app.
#
# Usage:
#   scripts/setup-gitea-admin.sh creds   # step 1: create creds secret, then deploy gitea
#   scripts/setup-gitea-admin.sh token   # step 2: mint API token (after gitea is up)
set -euo pipefail

NS="${NAMESPACE:-enzarb-system}"
USERNAME="${GITEA_ADMIN_USERNAME:-enzarb-admin}"
GITEA_SVC="${GITEA_SVC:-http://gitea-http.enzarb-system.svc.cluster.local:3000}"

case "${1:-}" in
  creds)
    PW="$(openssl rand -hex 24)"
    kubectl -n "$NS" create secret generic enzarb-gitea-admin-creds \
      --from-literal=username="$USERNAME" --from-literal=password="$PW" \
      --dry-run=client -o yaml | kubectl apply -f -
    echo "Created enzarb-gitea-admin-creds (username=$USERNAME)."
    echo "Now deploy gitea (mise run deploy-system) so the admin user is created,"
    echo "then run: $0 token"
    ;;
  token)
    # Mint an admin API token via a short-lived in-cluster pod (so the gitea
    # service and the creds secret are both reachable), then store it.
    PO="gitea-token-$RANDOM"
    kubectl -n "$NS" run "$PO" --restart=Never --image=curlimages/curl:8.10.1 \
      --env="G=$GITEA_SVC" \
      --overrides="$(cat <<JSON
{"spec":{"containers":[{"name":"c","image":"curlimages/curl:8.10.1","command":["sh","-c"],
"args":["curl -s -u \"\$U:\$P\" -X POST \"\$G/api/v1/users/\$U/tokens\" -H 'Content-Type: application/json' -d '{\"name\":\"enzarb-operator-'\$RANDOM'\",\"scopes\":[\"write:admin\",\"write:organization\",\"write:repository\",\"write:user\"]}' | grep -o '\"sha1\":\"[^\"]*\"' | cut -d'\"' -f4"],
"env":[{"name":"G","value":"$GITEA_SVC"},
{"name":"U","valueFrom":{"secretKeyRef":{"name":"enzarb-gitea-admin-creds","key":"username"}}},
{"name":"P","valueFrom":{"secretKeyRef":{"name":"enzarb-gitea-admin-creds","key":"password"}}}]}]}}
JSON
)" >/dev/null
    sleep 8
    TOK="$(kubectl -n "$NS" logs "$PO" | tr -d '[:space:]')"
    kubectl -n "$NS" delete pod "$PO" --ignore-not-found >/dev/null
    if [ -z "$TOK" ]; then echo "Failed to mint token (is gitea up with the admin user?)" >&2; exit 1; fi
    kubectl -n "$NS" create secret generic enzarb-gitea-admin \
      --from-literal=token="$TOK" --dry-run=client -o yaml | kubectl apply -f -
    echo "Stored enzarb-gitea-admin token. Restart the operator to pick it up:"
    echo "  kubectl -n $NS rollout restart deploy/enzarb-operator"
    ;;
  *)
    echo "usage: $0 <creds|token>" >&2; exit 1 ;;
esac
