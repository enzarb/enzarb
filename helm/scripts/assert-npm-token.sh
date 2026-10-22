#!/usr/bin/env bash
# The workflow engine is removed: `workflows.*` values are accepted and ignored. Assert that they
# still render and that no enzarb-workflows Deployment is ever produced, whatever they say.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-npm-token: $1: got '$2', want '$3'" >&2; fail=1; fi
}
render() { # extra helm args
  helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" "$@"
}
npm_env() { yq eval-all 'select(.kind=="Deployment" and .metadata.name=="enzarb-workflows") | .metadata.name' -; }

check "default" "$(render | npm_env)" ""
check "override accepted, ignored" \
  "$(render --set workflows.npmToken.existingSecret=other --set workflows.npmToken.key=tok | npm_env)" ""
check "disabled accepted" "$(render --set workflows.npmToken.enabled=false | npm_env)" ""
exit $fail
