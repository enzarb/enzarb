#!/usr/bin/env bash
# Offline tests for hack/oldie-helmchart.sh using fake kubectl/helm binaries (DRY_RUN render).
set -euo pipefail
cd "$(dirname "$0")/.."
script=$PWD/hack/oldie-helmchart.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Fake kubectl: `get helmchart` succeeds with $LIVE_VERSION when LIVE_VERSION is set (even empty), else fails.
cat > "$tmp/kubectl" <<'EOF'
#!/usr/bin/env bash
if [ -n "${LIVE_VERSION+x}" ]; then printf '%s' "$LIVE_VERSION"; exit 0; fi
if [ -n "${GET_ERROR:-}" ]; then echo "$GET_ERROR" >&2; exit 1; fi
echo 'Error from server (NotFound): helmcharts.helm.cattle.io "enzarb" not found' >&2
exit 1
EOF
cat > "$tmp/helm" <<'EOF'
#!/usr/bin/env bash
echo '{"name":"enzarb","version":"1.4.2","app_version":"1.4.2"}'
EOF
chmod +x "$tmp/kubectl" "$tmp/helm"

printf 'replicas: 1\nexistingSecret: enzarb-db\nsecretKeyRef: {}\nimage:\n  tag: v1\n' > "$tmp/ok.yaml"
printf 'db:\n  password: hunter2\n' > "$tmp/bad.yaml"
export KUBECTL=$tmp/kubectl HELM=$tmp/helm DRY_RUN=1 RELEASE_NAMESPACE=enzarb

run() { VALUES_FILE=$1 bash "$script"; }
fail() { echo "FAIL: $*" >&2; exit 1; }

# First create: version comes from the deployed release, never latest.
out=$(run "$tmp/ok.yaml")
grep -q 'version: "1.4.2"' <<< "$out" || fail "first create should pin the deployed version"
grep -q 'chart: oci://ghcr.io/enzarb/charts/enzarb' <<< "$out" || fail "chart"
grep -q 'targetNamespace: enzarb' <<< "$out" || fail "targetNamespace"
grep -q '    existingSecret: enzarb-db' <<< "$out" || fail "valuesContent"

# Re-run after the updater bumped the live version: the live value is kept.
out=$(LIVE_VERSION=2.0.0 run "$tmp/ok.yaml")
grep -q 'version: "2.0.0"' <<< "$out" || fail "live version must be preserved"

# FORCE_VERSION only wins when strictly greater.
out=$(LIVE_VERSION=2.0.0 FORCE_VERSION=1.9.0 run "$tmp/ok.yaml")
grep -q 'version: "2.0.0"' <<< "$out" || fail "must not downgrade"
out=$(LIVE_VERSION=2.0.0 FORCE_VERSION=2.1.0 run "$tmp/ok.yaml")
grep -q 'version: "2.1.0"' <<< "$out" || fail "should upgrade"

# A non-NotFound get failure must abort, not take the first-create path.
if GET_ERROR='Unable to connect to the server' run "$tmp/ok.yaml" > /dev/null 2>&1; then fail "API error must not mean missing"; fi
if GET_ERROR='Error from server (Forbidden): forbidden' run "$tmp/ok.yaml" > /dev/null 2>&1; then fail "RBAC error must not mean missing"; fi

# Secret guard.
if run "$tmp/bad.yaml" > /dev/null 2>&1; then fail "literal secret must be rejected"; fi

echo "oldie-helmchart tests ok"
