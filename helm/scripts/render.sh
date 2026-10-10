#!/usr/bin/env bash
# Render the chart for a named scenario to stdout. Scenarios: default, dev, oldie
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:?scenario}" in
  default) extra=(--set secrets.github.appId=1 --set-string secrets.github.privateKey=pem --set 'sandbox.egress.clusterCIDRs={10.42.0.0/16,10.43.0.0/16}') ;;
  dev) extra=(-f "$here/enzarb/values-dev.yaml") ;;
  oldie) extra=(-f "$here/enzarb/values-oldie.yaml") ;;
  *) echo "unknown scenario $1" >&2; exit 2 ;;
esac
# generated secret values are random; mask them so snapshots are stable
helm template enzarb "$here/enzarb" --namespace enzarb "${extra[@]}" \
  | sed -E 's/[ \t]+$//; s/^(  (session-jwt-secret|callback-secret)): .*/\1: <generated>/' \
  | cat -s
