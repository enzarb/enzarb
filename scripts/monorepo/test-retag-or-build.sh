#!/usr/bin/env bash
# Tests .github/scripts/retag-or-build.sh with a stub `docker` on PATH.
#
#   scripts/monorepo/test-retag-or-build.sh
# shellcheck disable=SC2319  # expect reads the status of the preceding condition on purpose
set -uo pipefail
root=$(git rev-parse --show-toplevel)
script=$root/.github/scripts/retag-or-build.sh
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
fixture=$root/scripts/monorepo/fixtures/imagetools-create-wrapped.json
cat > "$tmp/bin/docker" <<'STUB'
#!/usr/bin/env bash
# env: STUB_FAIL=1 (inspect of sha- fails), STUB_REVISION, STUB_INDEX=1 (platform map),
#      STUB_BAD_DIGEST=1 (tags resolve to another digest after create),
#      STUB_NO_WRAP=1 (imagetools create does not wrap a single manifest)
echo "$*" >> "$STUB_LOG"
# The source manifest: the dry run prints these bytes and the script hashes them, so the stubbed
# digest is their real sha256. A wrapped copy is the recorded imagetools response (see
# fixtures/README.md) with the source's media type, size and digest filled in; its digest is
# the sha256 of those bytes. STUB_NO_WRAP=1 simulates a stub (or buildx) that does not wrap.
if [ "${STUB_INDEX:-}" = 1 ]; then
  type=application/vnd.oci.image.index.v1+json
else type=application/vnd.oci.image.manifest.v1+json; fi
src_json="{\"schemaVersion\":2,\"mediaType\":\"$type\"}"
ci=sha256:$(printf '%s\n' "$src_json" | sha256sum | cut -d' ' -f1)
size=$(printf '%s\n' "$src_json" | wc -c)
wrapped_json=$(sed -e "s|@TYPE@|$type|" -e "s|@DIGEST@|$ci|" -e "s|@SIZE@|$size|" "$STUB_FIXTURE")
other=sha256:$(printf '%s\n' "$wrapped_json" | sha256sum | cut -d' ' -f1)
# Like buildx: a single-manifest source is wrapped in a new index (another digest) unless
# --prefer-index=false keeps it as is; an index source stays as it is.
wrap() {
  case "$*" in *"--prefer-index=false"*) return 1 ;; esac
  [ "${STUB_INDEX:-}" = 1 ] || [ "${STUB_NO_WRAP:-}" = 1 ] && return 1
  return 0
}
case "$*" in
  *"imagetools create"*"--dry-run"*) touch "$STUB_LOG.dry"
    case "$*" in
      *) if wrap "$@"; then printf '%s\n' "$wrapped_json"; else printf '%s\n' "$src_json"; fi ;;
    esac
    exit 0 ;;
  *"imagetools create"*) touch "$STUB_LOG.created"
    wrap "$@" && touch "$STUB_LOG.wrapped"
    exit 0 ;;
  *"inspect --raw"*) printf '%s\n' "$src_json" ;;
  *"{{json .Image}}"*)
    cfg="{\"config\":{\"Labels\":{\"org.opencontainers.image.revision\":\"$STUB_REVISION\"}}}"
    if [ "${STUB_INDEX:-}" = 1 ]; then echo "{\"linux/amd64\":$cfg}"; else echo "$cfg"; fi ;;
  *"{{.Manifest.MediaType}}"*) echo "$type" ;;
  *"{{.Manifest.Digest}}"*)
    case "$*" in
      *":sha-"*) [ "${STUB_FAIL:-}" = 1 ] && exit 1; echo "$ci" ;;
      *) if [ -f "$STUB_LOG.created" ] && { [ "${STUB_BAD_DIGEST:-}" = 1 ] || [ -f "$STUB_LOG.wrapped" ]; }; then
           echo "$other"; else echo "$ci"; fi ;;
    esac ;;
esac
STUB
chmod +x "$tmp/bin/docker"

sha=0123456789abcdef0123456789abcdef01234567
fail=0
run() { # <mode> [VAR=value...]   -> stdout in $out, status in $rc
  local mode=$1; shift
  rm -f "$tmp/log" "$tmp/log.created" "$tmp/log.wrapped" "$tmp/log.dry" "$tmp/summary"; : > "$tmp/log"

  out=$(env PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_FIXTURE="$fixture" STUB_REVISION="$sha" IMAGE=ghcr.io/x/y SHA="$sha" \
    VERSION=1.2.3 RETAG=true DRY_RUN=false GITHUB_STEP_SUMMARY="$tmp/summary" "$@" "${script_under_test:-$script}" "$mode" 2>/dev/null)
  rc=$?

}
expect() { # <name> <condition result 0|1>
  if [ "$2" = 0 ]; then echo "ok   $1"; else echo "FAIL $1 (out=$out rc=$rc)"; fail=1; fi
}

run check;                           [ "$out" = retag ] && [ $rc = 0 ];  expect "matching revision -> retag" $?
run check STUB_INDEX=1;              [ "$out" = retag ];                 expect "index image -> retag" $?
run check STUB_FAIL=1;               [ "$out" = build ];                 expect "no CI image -> build" $?
run check STUB_REVISION=deadbeef;    [ "$out" = build ];                 expect "revision mismatch -> build" $?
run check RETAG=false;               [ "$out" = build ];                 expect "RETAG=false -> build" $?
run check DRY_RUN=true;              [ "$out" = retag ] && ! grep -q create "$tmp/log"
expect "dry run check makes no create call" $?
run retag;                           [ $rc = 0 ] && grep -q "create --prefer-index=false -t ghcr.io/x/y:1.2.3 -t ghcr.io/x/y:latest ghcr.io/x/y:sha-0123456" "$tmp/log"
expect "retag creates version and latest" $?
run retag STUB_BAD_DIGEST=1;         [ $rc != 0 ];                       expect "digest mismatch after retag fails" $?

run verify;                          [ $rc = 0 ] && [ ! -f "$tmp/log.created" ] && grep -q "create --dry-run --prefer-index=false" "$tmp/log" \
  && grep -q 'ghcr.io/x/y:sha-0123456' "$tmp/summary" && grep -q 'application/vnd.oci.image.manifest.v1+json' "$tmp/summary" && grep -q 'sha256:' "$tmp/summary"
expect "verify of a single-manifest source passes, summarises it and pushes nothing" $?
run verify STUB_INDEX=1;             [ $rc = 0 ];                        expect "verify of an index source passes" $?
run retag DRY_RUN=true;              [ $rc = 0 ] && [ -f "$tmp/log.dry" ] && [ ! -f "$tmp/log.created" ]
expect "retag with DRY_RUN=true only dry-runs create" $?

# The stub itself: a dry-run create of a single manifest prints an index wrapping it.
wrap_test() { # [VAR=value...]: 0 when the stub wraps the way buildx does
  local src wrapped
  src=$(env PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_FIXTURE="$fixture" "$@" docker buildx imagetools create --dry-run --prefer-index=false -t x/y:1 x/y:sha-1) || return 1
  wrapped=$(env PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_FIXTURE="$fixture" "$@" docker buildx imagetools create --dry-run -t x/y:1 x/y:sha-1) || return 1
  [ "$(jq -r .mediaType <<<"$wrapped")" = application/vnd.oci.image.index.v1+json ] &&
    [ "$(jq '.manifests | length' <<<"$wrapped")" = 1 ] &&
    [ "$(jq -r '.manifests[0].digest' <<<"$wrapped")" = "sha256:$(printf '%s\n' "$src" | sha256sum | cut -d' ' -f1)" ] &&
    [ "$(printf '%s\n' "$wrapped" | sha256sum)" != "$(printf '%s\n' "$src" | sha256sum)" ]
}
wrap_test; expect "stub wraps a single manifest in an index" $?
! wrap_test STUB_NO_WRAP=1; expect "wrap test fails when the stub does not wrap" $?
src=$(env PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_FIXTURE="$fixture" docker buildx imagetools create --dry-run --prefer-index=false x)
[ "$(jq -r .mediaType <<<"$src")" = application/vnd.oci.image.manifest.v1+json ]
expect "--prefer-index=false keeps the source manifest" $?

# Negative test: the script without --prefer-index=false must fail the check.
sed 's/ --prefer-index=false//' "$script" > "$tmp/no-prefer-index.sh"; chmod +x "$tmp/no-prefer-index.sh"
cmp -s "$script" "$tmp/no-prefer-index.sh" && { echo "FAIL negative test: flag not stripped"; fail=1; }
script_under_test=$tmp/no-prefer-index.sh
err=$(env PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_FIXTURE="$fixture" STUB_REVISION="$sha" IMAGE=ghcr.io/x/y SHA="$sha" \
  "$script_under_test" verify 2>&1 >/dev/null); rc=$?; out=$err
[ $rc != 0 ] && grep -q "would push" <<<"$err"; expect "verify fails without --prefer-index=false" $?
exit "$fail"
