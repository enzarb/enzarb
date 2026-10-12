#!/usr/bin/env bash
# Deploy enzarb on oldie through an RKE2 helm-controller HelmChart (helm.cattle.io/v1) named
# `enzarb` in kube-system, applied with kubectl. Not written to the rke2 server `manifests`
# directory: the controller re-applies files there on restart, which would revert in-cluster
# `spec.version` patches made by enzarb-updater.
#
# Required env:
#   VALUES_FILE        non-secret chart values (becomes spec.valuesContent)
#   RELEASE_NAMESPACE  namespace of the existing Helm release (spec.targetNamespace)
# Optional env:
#   RELEASE_NAME       Helm release name of the existing release (default enzarb). RKE2's
#                      helm-controller has no spec.releaseName: the release name is the HelmChart
#                      metadata.name (enzarb), so any other value is refused.
#   CHART              default oci://ghcr.io/enzarb/charts/enzarb
#   FORCE_VERSION      apply this chart version on an existing HelmChart only if it is strictly
#                      greater than the live spec.version; on first create it overrides the
#                      deployed version
#   REGISTRY_SECRET    name of a kubernetes.io/dockerconfigjson Secret in kube-system with a ghcr.io
#                      read:packages credential (spec.dockerRegistrySecret): the chart is private and
#                      the helm-controller pulls it itself. Never a credential value (oldie:secrets
#                      creates the Secret from .env.oldie).
#   DRY_RUN=1          print the manifest instead of applying it
#   WAIT_TIMEOUT       wait for the helm-install Job (default 10m)
#   KUBECTL, HELM      binaries (default kubectl, helm); KUBECONFIG is honoured as usual
#
# spec.version ownership: on first create it is the currently deployed chart version (never
# "latest"). On later runs the live value is written back unchanged, so a client-side apply
# neither lowers it nor drops it from the last-applied annotation.
set -euo pipefail

KUBECTL=${KUBECTL:-kubectl}
HELM=${HELM:-helm}
CHART=${CHART:-oci://ghcr.io/enzarb/charts/enzarb}
RELEASE_NAME=${RELEASE_NAME:-enzarb}
WAIT_TIMEOUT=${WAIT_TIMEOUT:-10m}
[ "$RELEASE_NAME" = enzarb ] || { echo "RELEASE_NAME must be enzarb (the HelmChart name is the release name)" >&2; exit 1; }
: "${VALUES_FILE:?VALUES_FILE is required}"
: "${RELEASE_NAMESPACE:?RELEASE_NAMESPACE is required}"
[ -r "$VALUES_FILE" ] || { echo "cannot read $VALUES_FILE" >&2; exit 1; }

# Secrets live in Kubernetes Secrets; the chart must reference them by name. Fail on any
# secret-looking key with a literal value (references such as existingSecret/secretName/*Ref are fine).
secret_key='(password|passwd|token|secret|key)'
ref_key='(existingSecret|secretName|secretRef|secretKeyRef|[A-Za-z]*Ref|[A-Za-z]*SecretName|registryTokenSecret)'
bad=$(grep -inE "^[[:space:]-]*[A-Za-z0-9_.-]*${secret_key}[A-Za-z0-9_.-]*:[[:space:]]*[^[:space:]#{|>]" "$VALUES_FILE" |
  grep -viE "^[0-9]+:[[:space:]-]*${ref_key}:" | grep -viE ':[[:space:]]*(true|false|null|""|'"''"')[[:space:]]*(#.*)?$' || true)
if [ -n "$bad" ]; then
  echo "refusing: $VALUES_FILE has literal values for secret-looking keys (use Kubernetes Secrets):" >&2
  printf '%s\n' "$bad" | cut -d: -f1 | while read -r n; do echo "  line $n" >&2; done
  exit 1
fi

# true when $1 is strictly greater than $2 (sort -V; pre-release ordering is approximate).
version_gt() {
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n1)" = "$1" ]
}

version=""
# Only a NotFound error means "create"; any other failure (API unreachable, RBAC) aborts so we
# never fall back to the deployed version and lower a version enzarb-updater set.
get_err=$(mktemp)
trap 'rm -f "$get_err"' EXIT
if live=$("$KUBECTL" -n kube-system get helmchart.helm.cattle.io enzarb -o 'jsonpath={.spec.version}' 2>"$get_err"); then
  found=1
elif grep -q 'NotFound' "$get_err"; then
  found=0
else
  echo "cannot determine whether HelmChart enzarb exists:" >&2
  cat "$get_err" >&2
  exit 1
fi
if [ "$found" = 1 ]; then
  exists=1
  version=$live
  if [ -n "${FORCE_VERSION:-}" ]; then
    if [ -z "$live" ] || version_gt "$FORCE_VERSION" "$live"; then
      version=$FORCE_VERSION
    else
      echo "FORCE_VERSION $FORCE_VERSION is not greater than live $live; keeping $live" >&2
    fi
  fi
else
  exists=0
  version=${FORCE_VERSION:-}
  if [ -z "$version" ]; then
    meta=$("$HELM" get metadata "$RELEASE_NAME" -n "$RELEASE_NAMESPACE" -o json) ||
      { echo "no Helm release $RELEASE_NAME in $RELEASE_NAMESPACE to adopt; set FORCE_VERSION for a fresh install" >&2; exit 1; }
    re='"version":"([^"]+)"'
    [[ $meta =~ $re ]] || { echo "cannot read the deployed chart version from: $meta" >&2; exit 1; }
    version=${BASH_REMATCH[1]}
  fi
fi

render() {
  echo "apiVersion: helm.cattle.io/v1"
  echo "kind: HelmChart"
  echo "metadata:"
  echo "  name: enzarb"
  echo "  namespace: kube-system"
  echo "spec:"
  echo "  chart: $CHART"
  [ -z "$version" ] || echo "  version: \"$version\""
  echo "  targetNamespace: $RELEASE_NAMESPACE"
  echo "  createNamespace: true"
  echo "  timeout: 20m"
  # Never uninstall on a failed upgrade: the controller default (reinstall) runs `helm uninstall`,
  # which deletes the pgop Cluster/Database/Role CRs and the generated Secrets.
  echo "  failurePolicy: abort"
  # The Project CRD is also server-side applied by oldie:bootstrap-helmchart (field manager kubectl);
  # helm must be able to take .spec.versions over on a (re)install instead of failing on the conflict.
  echo "  forceConflicts: true"
  if [ -n "${REGISTRY_SECRET:-}" ]; then
    echo "  dockerRegistrySecret:"
    echo "    name: $REGISTRY_SECRET"
  fi
  echo "  valuesContent: |-"
  while IFS= read -r line || [ -n "$line" ]; do
    if [ -n "$line" ]; then printf '    %s\n' "$line"; else echo; fi
  done < "$VALUES_FILE"
}

if [ "${DRY_RUN:-}" = 1 ]; then
  render
  exit 0
fi

# A completed Job from a previous run may still exist; remember its UID so we wait for the
# Job the controller creates for this apply, not the stale one.
old_uid=$("$KUBECTL" -n kube-system get job helm-install-enzarb --ignore-not-found -o 'jsonpath={.metadata.uid}' 2>/dev/null || true)

applied=$(render | "$KUBECTL" apply -f -)
printf '%s\n' "$applied"
echo "HelmChart enzarb applied (existing=$exists, version=${version:-unset}); waiting for the helm-install Job"
# An unchanged HelmChart does not make the controller recreate the Job, so there is nothing new to wait for.
if [ -n "$old_uid" ] && [[ $applied == *unchanged* ]]; then
  echo "HelmChart unchanged; waiting on the existing Job" >&2
else
  # The controller creates/recreates the Job after the apply; wait for one with a new UID.
  for _ in $(seq 1 60); do
    new_uid=$("$KUBECTL" -n kube-system get job helm-install-enzarb --ignore-not-found -o 'jsonpath={.metadata.uid}' 2>/dev/null || true)
    [ -n "$new_uid" ] && [ "$new_uid" != "$old_uid" ] && break
    sleep 2
  done
  if [ -z "${new_uid:-}" ] || [ "$new_uid" = "$old_uid" ]; then
    echo "no new helm-install-enzarb Job appeared after apply; the controller did not start a rollout" >&2
    exit 1
  fi
fi
"$KUBECTL" -n kube-system wait --for=condition=complete job/helm-install-enzarb --timeout="$WAIT_TIMEOUT"
"$HELM" list -n "$RELEASE_NAMESPACE" --filter "^${RELEASE_NAME}\$"
