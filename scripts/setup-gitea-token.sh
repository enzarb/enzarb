#!/usr/bin/env bash
# Mint (or refresh) the Gitea admin API token with the correct scopes.
# Idempotent: deletes all existing tokens for the admin user and creates a
# fresh one with the required scopes. Restarts consumers if the token changed.
#
# Usage: scripts/setup-gitea-token.sh [namespace]
set -euo pipefail

NS="${1:-enzarb-system}"
TOKEN_NAME="enzarb-deploy-token"
LOCAL_PORT="${GITEA_LOCAL_PORT:-13000}"

# Pull admin creds from the cluster secret.
U=$(kubectl -n "$NS" get secret enzarb-gitea-admin-creds -o jsonpath='{.data.username}' | base64 -d)
P=$(kubectl -n "$NS" get secret enzarb-gitea-admin-creds -o jsonpath='{.data.password}' | base64 -d)
GITEA="http://localhost:${LOCAL_PORT}"

echo "  Port-forwarding Gitea to localhost:${LOCAL_PORT}..."
kubectl -n "$NS" port-forward svc/gitea-http "${LOCAL_PORT}:3000" >/dev/null 2>&1 &
PF_PID=$!
trap "kill $PF_PID 2>/dev/null || true" EXIT

# Wait for Gitea to be reachable via port-forward (up to 60s)
for _ in $(seq 1 60); do
  curl -sf "${GITEA}/api/v1/version" >/dev/null 2>&1 && break
  sleep 1
done
curl -sf "${GITEA}/api/v1/version" >/dev/null 2>&1 || {
  echo "  Gitea did not become reachable via port-forward. Skipping token mint." >&2
  exit 0
}
echo "  Gitea is ready."

# If the current secret token works for read:issue, we're done.
if kubectl -n "$NS" get secret enzarb-gitea-admin >/dev/null 2>&1; then
  EXISTING=$(kubectl -n "$NS" get secret enzarb-gitea-admin -o jsonpath='{.data.token}' | base64 -d 2>/dev/null || true)
  if [ -n "$EXISTING" ]; then
    STATUS=$(curl -s -o /dev/null -w '%{http_code}' \
      -H "Authorization: token ${EXISTING}" "${GITEA}/api/v1/repos/search?limit=1")
    if [ "$STATUS" = "200" ]; then
      kill $PF_PID 2>/dev/null || true
      trap - EXIT
      echo "  Existing Gitea token is valid. No change needed."
      exit 0
    fi
  fi
fi

# Delete all existing tokens for the admin user (idempotent clean slate).
echo "  Clearing existing admin tokens..."
IDS=$(curl -sf -u "$U:$P" "${GITEA}/api/v1/users/${U}/tokens" \
  | python3 -c "import sys,json; [print(t['id']) for t in json.load(sys.stdin)]" 2>/dev/null || true)
for ID in $IDS; do
  curl -sf -u "$U:$P" -X DELETE "${GITEA}/api/v1/users/${U}/tokens/${ID}" >/dev/null || true
done

# Mint the new token.
echo "  Minting token '${TOKEN_NAME}'..."
RESPONSE=$(curl -sf -u "$U:$P" -X POST "${GITEA}/api/v1/users/${U}/tokens" \
  -H "Content-Type: application/json" \
  -d "{\"name\":\"${TOKEN_NAME}\",\"scopes\":[\"write:admin\",\"write:organization\",\"write:repository\",\"write:user\",\"read:issue\"]}")
TOK=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['sha1'])" 2>/dev/null || true)

kill $PF_PID 2>/dev/null || true
trap - EXIT

if [ -z "$TOK" ]; then
  echo "  Failed to mint Gitea token. API response: ${RESPONSE}" >&2
  exit 1
fi

# Store in the secret (create or update).
OLD_TOK=""
if kubectl -n "$NS" get secret enzarb-gitea-admin >/dev/null 2>&1; then
  OLD_TOK=$(kubectl -n "$NS" get secret enzarb-gitea-admin -o jsonpath='{.data.token}' | base64 -d 2>/dev/null || true)
fi

kubectl -n "$NS" create secret generic enzarb-gitea-admin \
  --from-literal=token="$TOK" --dry-run=client -o yaml | kubectl apply -f - >/dev/null

if [ "$TOK" != "$OLD_TOK" ]; then
  echo "  Gitea token updated. Restarting consumers..."
  for deploy in enzarb-operator enzarb-app enzarb-metering; do
    kubectl -n "$NS" rollout restart "deploy/${deploy}" 2>/dev/null || true
  done
else
  echo "  Gitea token unchanged."
fi
echo "  Gitea admin token ready."
