#!/bin/sh
# /usr/local/bin/docker in the sandbox image (ADR-0016): the real client is
# /usr/local/libexec/docker/docker.
#
# dockerd runs with --iptables=false (gVisor), so the default bridge network has no NAT and containers
# have no outbound network. This wrapper makes `run`, `create` and `build` default to --network=host
# (the sandbox container's own network namespace, still bounded by the Project's egress NetworkPolicy).
# The default goes before the caller's arguments, so an explicit --network wins.
# ENZARB_DOCKER_NETWORK=default keeps Docker's own default; any other value replaces `host`.
real=${ENZARB_DOCKER_REAL:-/usr/local/libexec/docker/docker}
net=${ENZARB_DOCKER_NETWORK:-host}

if [ "$net" != default ]; then
  case "$1 $2" in
    "container run" | "container create" | "buildx build" | "builder build")
      a=$1 b=$2
      shift 2
      exec "$real" "$a" "$b" "--network=$net" "$@"
      ;;
  esac
  case "$1" in
    run | create | build)
      a=$1
      shift
      exec "$real" "$a" "--network=$net" "$@"
      ;;
  esac
fi
exec "$real" "$@"
