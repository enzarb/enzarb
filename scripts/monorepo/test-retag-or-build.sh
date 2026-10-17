#!/usr/bin/env bash
# Tests .github/scripts/retag-or-build.sh with a stub `docker` on PATH.
#
#   scripts/monorepo/test-retag-or-build.sh
set -uo pipefail
root=$(git rev-parse --show-toplevel)
script=$root/.github/scripts/retag-or-build.sh
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
cat > "$tmp/bin/docker" <<'STUB'
#!/usr/bin/env bash
# env: STUB_FAIL=1 (inspect of sha- fails), STUB_REVISION, STUB_INDEX=1 (platform map),
#      STUB_BAD_DIGEST=1 (tags resolve to another digest after create)
echo "$*" >> "$STUB_LOG"
case "$*" in
  # Like buildx: a single-manifest source is wrapped in a new index (another digest) unless
  # --prefer-index=false keeps it as is.
  *"imagetools create"*) touch "$STUB_LOG.created"
    case "$*" in *"--prefer-index=false"*) ;; *) touch "$STUB_LOG.wrapped" ;; esac
    exit 0 ;;
  *"{{json .Image}}"*)
    cfg="{\"config\":{\"Labels\":{\"org.opencontainers.image.revision\":\"$STUB_REVISION\"}}}"
    if [ "${STUB_INDEX:-}" = 1 ]; then echo "{\"linux/amd64\":$cfg}"; else echo "$cfg"; fi ;;
  *"{{.Manifest.Digest}}"*)
    case "$*" in
      *":sha-"*) [ "${STUB_FAIL:-}" = 1 ] && exit 1; echo sha256:ci ;;
      *) if [ -f "$STUB_LOG.created" ] && { [ "${STUB_BAD_DIGEST:-}" = 1 ] || [ -f "$STUB_LOG.wrapped" ]; }; then
           echo sha256:other; else echo sha256:ci; fi ;;
    esac ;;
esac
STUB
chmod +x "$tmp/bin/docker"

sha=0123456789abcdef0123456789abcdef01234567
fail=0
run() { # <mode> [VAR=value...]   -> stdout in $out, status in $rc
  local mode=$1; shift
  rm -f "$tmp/log" "$tmp/log.created" "$tmp/log.wrapped"; : > "$tmp/log"

  out=$(env PATH="$tmp/bin:$PATH" STUB_LOG="$tmp/log" STUB_REVISION="$sha" IMAGE=ghcr.io/x/y SHA="$sha" \
    VERSION=1.2.3 RETAG=true DRY_RUN=false "$@" "$script" "$mode" 2>/dev/null)
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
exit "$fail"
