#!/usr/bin/env bash
# Assertions on the rendered enzarb-realtime resources (golden snapshots only show diffs).
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
fail=0
check() { # description, actual, expected
  if [ "$2" != "$3" ]; then echo "assert-realtime: $1: got '$2', want '$3'" >&2; fail=1; fi
}
q() { yq eval-all "$1" -; }

on="$("$here/scripts/render.sh" realtime)"
check "replicas" "$(q 'select(.kind=="Deployment" and .metadata.name=="enzarb-realtime") | .spec.replicas' <<<"$on")" 2
check "readiness" "$(q 'select(.kind=="Deployment" and .metadata.name=="enzarb-realtime") | .spec.template.spec.containers[0].readinessProbe.httpGet.path' <<<"$on")" /readyz
check "liveness" "$(q 'select(.kind=="Deployment" and .metadata.name=="enzarb-realtime") | .spec.template.spec.containers[0].livenessProbe.httpGet.path' <<<"$on")" /healthz
check "service port" "$(q 'select(.kind=="Service" and .metadata.name=="enzarb-realtime") | .spec.ports[0].port' <<<"$on")" 8080
env_of() { q 'select(.kind=="Deployment" and .metadata.name=="'"$1"'") | .spec.template.spec.containers[0].env[] | select(.name=="'"$2"'")' <<<"$on"; }
check "PGDATABASE" "$(env_of enzarb-realtime PGDATABASE | q .value)" enzarb
check "DATABASE_URL" "$(env_of enzarb-realtime DATABASE_URL | q .value)" postgres:///enzarb
for v in PGHOST PGPORT PGUSER PGPASSWORD PGSSLMODE; do
  check "$v secret" "$(env_of enzarb-realtime "$v" | q .valueFrom.secretKeyRef.name)" enzarb-pg-realtime-credentials
done
check "allowed origins" "$(env_of enzarb-realtime ENZARB_REALTIME_ALLOWED_ORIGINS | q .value)" https://enzarb.dev
rt_ref="$(env_of enzarb-realtime ENZARB_REALTIME_JWT_SECRET | q .valueFrom.secretKeyRef)"
ui_ref="$(env_of enzarb-ui ENZARB_REALTIME_JWT_SECRET | q .valueFrom.secretKeyRef)"
check "jwt ref (ui == realtime)" "$ui_ref" "$rt_ref"
check "jwt ref name" "$(q .name <<<"$rt_ref")" enzarb-generated
check "jwt ref key" "$(q .key <<<"$rt_ref")" realtime-jwt-secret
check "jwt key generated" "$(q 'select(.kind=="Secret" and .metadata.name=="enzarb-generated") | .data | has("realtime-jwt-secret")' <<<"$on")" true
check "route path" "$(q 'select(.kind=="HTTPRoute" and .metadata.name=="enzarb-realtime") | .spec.rules[0].matches[0].path.value' <<<"$on")" /live/
check "route backend" "$(q 'select(.kind=="HTTPRoute" and .metadata.name=="enzarb-realtime") | .spec.rules[0].backendRefs[0].name' <<<"$on")" enzarb-realtime
check "route timeout" "$(q 'select(.kind=="HTTPRoute" and .metadata.name=="enzarb-realtime") | .spec.rules[0].timeouts.request' <<<"$on")" 0s
check "pgop role" "$(q 'select(.kind=="Role" and .metadata.name=="realtime") | .spec.roleName' <<<"$on")" enzarb_realtime

dep='select(.kind=="Deployment" and .metadata.name=="enzarb-realtime")'
grace="$(q "$dep | .spec.template.spec.terminationGracePeriodSeconds" <<<"$on")"
check "grace period" "$grace" 30
if [ "$grace" -lt 10 ]; then echo "assert-realtime: grace period $grace < 10" >&2; fail=1; fi

# Enabled by default; no realtime BackendTrafficPolicy unless the opt-in flag is set.
def="$("$here/scripts/render.sh" default)"
check "default: deployment" "$(q "$dep | .metadata.name" <<<"$def")" enzarb-realtime
check "default: no realtime BTP" "$(q 'select(.kind=="BackendTrafficPolicy") | .metadata.name' <<<"$def")" ""

origins_env='.spec.template.spec.containers[0].env[] | select(.name=="ENZARB_REALTIME_ALLOWED_ORIGINS") | .value'
check "default: allowed origins" "$(q "$dep | $origins_env" <<<"$def")" https://enzarb.local
ori="$("$here/scripts/render.sh" realtime-origins)"
check "explicit allowed origins" "$(q "$dep | $origins_env" <<<"$ori")" https://a.example,https://b.example
norm="$(helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" --set publicUrl=https://enzarb.dev/app/)"
check "publicUrl path stripped" "$(q "$dep | $origins_env" <<<"$norm")" https://enzarb.dev

# Origins enzarb-realtime would refuse must fail at render time.
for bad in 'https://u@h' 'https://h?x' 'https://h#f' 'https://h:' 'https://h/' 'https://h/p' ' https://h'; do
  if helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" --set-string "realtime.allowedOrigins[0]=$bad" >/dev/null 2>&1; then
    echo "assert-realtime: allowedOrigins '$bad' rendered, want failure" >&2; fail=1
  fi
done
if helm template enzarb "$here/enzarb" --namespace enzarb -f "$here/enzarb/values-oldie.yaml" --set publicUrl=https://u@enzarb.dev/ >/dev/null 2>&1; then
  echo "assert-realtime: publicUrl with userinfo rendered, want failure" >&2; fail=1
fi

btp="$("$here/scripts/render.sh" realtime-btp)"
bsel='select(.kind=="BackendTrafficPolicy" and .metadata.name=="enzarb-realtime")'
check "btp target kind" "$(q "$bsel | .spec.targetRefs[0].kind" <<<"$btp")" HTTPRoute
check "btp target name" "$(q "$bsel | .spec.targetRefs[0].name" <<<"$btp")" enzarb-realtime
check "btp idle timeout" "$(q "$bsel | .spec.timeout.http.connectionIdleTimeout" <<<"$btp")" 2400s
check "btp sandbox-proxy kept" "$(q 'select(.kind=="BackendTrafficPolicy" and .metadata.name=="enzarb-sandbox-proxy") | .metadata.name' <<<"$btp")" enzarb-sandbox-proxy

off="$("$here/scripts/render.sh" realtime-off)"
for k in 'Deployment enzarb-realtime' 'Service enzarb-realtime' 'HTTPRoute enzarb-realtime' 'Role realtime' 'ServiceAccount enzarb-realtime'; do
  set -- $k
  check "disabled: no $k" "$(q 'select(.kind=="'"$1"'" and .metadata.name=="'"$2"'") | .metadata.name' <<<"$off")" ""
done
check "disabled: no UI env" "$(q 'select(.kind=="Deployment" and .metadata.name=="enzarb-ui") | .spec.template.spec.containers[0].env[] | select(.name=="ENZARB_REALTIME_JWT_SECRET") | .name' <<<"$off")" ""
check "disabled: no jwt key" "$(q 'select(.kind=="Secret" and .metadata.name=="enzarb-generated") | .data | has("realtime-jwt-secret")' <<<"$off")" false
if grep -q realtime <<<"$off"; then
  echo "assert-realtime: disabled render still mentions realtime" >&2; fail=1
fi

# replicaCount=3 changes only the replica count.
diff_lines="$(diff <(printf '%s\n' "$on") <("$here/scripts/render.sh" realtime-replicas) | grep -c '^[<>]' || true)"
check "replicas override diff lines" "$diff_lines" 2
check "replicas override" "$("$here/scripts/render.sh" realtime-replicas | q 'select(.kind=="Deployment" and .metadata.name=="enzarb-realtime") | .spec.replicas')" 3


[ "$fail" = 0 ] && echo "assert-realtime ok"
exit $fail
