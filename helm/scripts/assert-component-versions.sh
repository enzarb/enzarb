#!/usr/bin/env bash
# Assertions on ENZARB_COMPONENT_VERSIONS (the UI About page input) in the rendered enzarb-ui Deployment.
# Shape: {"chart": "<chart version>", "components": {"<repo name>": "<tag>"}}.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-component-versions: $1: got '$2', want '$3'" >&2; fail=1; fi
}
q() { yq eval-all "$1" -; }
# Raw env value; Kubernetes uses the last entry when a name is repeated, so take the last match.
raw() {
  q 'select(.kind=="Deployment" and .metadata.name=="enzarb-ui") | .spec.template.spec.containers[0].env | map(select(.name=="ENZARB_COMPONENT_VERSIONS")) | .[-1].value'
}
vals() { yq "$1" "$here/enzarb/values.yaml"; }

def="$("$here/scripts/render.sh" default)"
v="$(raw <<<"$def")"
check "default: valid JSON" "$(yq -p=json '. | tag' <<<"$v")" '!!map'
vd() { yq -p=json "$1" <<<"$v"; }
check "default: top-level keys" "$(vd 'keys | join(",")')" chart,components
check "default: component keys" "$(vd '.components | keys | join(",")')" enzarb-connectors,enzarb-realtime,enzarb-sandbox-container,enzarb-sandbox-operator,enzarb-sandbox-proxy,enzarb-ui,enzarb-workflows
check "default: no empty values" "$(vd '[.components[] | select(. == "" or . == null)] | length')" 0
check "default: no updater (disabled)" "$(vd '.components | has("enzarb-updater")')" false
check "default: enzarb-ui" "$(vd '.components["enzarb-ui"]')" "$(vals .ui.image.tag)"
check "default: enzarb-workflows" "$(vd '.components["enzarb-workflows"]')" "$(vals .workflows.image.tag)"
check "default: enzarb-connectors" "$(vd '.components["enzarb-connectors"]')" "$(vals .connectors.image.tag)"
check "default: enzarb-realtime" "$(vd '.components["enzarb-realtime"]')" "$(vals .realtime.image.tag)"
check "default: enzarb-sandbox-proxy" "$(vd '.components["enzarb-sandbox-proxy"]')" "$(vals .sandboxProxy.image.tag)"
check "default: enzarb-sandbox-operator" "$(vd '.components["enzarb-sandbox-operator"]')" "$(vals .operator.image.tag)"
sbx="$(vals .operator.sandboxImage)"
check "default: enzarb-sandbox-container" "$(vd '.components["enzarb-sandbox-container"]')" "${sbx##*:}"
check "default: chart" "$(vd .chart)" "$(yq .version "$here/enzarb/Chart.yaml")"

off="$("$here/scripts/render.sh" realtime-off)"
voff="$(raw <<<"$off")"
check "realtime off: no realtime key" "$(yq -p=json '.components | has("enzarb-realtime")' <<<"$voff")" false
check "realtime off: ui kept" "$(yq -p=json '.components["enzarb-ui"]' <<<"$voff")" "$(vals .ui.image.tag)"

upd="$("$here/scripts/render.sh" updater-webhook)"
check "updater on: enzarb-updater" "$(yq -p=json '.components["enzarb-updater"]' <<<"$(raw <<<"$upd")")" "$(vals .updater.image.tag)"

# A registry port, a digest, or a missing tag in operator.sandboxImage must not corrupt the sandbox version.
sb() { helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" --set-string "operator.sandboxImage=$1" | raw | yq -p=json '.components["enzarb-sandbox-container"]'; }
check "sandbox: registry port" "$(sb reg:5000/enzarb-sandbox:1.2.3)" 1.2.3
check "sandbox: digest" "$(sb ghcr.io/enzarb/enzarb-sandbox:1.2.3@sha256:abc)" 1.2.3
check "sandbox: no tag" "$(sb reg:5000/enzarb-sandbox)" reg:5000/enzarb-sandbox

# ui.extraEnv comes last, so it overrides the chart-computed value.
ovr="$(helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" \
  --set-string 'ui.extraEnv[0].name=ENZARB_COMPONENT_VERSIONS' --set-string 'ui.extraEnv[0].value=override')"
check "extraEnv override wins" "$(raw <<<"$ovr")" override

[ "$fail" = 0 ] && echo "assert-component-versions ok"
exit $fail
