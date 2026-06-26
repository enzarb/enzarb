#!/usr/bin/env bash
# Create all secrets that enzarb auto-generates (idempotent — skip if present).
# Secrets that require external input (S3, Google OAuth, DB, OIDC) are NOT
# touched here; they must be pre-created by the operator before running deploy.
#
# Usage: scripts/setup-secrets.sh [namespace]
set -euo pipefail

NS="${1:-enzarb-system}"

# Ensure the target namespace exists (may not exist on first run before system deploy).
kubectl create namespace "$NS" --dry-run=client -o yaml | kubectl apply -f - >/dev/null

secret_exists() { kubectl -n "$NS" get secret "$1" >/dev/null 2>&1; }

# ── Registry token-signing keypair (authd signs, Zot verifies) ───────────────
if secret_exists registry-token-signing; then
  # Verify the existing secret has a valid key/cert pair before skipping.
  KEY_MOD=$(kubectl -n "$NS" get secret registry-token-signing \
    -o jsonpath='{.data.tls\.key}' | base64 -d | openssl rsa -noout -modulus 2>/dev/null | md5sum)
  CRT_MOD=$(kubectl -n "$NS" get secret registry-token-signing \
    -o jsonpath='{.data.tls\.crt}' | base64 -d | openssl x509 -noout -modulus 2>/dev/null | md5sum)
  if [ "$KEY_MOD" = "$CRT_MOD" ] && [ -n "$KEY_MOD" ]; then
    echo "  [skip] registry-token-signing already exists and key/cert pair is valid"
  else
    echo "  [WARN] registry-token-signing exists but key/cert pair is INVALID — re-running"
    echo "         setup-registry-secrets.sh to rotate and restart affected services."
    "$(dirname "$0")/setup-registry-secrets.sh" "$NS"
  fi
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

# ── GitHub OAuth App (optional — skip if GITHUB_OAUTH_CLIENT_ID is unset) ────
if [ -n "${GITHUB_OAUTH_CLIENT_ID:-}" ]; then
  if secret_exists enzarb-github-oauth; then
    echo "  [skip] enzarb-github-oauth already exists"
  else
    if [ -z "${GITHUB_OAUTH_CLIENT_SECRET:-}" ]; then
      echo "  [warn] GITHUB_OAUTH_CLIENT_ID is set but GITHUB_OAUTH_CLIENT_SECRET is empty — skipping"
    else
      echo "  [create] enzarb-github-oauth"
      kubectl -n "$NS" create secret generic enzarb-github-oauth \
        --from-literal=clientSecret="$GITHUB_OAUTH_CLIENT_SECRET"
    fi
  fi
fi

echo "  Secrets setup complete."
