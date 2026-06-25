#!/usr/bin/env bash
# Create or rotate the secrets authd needs for registry token auth:
#   - registry-token-signing : RSA keypair. authd signs registry tokens with the
#     key; Zot trusts the cert (mounted as its bearer-auth anchor).
#   - enzarb-registry-admin  : shared secret the app presents to authd ("admin"
#     basic-auth user) to mint scoped tokens for the registry UI.
#
# Both secrets are always regenerated. After updating them, authd and Zot are
# restarted together so they always share the same keypair.
#
# Usage: scripts/setup-registry-secrets.sh [namespace]
set -euo pipefail

NS="${1:-enzarb-system}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "Generating RSA token-signing keypair…"
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TMP/tls.key" -out "$TMP/tls.crt" \
  -subj "/CN=registry-token" -days 3650 >/dev/null 2>&1

# Verify key/cert pair before writing to the cluster.
KEY_MOD=$(openssl rsa -noout -modulus -in "$TMP/tls.key" | md5sum)
CRT_MOD=$(openssl x509 -noout -modulus -in "$TMP/tls.crt" | md5sum)
if [ "$KEY_MOD" != "$CRT_MOD" ]; then
  echo "ERROR: generated key and cert moduli do not match — aborting." >&2
  exit 1
fi

echo "Applying secret registry-token-signing in namespace $NS…"
kubectl -n "$NS" delete secret registry-token-signing --ignore-not-found=true >/dev/null
kubectl -n "$NS" create secret tls registry-token-signing \
  --cert="$TMP/tls.crt" --key="$TMP/tls.key"

echo "Applying secret enzarb-registry-admin in namespace $NS…"
kubectl -n "$NS" delete secret enzarb-registry-admin --ignore-not-found=true >/dev/null
kubectl -n "$NS" create secret generic enzarb-registry-admin \
  --from-literal=token="$(openssl rand -hex 32)"

echo "Restarting authd and Zot so they load the new keypair together…"
kubectl -n "$NS" rollout restart deployment/enzarb-authd
kubectl -n "$NS" rollout restart deployment/zot

echo "Waiting for rollouts to complete…"
kubectl -n "$NS" rollout status deployment/enzarb-authd --timeout=120s
kubectl -n "$NS" rollout status deployment/zot --timeout=120s

echo "Done. authd and Zot are running with the new registry-token-signing keypair."
echo "Also restart any pods that cache the old admin token (e.g. metering):"
echo "  kubectl -n $NS rollout restart deployment/enzarb-metering"
