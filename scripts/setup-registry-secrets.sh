#!/usr/bin/env bash
# Create the secrets authd needs for registry/git token auth:
#   - registry-token-signing : RSA keypair. authd signs registry tokens with the
#     key; Zot trusts the cert (mounted as its bearer-auth anchor).
#   - enzarb-registry-admin  : shared secret the app presents to authd ("admin"
#     basic-auth user) to mint scoped tokens for the registry UI.
#
# Idempotent-ish: re-running regenerates the keypair, which invalidates tokens
# already issued (they're short-lived, so this is safe) and requires restarting
# authd + Zot to pick up the new cert.
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

echo "Applying secret registry-token-signing in namespace $NS…"
kubectl -n "$NS" create secret tls registry-token-signing \
  --cert="$TMP/tls.crt" --key="$TMP/tls.key" \
  --dry-run=client -o yaml | kubectl apply -f -

echo "Applying secret enzarb-registry-admin in namespace $NS…"
ADMIN_TOKEN="$(openssl rand -hex 32)"
kubectl -n "$NS" create secret generic enzarb-registry-admin \
  --from-literal=token="$ADMIN_TOKEN" \
  --dry-run=client -o yaml | kubectl apply -f -

echo "Done. Restart authd and Zot to load the signing cert:"
echo "  kubectl -n $NS rollout restart deploy/enzarb-authd"
echo "  kubectl -n $NS rollout restart deploy/zot   # or the zot workload name"
