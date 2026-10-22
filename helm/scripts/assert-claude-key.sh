#!/usr/bin/env bash
# Assertions on the per-user Claude token keyring wiring (see README "Claude token keyring").
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-claude-key: $1: got '$2', want '$3'" >&2; fail=1; fi
}
render() { # extra helm args
  helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" "$@"
}
secret() { yq eval-all 'select(.kind=="Secret" and .metadata.name=="enzarb-claude-key") | '"$1" -; }
# Keyring volume of a Deployment: name, secretName, defaultMode, mount readOnly.
vol() { yq eval-all 'select(.kind=="Deployment" and .metadata.name=="'"$1"'") | .spec.template.spec.volumes[] | select(.name=="claude-key") | '"$2" -; }
mnt() { yq eval-all 'select(.kind=="Deployment" and .metadata.name=="'"$1"'") | .spec.template.spec.containers[0].volumeMounts[] | select(.name=="claude-key") | '"$2" -; }

on="$(render)"
check "one keyring secret" "$(secret .metadata.name <<<"$on" | grep -c enzarb-claude-key)" 1
check "keep policy" "$(secret '.metadata.annotations["helm.sh/resource-policy"]' <<<"$on")" keep
check "active and one key id" "$(secret '.data | keys | length' <<<"$on")" 2
check "active present" "$(secret '.data | has("active")' <<<"$on")" true
# The active id names a key, and the key is 32 bytes.
active="$(secret '.data.active' <<<"$on" | base64 -d)"
check "active names a key" "$(secret ".data.\"$active\" | length > 0" <<<"$on")" true
check "key is 32 bytes" "$(secret ".data.\"$active\"" <<<"$on" | base64 -d | wc -c | tr -d ' ')" 32
for d in enzarb-sandbox-proxy; do
  check "$d secret" "$(vol $d .secret.secretName <<<"$on")" enzarb-claude-key
  check "$d mode" "$(vol $d .secret.defaultMode <<<"$on")" 0440
  # root-owned key files must be group-readable by the non-root uid
  check "$d fsGroup" "$(yq eval-all 'select(.kind=="Deployment" and .metadata.name=="'"$d"'") | .spec.template.spec.securityContext.fsGroup' - <<<"$on")" 65532
  check "$d readOnly" "$(mnt $d .readOnly <<<"$on")" true
done
check "proxy db role" "$(yq eval-all 'select(.kind=="Deployment" and .metadata.name=="enzarb-sandbox-proxy") | .spec.template.spec.containers[0].env[] | select(.name=="PGUSER") | .valueFrom.secretKeyRef.name' - <<<"$on")" enzarb-pg-sandbox-proxy-credentials
check "pgop role" "$(yq eval-all 'select(.kind=="Role" and .metadata.name=="sandbox-proxy") | .spec.roleName' - <<<"$on")" enzarb_sandbox_proxy

# Never in the sandbox namespace.
check "sandbox ns refs" "$(yq eval-all 'select(.metadata.namespace=="enzarb-sandboxes")' - <<<"$on" | grep -c enzarb-claude-key || true)" 0
check "no other deployment mounts it" "$(yq eval-all 'select(.kind=="Deployment" and .metadata.name!="enzarb-sandbox-proxy") | .spec.template.spec.volumes[].secret.secretName' - <<<"$on" | grep -c enzarb-claude-key || true)" 0

# Bring-your-own keyring: nothing rendered, mounts point at it.
ex="$(render --set secrets.claudeKey.existingSecret=mine)"
check "existing: no secret" "$(yq eval-all 'select(.kind=="Secret" and (.metadata.name=="enzarb-claude-key" or .metadata.name=="mine")) | .metadata.name' - <<<"$ex")" ""
check "existing: proxy mount" "$(vol enzarb-sandbox-proxy .secret.secretName <<<"$ex")" mine

# Rekey Job: absent by default, gated, no RBAC on the keyring.
job() { yq eval-all 'select(.kind=="Job" and (.metadata.name | test("^enzarb-claude-rekey-")))' - ; }
check "rekey absent by default" "$(job <<<"$on")" ""
if render --set secrets.claudeKey.rekey.enabled=true >/dev/null 2>&1; then
  echo "assert-claude-key: rekey without suffix rendered" >&2; fail=1
fi
rk="$(render --set secrets.claudeKey.rekey.enabled=true --set secrets.claudeKey.rekey.suffix=t1)"
rkj() { yq eval-all 'select(.kind=="Job" and .metadata.name=="enzarb-claude-rekey-t1") | '"$1" - <<<"$rk"; }
proxy_image="$(yq eval-all 'select(.kind=="Deployment" and .metadata.name=="enzarb-sandbox-proxy") | .spec.template.spec.containers[0].image' - <<<"$rk")"
check "rekey image" "$(rkj .spec.template.spec.containers[0].image)" "$proxy_image"
check "rekey args" "$(rkj '.spec.template.spec.containers[0].args | join(",")')" rekey
check "rekey mount readOnly" "$(rkj '.spec.template.spec.containers[0].volumeMounts[] | select(.name=="claude-key") | .readOnly')" true
check "rekey no api token" "$(rkj .spec.template.spec.automountServiceAccountToken)" false
check "rekey not a hook" "$(rkj '.metadata.annotations["helm.sh/hook"]')" null
check "no rbac on keyring" "$(yq eval-all 'select(.kind=="Role" or .kind=="ClusterRole") | .rules[].resourceNames[]' - <<<"$rk" | grep -c enzarb-claude-key || true)" 0
exit $fail
