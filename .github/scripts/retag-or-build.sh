#!/usr/bin/env bash
# Release an image by retagging the CI-built sha-<short> image instead of rebuilding it (docs/ci.md).
#
#   retag-or-build.sh check   prints `retag` or `build`, logs the path taken, writes mode=, digest= and
#                             reason= to $GITHUB_OUTPUT and the log line to $GITHUB_STEP_SUMMARY
#   retag-or-build.sh retag   adds <version> and latest to the sha- image and fails unless both resolve
#                             to the same digest (with DRY_RUN=true it runs `verify` instead: no push)
#   retag-or-build.sh verify  runs the same `imagetools create` with --dry-run and fails unless the manifest
#                             it would push has the source's media type and digest (a single-manifest source
#                             must not be wrapped in an index); writes the result to $GITHUB_STEP_SUMMARY
#                             and mode=, digest=, type= to $GITHUB_OUTPUT. Pushes nothing, VERSION optional.
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

# The create invocation shared by retag and verify, so the dry run checks exactly what is pushed.
# --prefer-index=false: a single-manifest source stays that manifest (same digest) instead of
# being wrapped in a new index; an index source is copied as is either way.
create_args() {
  create=(docker buildx imagetools create --prefer-index=false -t "$IMAGE:$1" -t "$IMAGE:latest" "$src")
}

retag() {
  : "${VERSION:?}"
  if [ "${DRY_RUN:-false}" = true ]; then verify; return; fi
  local want got t create
  want=$(docker buildx imagetools inspect --format '{{.Manifest.Digest}}' "$src")
  create_args "$VERSION"
  "${create[@]}"
  for t in "$IMAGE:$VERSION" "$IMAGE:latest"; do
    got=$(docker buildx imagetools inspect --format '{{.Manifest.Digest}}' "$t")
    if [ "$got" != "$want" ]; then
      echo "::error::$t is $got, expected $want ($src)" >&2
      exit 1
    fi
  done
  echo "retagged $src ($want) as $VERSION and latest" >&2
}

verify() {
  local want_digest want_type create plan got_digest got_type
  want_digest=$(docker buildx imagetools inspect --format '{{.Manifest.Digest}}' "$src")
  want_type=$(docker buildx imagetools inspect --format '{{.Manifest.MediaType}}' "$src")
  create_args "${VERSION:-dry-run}"
  plan=$(mktemp)
  # shellcheck disable=SC2064
  trap "rm -f '$plan'" RETURN
  "${create[@]:0:4}" --dry-run "${create[@]:4}" > "$plan"
  got_type=$(jq -r '.mediaType // ""' "$plan")
  if jq -e 'has("schemaVersion") | not' "$plan" >/dev/null 2>&1; then
    # A descriptor ({mediaType, digest, size}) rather than the manifest body.
    got_digest=$(jq -r '.digest // ""' "$plan")
  else
    # The printed body is re-serialised by buildx, so its bytes need not hash to the source digest:
    # compare the canonical JSON with the source manifest, and report the source digest if equal.
    if [ "$(jq -S -c . "$plan")" = "$(docker buildx imagetools inspect --raw "$src" | jq -S -c .)" ]; then
      got_digest=$want_digest
    else
      got_digest="sha256:$(sha256sum "$plan" | cut -d' ' -f1)"
    fi
  fi
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
      echo "### Retag dry run"
      echo "| source image | manifest type | digest |"
      echo "|---|---|---|"
      echo "| \`$src\` | \`$want_type\` | \`$want_digest\` |"
      echo
      echo "planned: type \`$got_type\`, digest \`$got_digest\`"
    } >> "$GITHUB_STEP_SUMMARY"
  fi
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    { echo "mode=verify"; echo "digest=$want_digest"; echo "type=$want_type"; } >> "$GITHUB_OUTPUT"
  fi
  if [ "$got_digest" != "$want_digest" ] || [ "$got_type" != "$want_type" ]; then
    echo "::error::retag of $src would push $got_type $got_digest, expected $want_type $want_digest (is --prefer-index=false missing?)" >&2
    exit 1
  fi
  echo "verified: retag of $src keeps $want_type $want_digest" >&2
}

case "${1:-}" in
  check) check ;;
  retag) retag ;;
  verify) verify ;;
  *) echo "usage: $0 check|retag|verify" >&2; exit 2 ;;
esac
