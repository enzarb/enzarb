#!/usr/bin/env bash
# Unit test for docker-wrapper.sh with a fake client. Usage: scripts/test-docker-wrapper.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
printf '#!/bin/sh\necho "$*"\n' > "$tmp/docker"
chmod 0755 "$tmp/docker"
export ENZARB_DOCKER_REAL="$tmp/docker"

check() {
  want=$1
  shift
  got=$(sh "$here/docker-wrapper.sh" "$@")
  [ "$got" = "$want" ] || { echo "FAIL: docker $*: got '$got', want '$want'"; exit 1; }
}

check 'run --network=host --rm alpine true' run --rm alpine true
check 'create --network=host alpine' create alpine
check 'build --network=host -t x .' build -t x .
check 'buildx build --network=host -t x .' buildx build -t x .
check 'builder build --network=host .' builder build .
check 'container run --network=host alpine' container run alpine
# An explicit --network comes later and wins.
check 'run --network=host --network=none alpine' run --network=none alpine
# Everything else is untouched.
check 'version --format {{.Server.Version}}' version --format '{{.Server.Version}}'
check 'ps -a' ps -a
check 'compose up' compose up
check 'buildx version' buildx version
check '--version' --version
check ''
ENZARB_DOCKER_NETWORK=default check 'run --rm alpine' run --rm alpine
ENZARB_DOCKER_NETWORK=none check 'run --network=none alpine' run alpine
echo "docker-wrapper tests passed"
