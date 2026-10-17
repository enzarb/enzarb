#!/usr/bin/env bash
# Release an image by retagging the CI-built sha-<short> image instead of rebuilding it (docs/ci.md).
#
#   retag-or-build.sh check   prints `retag` or `build`, logs the path taken, writes mode=, digest= and
#                             reason= to $GITHUB_OUTPUT and the log line to $GITHUB_STEP_SUMMARY
#   retag-or-build.sh retag   adds <version> and latest to the sha- image and fails unless both resolve
#                             to the same digest
#
# Env: IMAGE (registry/name), SHA (full commit), VERSION, RETAG (true to allow retagging), DRY_RUN.
# Retag needs a sha- image whose org.opencontainers.image.revision label is exactly SHA (sha- is a short,
# mutable tag). Anything else, including any inspect failure, means a rebuild.
set -euo pipefail

: "${IMAGE:?}" "${SHA:?}"
src="$IMAGE:sha-${SHA::7}"

emit() { # <mode> <digest> <reason>
  local line
  if [ "$1" = retag ]; then line="release path: retag of sha-${SHA::7} ($2)"
  else line="release path: rebuild ($3)"; fi
  [ "${DRY_RUN:-false}" = true ] && line="$line [dry run: would $1]"
  echo "$line" >&2
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then echo "$line" >> "$GITHUB_STEP_SUMMARY"; fi
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    { echo "mode=$1"; echo "digest=$2"; echo "reason=$3"; } >> "$GITHUB_OUTPUT"
  fi
  echo "$1"
}

check() {
  if [ "${RETAG:-false}" != true ]; then emit build "" "retag disabled for this run"; return; fi
  local digest cfg rev
  if ! digest=$(docker buildx imagetools inspect --format '{{.Manifest.Digest}}' "$src" 2>/dev/null) \
    || [ -z "$digest" ]; then
    emit build "" "no CI image $src"; return
  fi
  # .Image is the config, or a platform -> config map for an index (the first platform is read).
  if ! cfg=$(docker buildx imagetools inspect --format '{{json .Image}}' "$src" 2>/dev/null); then
    emit build "" "cannot read the config of $src"; return
  fi
  rev=$(jq -r 'if has("config") then . else (to_entries[0].value // {}) end
    | .config.Labels["org.opencontainers.image.revision"] // ""' <<<"$cfg" 2>/dev/null || true)
  if [ "$rev" != "$SHA" ]; then
    emit build "" "revision label of $src is '${rev:-none}', not $SHA"; return
  fi
  emit retag "$digest" ""
}

retag() {
  : "${VERSION:?}"
  local want got t
  want=$(docker buildx imagetools inspect --format '{{.Manifest.Digest}}' "$src")
  docker buildx imagetools create -t "$IMAGE:$VERSION" -t "$IMAGE:latest" "$src"
  for t in "$IMAGE:$VERSION" "$IMAGE:latest"; do
    got=$(docker buildx imagetools inspect --format '{{.Manifest.Digest}}' "$t")
    if [ "$got" != "$want" ]; then
      echo "::error::$t is $got, expected $want ($src)" >&2
      exit 1
    fi
  done
  echo "retagged $src ($want) as $VERSION and latest" >&2
}

case "${1:-}" in
  check) check ;;
  retag) retag ;;
  *) echo "usage: $0 check|retag" >&2; exit 2 ;;
esac
