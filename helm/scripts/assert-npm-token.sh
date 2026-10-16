#!/usr/bin/env bash
# `workflows.npmToken` is deprecated (ADR-0022: accepted and ignored). Assert that the keys are
# still accepted and that ENZARB_NPM_TOKEN is never rendered, whatever they say.
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

check "default" "$(render | npm_env .name)" ""
check "override accepted, ignored" \
  "$(render --set workflows.npmToken.existingSecret=other --set workflows.npmToken.key=tok | npm_env .name)" ""
check "disabled accepted" "$(render --set workflows.npmToken.enabled=false | npm_env .name)" ""
exit $fail
