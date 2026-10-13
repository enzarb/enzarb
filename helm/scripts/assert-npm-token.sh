#!/usr/bin/env bash
# Assertions on the workflows ENZARB_NPM_TOKEN wiring (see README "Private npm packages").
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-npm-token: $1: got '$2', want '$3'" >&2; fail=1; fi
}
render() { # extra helm args
  helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" "$@"
}
npm_env() { yq eval-all 'select(.kind=="Deployment" and .metadata.name=="enzarb-workflows") | .spec.template.spec.containers[0].env[] | select(.name=="ENZARB_NPM_TOKEN") | '"$1" -; }

on="$(render)"
check "default secret" "$(npm_env .valueFrom.secretKeyRef.name <<<"$on")" enzarb-updater-registry
check "default key" "$(npm_env .valueFrom.secretKeyRef.key <<<"$on")" registry-token
check "optional" "$(npm_env .valueFrom.secretKeyRef.optional <<<"$on")" true
check "never a literal value" "$(npm_env .value <<<"$on")" null

ov="$(render --set workflows.npmToken.existingSecret=other --set workflows.npmToken.key=tok)"
check "override secret" "$(npm_env .valueFrom.secretKeyRef.name <<<"$ov")" other
check "override key" "$(npm_env .valueFrom.secretKeyRef.key <<<"$ov")" tok

off="$(render --set workflows.npmToken.enabled=false)"
check "disabled" "$(npm_env .name <<<"$off")" ""

# Only the executor role needs it.
noexec="$(render --set "workflows.roles=reducer\,ticker")"
check "no executor role" "$(npm_env .name <<<"$noexec")" ""
exit $fail
