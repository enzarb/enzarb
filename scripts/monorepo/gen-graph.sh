#!/usr/bin/env bash
# Generates scripts/monorepo/graph.generated.sh: per-component dependency closure, the affected.sh
# RULES, the release path lists (next-version.sh deps_of) and the image table (image-components.sh).
# It also rewrites the `# BEGIN generated:copy` .. `# END generated:copy` block of each image Dockerfile
# (every workspace member for Rust images, the `file:` dependency closure for ui, minus .dockerignore).
#
#   scripts/monorepo/gen-graph.sh                 write the generated file and the Dockerfile COPY blocks
#   scripts/monorepo/gen-graph.sh --check         fail when a committed generated file is stale
#   scripts/monorepo/gen-graph.sh --root <dir>    operate on another tree (tests)
#
# Inputs: `cargo metadata` path deps (root workspace), package.json `file:`/`link:` deps, and the hand-written
# scripts/monorepo/components.json (images, non-code inputs, workflow files). Needs cargo and jq; no network.
# shellcheck disable=SC2016 # jq programs, not shell expansions
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
check=0
while [ $# -gt 0 ]; do
  case $1 in
    --check) check=1 ;;
    --root) shift; root=$(cd "${1:?--root needs a directory}" && pwd) ;;
    *) echo "usage: $0 [--check] [--root <dir>]" >&2; exit 2 ;;
  esac
  shift
done
manifest=$root/scripts/monorepo/components.json
out=$root/scripts/monorepo/graph.generated.sh

m() { jq -r "$@" "$manifest"; }
comp() { echo "${1%%/*}"; }
under() { [[ $1 == "$2" || $1 == "$2"/* ]]; } # <path> <dir>

mapfile -t order < <(m '.components | keys_unsorted[]')
mapfile -t granular < <(m '.crate_granular // [] | .[]')

# --- manifest hygiene: every glob must match a file ---------------------------------------------
mapfile -t files < <(cd "$root" && find . \( -name .git -o -name node_modules -o -name target \) -prune -o -type f -print | sed 's|^\./||')
bad=0
while IFS= read -r glob; do
  hit=0
  for f in "${files[@]}"; do
    # shellcheck disable=SC2254 # the glob is meant to be a pattern
    case $f in $glob) hit=1; break ;; esac
  done
  if [ "$hit" -eq 0 ]; then echo "error: components.json glob matches no file: $glob" >&2; bad=1; fi
done < <(m '[.rust_root_files[]?, (.components[] | (.extra_inputs[]?, .workflow_files[]?, .globs[]?))] | unique[]')
[ "$bad" -eq 0 ] || exit 1

# --- edges ---------------------------------------------------------------------------------------
# crates: workspace member dirs; edges: "<from> <to>" between member/path-dep dirs (relative to the root)
meta=$(cd "$root" && cargo metadata --offline --no-deps --format-version 1)
mapfile -t crates < <(jq -r '.workspace_root as $w | .packages[] | .manifest_path | sub("/Cargo.toml$"; "") | sub("^" + $w + "/"; "")' <<<"$meta" | sort -u)
edges=$(jq -r '.workspace_root as $w | .packages[] | (.manifest_path | sub("/Cargo.toml$"; "") | sub("^" + $w + "/"; "")) as $f
  | .dependencies[] | select(.path != null and (.path | startswith($w + "/"))) | "\($f) \(.path | sub("^" + $w + "/"; ""))"' <<<"$meta")
while IFS= read -r pj; do
  dir=$(dirname "$pj")
  while IFS= read -r v; do
    [ -n "$v" ] || continue
    printf -v edges '%s\n%s %s' "$edges" "${dir#./}" "$(realpath -m --relative-to="$root" "$root/$dir/${v#*:}")"
  done < <(jq -r '[.dependencies, .devDependencies, .optionalDependencies, .peerDependencies] | map(select(. != null) | .[]) | .[] | select(startswith("file:") or startswith("link:"))' "$root/$pj")
done < <(cd "$root" && find . \( -name .git -o -name node_modules -o -name target \) -prune -o -name package.json -print | sed 's|^\./||')
edges=$(sed '/^$/d' <<<"$edges" | sort -u)
# npm `file:`/`link:` edges only (the ui Dockerfile copies those packages, not the Cargo path deps)
npm_edges=$(
  while IFS= read -r pj; do
    dir=$(dirname "$pj")
    while IFS= read -r v; do
      [ -n "$v" ] || continue
      echo "${dir#./} $(realpath -m --relative-to="$root" "$root/$dir/${v#*:}")"
    done < <(jq -r '[.dependencies, .devDependencies, .optionalDependencies, .peerDependencies] | map(select(. != null) | .[]) | .[] | select(startswith("file:") or startswith("link:"))' "$root/$pj")
  done < <(cd "$root" && find . \( -name .git -o -name node_modules -o -name target \) -prune -o -name package.json -print | sed 's|^\./||') | sort -u
)

is_granular() { local g; for g in "${granular[@]}"; do [ "$g" = "$1" ] && return 0; done; return 1; }

direct() { # <entry>: entries it depends on (component dirs, or crate dirs for granular ones)
  local entry=$1 f t te
  while read -r f t; do
    [ -n "$f" ] || continue
    under "$f" "$entry" || continue
    under "$t" "$entry" && continue
    if is_granular "$t" || { [ "$(comp "$entry")" = "$(comp "$t")" ] && [ "$entry" != "$(comp "$entry")" ]; }; then te=$t; else te=$(comp "$t"); fi
    echo "$te"
  done <<<"$edges"
}

closure() { # <component>: sorted entries of the transitive dependency closure, without the component itself
  local c=$1 queue=("$1") seen=" $1 " e d
  local -a result=()
  while [ ${#queue[@]} -gt 0 ]; do
    e=${queue[0]}; queue=("${queue[@]:1}")
    while IFS= read -r d; do
      [ -n "$d" ] || continue
      case $seen in *" $d "*) continue ;; esac
      seen="$seen$d "
      under "$d" "$c" && continue
      result+=("$d"); queue+=("$d")
    done < <(direct "$e")
  done
  local r p keep
  for r in "${result[@]}"; do # drop crate entries already covered by a component entry
    keep=1
    for p in "${result[@]}"; do [ "$p" != "$r" ] && under "$r" "$p" && keep=0; done
    [ "$keep" -eq 1 ] && echo "$r"
  done | sort -u
}

has_crate() { local k; for k in "${crates[@]}"; do under "$k" "$1" && return 0; done; return 1; }

# --- Dockerfile COPY blocks ------------------------------------------------------------------------
mapfile -t dockerignored < <(sed -e 's/#.*//' -e 's/[[:space:]]*$//' -e '/^$/d' "$root/.dockerignore" 2>/dev/null || true)
copyable() { local i; for i in "${dockerignored[@]}"; do [ "$i" = "$1" ] && return 1; done; return 0; } # not in .dockerignore

npm_closure() { # <dir>: transitive `file:` dependency dirs
  local queue=("$1") seen=" $1 " cur f t
  while [ ${#queue[@]} -gt 0 ]; do
    cur=${queue[0]}
    queue=("${queue[@]:1}")
    while read -r f t; do
      [ "$f" = "$cur" ] || continue
      case $seen in *" $t "*) continue ;; esac
      seen="$seen$t "
      queue+=("$t")
      echo "$t"
    done <<<"$npm_edges"
  done | sort -u
}

docker_kind() { # <component>: rust, npm or none
  if has_crate "$1"; then echo rust
  elif [ -n "$(npm_closure "$1")" ]; then echo npm
  else echo none; fi
}

docker_block() { # <component>: COPY lines
  local c=$1 d s
  local -a src
  case $(docker_kind "$c") in
    rust)
      echo "COPY $(m '.rust_root_files | join(" ")') ./"
      { printf '%s\n' "${crates[@]}"; m '.docker_extra_dirs[]?'; } | sort -u | while IFS= read -r d; do
        if copyable "$d" && [ -d "$root/$d" ]; then echo "COPY $d $d"; fi
      done
      ;;
    npm)
      while IFS= read -r d; do
        if [ -f "$root/$d/pnpm-lock.yaml" ]; then echo "COPY $d/package.json $d/pnpm-lock.yaml $d/.npmrc /repo/$d/"; else echo "COPY $d/package.json /repo/$d/"; fi
        mapfile -t src < <(m --arg c "$c" --arg d "$d" '.components[$c].docker_npm_src[$d][]?')
        if [ ${#src[@]} -eq 0 ]; then echo "COPY $d /repo/$d"; else for s in "${src[@]}"; do echo "COPY $d/$s /repo/$d/$s"; done; fi
      done < <(npm_closure "$c")
      ;;
  esac
}

# render_dockerfile <component>: the Dockerfile with its generated block replaced
render_dockerfile() {
  local c=$1 f=$root/$1/Dockerfile bf
  if ! { grep -q '^# BEGIN generated:copy' "$f" && grep -q '^# END generated:copy' "$f"; }; then
    echo "error: ${f#"$root"/} has no '# BEGIN generated:copy' / '# END generated:copy' markers" >&2
    return 1
  fi
  bf=$(mktemp)
  docker_block "$c" >"$bf"
  awk -v bf="$bf" '/^# BEGIN generated:copy/ { print; while ((getline l < bf) > 0) print l; close(bf); skip = 1; next }
    /^# END generated:copy/ { skip = 0 } !skip { print }' "$f"
  rm -f "$bf"
}

dockerfile_components() { # image components that have a Dockerfile to generate a block for
  local c
  for c in "${order[@]}"; do
    [ "$(m --arg c "$c" '.components[$c].image != null')" = true ] || continue
    [ -f "$root/$c/Dockerfile" ] || continue
    [ "$(docker_kind "$c")" = none ] || echo "$c"
  done
}

generate() {
  local c table="" var g
  echo '#!/usr/bin/env bash'
  echo '# GENERATED by scripts/monorepo/gen-graph.sh from cargo metadata, package.json file: deps and'
  echo '# scripts/monorepo/components.json. Do not edit: change the manifest and run `mise run graph:gen`.'
  echo '# shellcheck disable=SC2034'
  printf 'COMPONENT_ORDER=(%s)\n' "${order[*]}"
  local -a rules=()
  for c in "${order[@]}"; do
    if [ "$(m --arg c "$c" '.components[$c].pseudo // false')" = true ]; then
      rules+=("$c|$(m --arg c "$c" '.components[$c].globs | join(" ")')")
      continue
    fi
    local -a dirs=() specs=() globs=()
    mapfile -t dirs < <(closure "$c")
    mapfile -t extras < <(m --arg c "$c" '.components[$c].extra_inputs[]?')
    mapfile -t wf < <(m --arg c "$c" '.components[$c].workflow_files[]?')
    specs=("${dirs[@]}")
    for g in "${extras[@]}"; do specs+=("${g%/\*\*}"); done
    var=DEPS_${c//-/_}
    printf '%s=(' "$var"
    for g in "${specs[@]}"; do printf " '%s'" "$g"; done
    echo ' )'
    if has_crate "$c"; then mapfile -t globs < <(m '.rust_root_files[]?'); else globs=(); fi
    globs+=("$c/**")
    for g in "${dirs[@]}"; do globs+=("$g/**"); done
    globs+=("${extras[@]}" "${wf[@]}")
    rules+=("$c|${globs[*]}")
    if [ "$(m --arg c "$c" '.components[$c].image != null')" = true ]; then
      table+=$(m --arg c "$c" '.components[$c] | "\($c) \(.image) \(.context) \(.smoke // "-") \(.timeout)"')$'\n'
    fi
  done
  echo 'RULES=('
  for g in "${rules[@]}"; do echo "  '$g'"; done
  echo ')'
  echo "IMAGE_TABLE='${table%$'\n'}'"
}

mapfile -t dockerfiles < <(dockerfile_components)

if [ "$check" -eq 1 ]; then
  tmp=$(mktemp)
  trap 'rm -f "$tmp"' EXIT
  stale=0
  generate >"$tmp"
  diff -u "$out" "$tmp" || stale=1
  for c in "${dockerfiles[@]}"; do
    render_dockerfile "$c" >"$tmp" || exit 1
    diff -u "$root/$c/Dockerfile" "$tmp" || stale=1
  done
  if [ "$stale" -eq 1 ]; then
    echo "error: generated files are stale: run 'mise run graph:gen' and commit the result" >&2
    exit 1
  fi
  echo "ok: ${out#"$root"/} and the Dockerfile COPY blocks are up to date"
else
  generate >"$out.tmp"
  mv "$out.tmp" "$out"
  for c in "${dockerfiles[@]}"; do
    render_dockerfile "$c" >"$root/$c/Dockerfile.tmp" || { rm -f "$root/$c/Dockerfile.tmp"; exit 1; }
    cat "$root/$c/Dockerfile.tmp" >"$root/$c/Dockerfile" # keeps the file mode
    rm -f "$root/$c/Dockerfile.tmp"
  done
fi
