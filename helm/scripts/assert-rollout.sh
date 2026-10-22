#!/usr/bin/env bash
# Assertions on zero-downtime rollout settings of the HTTP Deployments (goldens only show diffs).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-rollout: $1: got '$2', want '$3'" >&2; fail=1; fi
}
q() { yq eval-all "$1" -; }
dep() { echo 'select(.kind=="Deployment" and .metadata.name=="'"$1"'")'; }
tpl() { # extra helm args
  helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" "$@"
}

on="$("$here/scripts/render.sh" oldie)"
for d in enzarb-ui enzarb-sandbox-proxy enzarb-connectors enzarb-realtime enzarb-sandbox-operator; do
  s="$(dep "$d")"
  check "$d strategy type" "$(q "$s | .spec.strategy.type" <<<"$on")" RollingUpdate
  check "$d maxUnavailable" "$(q "$s | .spec.strategy.rollingUpdate.maxUnavailable" <<<"$on")" 0
  check "$d maxSurge" "$(q "$s | .spec.strategy.rollingUpdate.maxSurge" <<<"$on")" 1
  grace="$(q "$s | .spec.template.spec.terminationGracePeriodSeconds" <<<"$on")"
  sleep_s="$(q "$s | .spec.template.spec.containers[0].lifecycle.preStop.sleep.seconds // 0" <<<"$on")"
  if [ "$grace" = null ] || [ "$grace" -le "$sleep_s" ]; then
    echo "assert-rollout: $d: grace period '$grace' must exceed preStop sleep '$sleep_s'" >&2; fail=1
  fi
done
for d in enzarb-ui enzarb-sandbox-proxy enzarb-connectors enzarb-realtime; do
  check "$d preStop sleep" "$(q "$(dep "$d") | .spec.template.spec.containers[0].lifecycle.preStop.sleep.seconds" <<<"$on")" 10
  check "$d readiness period" "$(q "$(dep "$d") | .spec.template.spec.containers[0].readinessProbe.periodSeconds" <<<"$on")" 2
done
check "ui readiness" "$(q "$(dep enzarb-ui) | .spec.template.spec.containers[0].readinessProbe.httpGet.path" <<<"$on")" /readyz
check "sandbox-proxy readiness" "$(q "$(dep enzarb-sandbox-proxy) | .spec.template.spec.containers[0].readinessProbe.httpGet.path" <<<"$on")" /healthz
check "operator: no preStop" "$(q "$(dep enzarb-sandbox-operator) | .spec.template.spec.containers[0] | has(\"lifecycle\")" <<<"$on")" false

# ui.probes.http=false falls back to TCP readiness, still polled every 2s.
tcp="$(tpl --set ui.probes.http=false)"
ui="$(dep enzarb-ui) | .spec.template.spec.containers[0].readinessProbe"
check "ui tcp readiness port" "$(q "$ui | .tcpSocket.port" <<<"$tcp")" http
check "ui tcp readiness period" "$(q "$ui | .periodSeconds" <<<"$tcp")" 2

# preStopSeconds=0 omits the hook.
for c in ui:enzarb-ui sandboxProxy:enzarb-sandbox-proxy connectors:enzarb-connectors; do
  key="${c%%:*}"; name="${c##*:}"
  off="$(tpl --set "$key.preStopSeconds=0")"
  check "$name preStopSeconds=0" "$(q "$(dep "$name") | .spec.template.spec.containers[0] | has(\"lifecycle\")" <<<"$off")" false
  sur="$(tpl --set "$key.rollout.maxSurge=2" --set "$key.preStopSeconds=3")"
  check "$name maxSurge override" "$(q "$(dep "$name") | .spec.strategy.rollingUpdate.maxSurge" <<<"$sur")" 2
  check "$name preStop override" "$(q "$(dep "$name") | .spec.template.spec.containers[0].lifecycle.preStop.sleep.seconds" <<<"$sur")" 3
done

[ "$fail" = 0 ] && echo "assert-rollout ok"
exit $fail
