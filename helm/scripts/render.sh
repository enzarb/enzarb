#!/usr/bin/env bash
# Render the chart for a named scenario to stdout. Scenarios: default, dev, oldie, updater-webhook, realtime, realtime-replicas, realtime-off, realtime-btp, realtime-origins
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:?scenario}" in
  default) extra=(--set secrets.github.appId=1 --set-string secrets.github.privateKey=pem --set 'sandbox.egress.clusterCIDRs={10.42.0.0/16,10.43.0.0/16}') ;;
  dev) extra=(-f "$here/enzarb/values-dev.yaml") ;;
  oldie) extra=(-f "$here/enzarb/values-oldie.yaml") ;;
  updater-webhook) extra=(-f "$here/enzarb/values-oldie.yaml" --set updater.webhook.enabled=true --set secrets.updater.registryTokenSecret=ghcr-read --set updater.poll.intervalSecs=0) ;;
  realtime) extra=(-f "$here/enzarb/values-oldie.yaml") ;;
  realtime-replicas) extra=(-f "$here/enzarb/values-oldie.yaml" --set realtime.replicaCount=3) ;;
  realtime-origins) extra=(-f "$here/enzarb/values-oldie.yaml" --set 'realtime.allowedOrigins={https://a.example,https://b.example}') ;;
  realtime-off)extra=(-f "$here/enzarb/values-oldie.yaml" --set realtime.enabled=false) ;;
  realtime-btp) extra=(-f "$here/enzarb/values-oldie.yaml" --set route.backendTrafficPolicy.enabled=true) ;;
  *) echo "unknown scenario $1" >&2; exit 2 ;;
esac
# generated secret values are random; mask them so snapshots are stable
helm template enzarb "$here/enzarb" --namespace enzarb "${extra[@]}" \
  | sed -E 's/[ \t]+$//; s/^(  (session-jwt-secret|callback-secret|realtime-jwt-secret|updater-webhook-secret)): .*/\1: <generated>/' \
  | cat -s
