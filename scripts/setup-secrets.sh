#!/usr/bin/env bash
# Create all secrets that enzarb auto-generates (idempotent — skip if present).
# Secrets that require external input (S3, Google OAuth, DB, OIDC) are NOT
# touched here; they must be pre-created by the operator before running deploy.
#
# Usage: scripts/setup-secrets.sh [namespace]
set -euo pipefail

NS="${1:-enzarb-system}"
GITEA_SVC="${GITEA_SVC:-http://gitea-http.${NS}.svc.cluster.local:3000}"
GITEA_ADMIN_USERNAME="${GITEA_ADMIN_USERNAME:-enzarb-admin}"

# Ensure the target namespace exists (may not exist on first run before system deploy).
kubectl create namespace "$NS" --dry-run=client -o yaml | kubectl apply -f - >/dev/null

secret_exists() { kubectl -n "$NS" get secret "$1" >/dev/null 2>&1; }

# ── Gitea admin creds (username + random password) ───────────────────────────
# Must exist BEFORE gitea is deployed so the chart can bootstrap the admin user.
if secret_exists enzarb-gitea-admin-creds; then
  echo "  [skip] enzarb-gitea-admin-creds already exists"
else
  echo "  [create] enzarb-gitea-admin-creds"
  PW="$(openssl rand -hex 24)"
  kubectl -n "$NS" create secret generic enzarb-gitea-admin-creds \
    --from-literal=username="$GITEA_ADMIN_USERNAME" \
    --from-literal=password="$PW"
fi

# ── Registry token-signing keypair (authd signs, Zot verifies) ───────────────
if secret_exists registry-token-signing; then
  echo "  [skip] registry-token-signing already exists"
else
  echo "  [create] registry-token-signing"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$TMP/tls.key" -out "$TMP/tls.crt" \
    -subj "/CN=registry-token" -days 3650 >/dev/null 2>&1
  kubectl -n "$NS" create secret tls registry-token-signing \
    --cert="$TMP/tls.crt" --key="$TMP/tls.key"
fi

# ── Registry admin shared secret (app → authd admin auth) ────────────────────
if secret_exists enzarb-registry-admin; then
  echo "  [skip] enzarb-registry-admin already exists"
else
  echo "  [create] enzarb-registry-admin"
  kubectl -n "$NS" create secret generic enzarb-registry-admin \
    --from-literal=token="$(openssl rand -hex 32)"
fi

# ── App JWT signing keypair (issued by the app for agent auth) ────────────────
if secret_exists enzarb-jwt; then
  echo "  [skip] enzarb-jwt already exists"
else
  echo "  [create] enzarb-jwt"
  TMP2="$(mktemp -d)"
  trap 'rm -rf "$TMP2"' EXIT
  openssl genrsa -out "$TMP2/private.pem" 2048 >/dev/null 2>&1
  openssl rsa -in "$TMP2/private.pem" -pubout -out "$TMP2/public.pem" >/dev/null 2>&1
  kubectl -n "$NS" create secret generic enzarb-jwt \
    --from-file=privateKey="$TMP2/private.pem" \
    --from-file=publicKey="$TMP2/public.pem"
fi

echo "  Secrets setup complete."
