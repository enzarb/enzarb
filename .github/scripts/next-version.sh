#!/usr/bin/env bash
# Release version computation for one monorepo component.
#
#   .github/scripts/next-version.sh <dir> [<sha>]   prints "<last-tag> <next-version>"
#                                                   (<last-tag> is "-" before the first release,
#                                                   <next-version> is "-" when nothing is releasable)
#
# Sourceable: defines deps_of, release_paths, last_tag and next_version.
#
# Rules (unchanged from the per-repo release.yml): Conventional Commits since <dir>/vX.Y.Z that touch
# the component dir or one of its path-dependency dirs. feat -> minor, fix/perf/revert -> patch,
# `type!:` or a BREAKING CHANGE footer -> major (while 0.x, breaking and feat both bump minor).
# chore/docs/ci/test/refactor-only history releases nothing. First release is 0.1.0.
set -euo pipefail

# Path dependencies come from scripts/monorepo/graph.generated.sh (scripts/monorepo/gen-graph.sh).
# shellcheck source=scripts/monorepo/graph.generated.sh
source "$(dirname "${BASH_SOURCE[0]}")/../../scripts/monorepo/graph.generated.sh"

deps_of() { # <dir>   path-dependency dirs and non-code inputs (git pathspecs), empty if unknown
  [[ $1 =~ ^[A-Za-z0-9_-]+$ ]] || return 0
  local -n _deps="DEPS_${1//-/_}"
  echo "${_deps[@]+"${_deps[@]}"}"
}

release_paths() { # <dir>
  # shellcheck disable=SC2046
  echo "$1" $(deps_of "$1")
}

last_tag() { # <dir> <sha>
  git describe --tags --abbrev=0 --match "$1/v[0-9]*" "$2" 2>/dev/null || true
}

next_version() { # <dir> <sha>   (inside a full clone with tags)
  local dir=$1 sha=$2 last major=0 minor=0 patch=0 subj msg ver M m p
  last=$(last_tag "$dir" "$sha")
  if [ -z "$last" ]; then echo "0.1.0"; return; fi   # first release
  # shellcheck disable=SC2046
  while IFS= read -r -d '' msg; do
    subj=${msg%%$'\n'*}
    if [[ $subj =~ ^[a-z]+(\([^\)]*\))?!: ]] || [[ $msg == *$'\n'BREAKING[\ -]CHANGE:* ]]; then major=1
    elif [[ $subj =~ ^feat(\([^\)]*\))?: ]]; then minor=1
    elif [[ $subj =~ ^(fix|perf|revert)(\([^\)]*\))?: ]]; then patch=1
    fi
  done < <(git log -z --ancestry-path --format=%B "$last..$sha" -- $(release_paths "$dir"))
  [ $((major + minor + patch)) -eq 0 ] && return   # chore/docs/ci/test only
  IFS=. read -r M m p <<<"${last#"$dir"/v}"
  if [ "$major" = 1 ]; then
    if [ "$M" = 0 ]; then ver="0.$((m + 1)).0"; else ver="$((M + 1)).0.0"; fi   # 0.x: breaking bumps minor
  elif [ "$minor" = 1 ]; then
    if [ "$M" = 0 ]; then ver="0.$((m + 1)).0"; else ver="$M.$((m + 1)).0"; fi
  else ver="$M.$m.$((p + 1))"; fi
  echo "$ver"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  [ $# -ge 1 ] || { echo "usage: $0 <dir> [<sha>]" >&2; exit 2; }
  sha=${2:-HEAD}
  l=$(last_tag "$1" "$sha"); n=$(next_version "$1" "$sha")
  echo "${l:--} ${n:--}"
fi
