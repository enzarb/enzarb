#!/usr/bin/env bash
# Assertions on the Web Push (VAPID) keypair wiring (see README "Web Push keypair").
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-vapid-key: $1: got '$2', want '$3'" >&2; fail=1; fi
}
render() { # extra helm args
  helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" "$@"
}
ui_env() { yq eval-all 'select(.kind=="Deployment" and .metadata.name=="enzarb-ui") | .spec.template.spec.containers[0].env[] | select(.name=="'"$1"'") | '"$2" -; }
job() { yq eval-all 'select(.kind=="Job" and .metadata.name=="enzarb-vapid-key") | '"$1" -; }

on="$(render)"
check "job is a pre hook" "$(job '.metadata.annotations["helm.sh/hook"]' <<<"$on")" pre-install,pre-upgrade
check "job image is the ui image" "$(job '.spec.template.spec.containers[0].image' <<<"$on")" "$(yq eval-all 'select(.kind=="Deployment" and .metadata.name=="enzarb-ui") | .spec.template.spec.containers[0].image' - <<<"$on")"
check "job secret name" "$(job '.spec.template.spec.containers[0].env[] | select(.name=="ENZARB_VAPID_SECRET") | .value' <<<"$on")" enzarb-vapid-key
check "job namespace" "$(job '.spec.template.spec.containers[0].env[] | select(.name=="ENZARB_VAPID_NAMESPACE") | .value' <<<"$on")" enzarb
# The chart never renders the private key (nothing to leak into release history).
check "no rendered secret" "$(yq eval-all 'select(.kind=="Secret" and .metadata.name=="enzarb-vapid-key") | .metadata.name' - <<<"$on")" ""
# The Job may create Secrets and read only its own.
check "get is limited to the keypair" "$(yq eval-all 'select(.kind=="Role" and .metadata.name=="enzarb-vapid") | .rules[] | select(.verbs[]=="get") | .resourceNames[0]' - <<<"$on")" enzarb-vapid-key
check "no update or delete" "$(yq eval-all 'select(.kind=="Role" and .metadata.name=="enzarb-vapid") | .rules[].verbs[]' - <<<"$on" | grep -cE 'update|patch|delete|list|watch' || true)" 0
# Only the ui Deployment reads the keypair, from the Secret, never as a literal.
check "ui public key ref" "$(ui_env ENZARB_VAPID_PUBLIC_KEY '.valueFrom.secretKeyRef | .name + "/" + .key' <<<"$on")" enzarb-vapid-key/public-key
check "ui private key ref" "$(ui_env ENZARB_VAPID_PRIVATE_KEY '.valueFrom.secretKeyRef | .name + "/" + .key' <<<"$on")" enzarb-vapid-key/private-key
check "ui keys are optional" "$(ui_env ENZARB_VAPID_PRIVATE_KEY '.valueFrom.secretKeyRef.optional' <<<"$on")" true
check "ui private key has no literal" "$(ui_env ENZARB_VAPID_PRIVATE_KEY '.value' <<<"$on")" null
check "no other deployment reads it" "$(yq eval-all 'select(.kind=="Deployment" and .metadata.name!="enzarb-ui") | .spec.template.spec.containers[].env[].valueFrom.secretKeyRef.name' - <<<"$on" | grep -c enzarb-vapid-key || true)" 0
check "subject unset by default" "$(ui_env ENZARB_VAPID_SUBJECT .value <<<"$on")" ""
check "sandbox ns refs" "$(yq eval-all 'select(.metadata.namespace=="enzarb-sandboxes")' - <<<"$on" | grep -c enzarb-vapid || true)" 0

# Subject override.
sub="$(render --set secrets.vapidKey.subject=mailto:ops@example.com)"
check "subject override" "$(ui_env ENZARB_VAPID_SUBJECT .value <<<"$sub")" mailto:ops@example.com
if render --set secrets.vapidKey.subject=ops >/dev/null 2>&1; then
  echo "assert-vapid-key: invalid subject rendered" >&2; fail=1
fi

# Bring-your-own keypair: no Job, no RBAC, env points at it.
ex="$(render --set secrets.vapidKey.existingSecret=mine)"
check "existing: no job" "$(job .metadata.name <<<"$ex")" ""
check "existing: no role" "$(yq eval-all 'select(.kind=="Role" and .metadata.name=="enzarb-vapid") | .metadata.name' - <<<"$ex")" ""
check "existing: ui public key ref" "$(ui_env ENZARB_VAPID_PUBLIC_KEY '.valueFrom.secretKeyRef.name' <<<"$ex")" mine
check "existing: ui private key ref" "$(ui_env ENZARB_VAPID_PRIVATE_KEY '.valueFrom.secretKeyRef.name' <<<"$ex")" mine
exit $fail
