#!/usr/bin/env bash
# Reconcile enzarb/values.yaml with the latest release tag (<dir>/vX.Y.Z, read from local git) of
# every component in scripts/components.yaml. Idempotent: a second run changes nothing. A component
# only advances when its image exists, i.e. when the `ci-<dir>` run of the release tag succeeded (its
# docker job pushes ghcr.io/enzarb/<image>:<version> on `<dir>/v*` tags), and never to an older
# version than the current pin.
#
#   helm/scripts/sync-components.sh [--message-file FILE] [--dry-run]
#
# Writes a Conventional Commit message to FILE when something changed: `feat(deps)!:` for a major
# step, `feat(deps):` for minor, `fix(deps):` for patch (the largest step wins), which the release
# workflow turns into the next chart version. A component without any tag is skipped.
# Needs: a clone with tags (fetch-depth 0), yq (mikefarah v4), and gh with GH_TOKEN (Actions read on
# this repo) for the tag-build check; SKIP_IMAGE_CHECK=1 skips that check (no gh needed).
set -euo pipefail
# NOTE: when the operator pin bumps, also diff operator/deploy/rbac.yaml against the
# operator Role/ClusterRole in enzarb/templates/rbac.yaml (v0.3.0 added pods get,delete; see #24).
here="$(cd "$(dirname "$0")/.." && pwd)"
values="$here/enzarb/values.yaml"
root="$(git -C "$here" rev-parse --show-toplevel)"
msg_file="" dry=0
while [ $# -gt 0 ]; do
  case "$1" in
    --message-file) msg_file=$2; shift 2 ;;
    --dry-run) dry=1; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
done

err=$(mktemp); tmp=""
trap 'rm -f "$err" "$tmp"' EXIT

# Prints the highest <dir>/vX.Y.Z tag (plain X.Y.Z only), or nothing when the component has none.
latest_tag() { # <dir>
  git -C "$root" tag --list "$1/v[0-9]*" | grep -E "^$1/v[0-9]+\.[0-9]+\.[0-9]+\$" | sort -V | tail -1 || true
}
# True when the tag's ci-<dir> run succeeded (the docker job pushed the image).
image_built() { # <dir> <tag>
  [ "${SKIP_IMAGE_CHECK:-0}" = 1 ] && return 0
  local c
  c=$(gh run list -R "${GITHUB_REPOSITORY:-enzarb/enzarb}" --workflow "ci-$1.yml" --branch "$2" --limit 1 \
    --json conclusion --jq '.[0].conclusion // ""' 2>"$err") || { cat "$err" >&2; echo "$1: cannot read the ci run of $2" >&2; exit 1; }
  [ "$c" = success ]
}
# Prints major|minor|patch for old -> new (only called when new is newer).
step() {
  local IFS=.; read -r o1 o2 _ <<<"${1%%[-+]*}"; read -r n1 n2 _ <<<"${2%%[-+]*}"
  if [ "$n1" != "$o1" ]; then echo major; elif [ "$n2" != "$o2" ]; then echo minor; else echo patch; fi
}
rank() { case $1 in major) echo 3 ;; minor) echo 2 ;; *) echo 1 ;; esac; }
is_newer() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$2" ]; } # <current> <new>
# Replaces the value at <path> (e.g. .ui.image.tag) in place, keeping comments and blank lines
# (`yq -i` rewrites the file and drops blank lines, and yq's `line` operator is unreliable after
# header comments). values.yaml indents by two spaces; the result is verified by reading it back.
set_value() { # <path> <value>
  awk -v path="${1#.}" -v val="$2" '
    BEGIN { n = split(path, k, ".") }
    /^[[:space:]]*(#|$)/ { print; next }
    {
      match($0, /^ */); lvl = RLENGTH / 2
      if (!match($0, /^ *[^: ]+:/)) { print; next }
      stack[lvl] = substr($0, lvl * 2 + 1, RLENGTH - lvl * 2 - 1)
      if (lvl + 1 == n && !done) {
        ok = 1
        for (i = 0; i < n; i++) if (stack[i] != k[i + 1]) ok = 0
        if (ok) { sub(/:[[:space:]].*$/, ": " val); done = 1 }
      }
      print
    }
    END { exit done ? 0 : 3 }' "$values" > "$values.new" || { rm -f "$values.new"; echo "could not find $1 in values.yaml" >&2; exit 1; }
  mv "$values.new" "$values"
  [ "$(yq "$1" "$values")" = "$2" ] || { echo "failed to set $1 to $2" >&2; exit 1; }
}

changes=() worst=patch
n=$(yq '.components | length' "$here/scripts/components.yaml")
for i in $(seq 0 $((n - 1))); do
  c() { yq ".components[$i].$1 // \"\"" "$here/scripts/components.yaml"; }
  repo=$(c dir) image=$(c image) path=$(c path) prefix=$(c prefix) crd=$(c crd)
  latest=$(latest_tag "$repo")
  if [ -z "$latest" ]; then echo "$repo: no release yet, keeping current"; continue; fi
  new=${latest#"$repo"/v}
  current=$(yq "$path" "$values"); current=${current#"$prefix"}
  if [ "$new" = "$current" ]; then echo "$repo: $current (up to date)"; continue; fi
  if ! is_newer "$current" "$new"; then echo "$repo: latest release $new is older than the pinned $current, keeping $current"; continue; fi
  if ! image_built "$repo" "$latest"; then
    echo "$repo: $new released but its tag build has not succeeded (ghcr.io/enzarb/$image:$new), keeping $current"; continue
  fi
  lvl=$(step "$current" "$new")
  [ "$(rank "$lvl")" -gt "$(rank "$worst")" ] && worst=$lvl
  echo "$repo: $current -> $new ($lvl)"
  changes+=("$repo ${current} -> ${new}")
  [ "$dry" = 1 ] && continue
  if [ "$crd" = true ]; then # fetch before editing anything, so a failure leaves values.yaml untouched
    tmp=$(mktemp)
    git -C "$root" show "$latest:$repo/deploy/crds/project.yaml" > "$tmp"
  fi
  set_value "$path" "$prefix$new"
  if [ "$crd" = true ]; then "$here/scripts/sync-crds.sh" "$tmp" >/dev/null; fi
done

if [ ${#changes[@]} -eq 0 ]; then echo "nothing to do"; exit 0; fi
case $worst in major) type='feat(deps)!' ;; minor) type='feat(deps)' ;; *) type='fix(deps)' ;; esac
if [ -n "$msg_file" ] && [ "$dry" != 1 ]; then
  {
    echo "$type: bump $(printf '%s, ' "${changes[@]%% *}" | sed 's/, $//')"
    echo
    printf -- '- %s\n' "${changes[@]}"
  } > "$msg_file"
fi
echo "changed: ${#changes[@]} component(s), commit type $type"
