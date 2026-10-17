#!/usr/bin/env bash
# Tests for scripts/monorepo/gen-graph.sh and its consumers (affected.sh, next-version.sh,
# image-components.sh), run on a scratch copy of the tree so nothing in the checkout changes.
#
#   1. a fake Cargo path dependency is picked up by affected, release paths/version and the image table;
#   2. the same for a fake npm `file:` dependency;
#   3. the freshness check passes on the committed tree and fails when the generated file or the
#      manifest is stale;
#   4. the known disagreements stay fixed (ui inputs, sandbox-container -> ui/package-lock.json);
#   5. a manifest glob that matches no file makes the generator fail.
set -euo pipefail

cd "$(dirname "$0")/../.."
src=$PWD
fail=0
tmps=()
trap 'rm -rf "${tmps[@]}"' EXIT

ok() { # ok <description> <expected> <got>
  if [ "$2" != "$3" ]; then echo "FAIL: $1: expected [$2], got [$3]" >&2; fail=1; fi
}
contains() { # contains <description> <needle> <haystack>
  case " $3 " in *" $2 "*) ;; *) echo "FAIL: $1: [$3] lacks [$2]" >&2; fail=1 ;; esac
}

scratch() { # copy of the working tree (tracked + untracked, minus ignored) into a new temp dir
  local d
  d=$(mktemp -d)
  tmps+=("$d")
  (cd "$src" && git ls-files -z -co --exclude-standard | tar --null -T - --ignore-failed-read -cf - 2>/dev/null) | tar -x -C "$d"
  echo "$d"
}

affected() { # <tree> <paths...>
  local tree=$1
  shift
  printf '%s\n' "$@" | bash "$tree/scripts/monorepo/affected.sh" --files-from - | paste -sd' ' -
}

paths_of() { (cd "$1" && source .github/scripts/next-version.sh && release_paths "$2"); }

# next_version <tree> <dir> <path-to-touch>: a throwaway repo tagged <dir>/v0.1.0 at the current
# state, then a `feat:` commit touching only <path-to-touch>.
bump_after() {
  local tree=$1 dir=$2 touch=$3
  (
    cd "$tree"
    git init -q . && git config user.email t@t && git config user.name t
    git add -A && git commit -qm "chore: base"
    git tag "$dir/v0.1.0"
    echo change >>"$touch"
    git add -A && git commit -qm "feat: change"
    source .github/scripts/next-version.sh
    next_version "$dir" HEAD
  )
}

# --- 1. fake Cargo path dependency ------------------------------------------------------------
t=$(scratch)
mkdir -p "$t/fakecrate/src"
printf '[package]\nname = "fakecrate"\nversion = "0.0.0"\nedition = "2024"\n' >"$t/fakecrate/Cargo.toml"
echo '// fake' >"$t/fakecrate/src/lib.rs"
sed -i 's|^    "updater",|    "updater",\n    "fakecrate",|' "$t/Cargo.toml"
sed -i 's|^\[dependencies\]|&\nfakecrate = { path = "../fakecrate" }|' "$t/updater/Cargo.toml"
jq '.components.fakecrate = {image: "enzarb-fake", context: ".", smoke: null, timeout: 10}' "$t/scripts/monorepo/components.json" >"$t/m.json"
mv "$t/m.json" "$t/scripts/monorepo/components.json"
bash "$t/scripts/monorepo/gen-graph.sh" --root "$t"
ok "cargo: affected" "updater fakecrate" "$(affected "$t" fakecrate/src/lib.rs)"
contains "cargo: release paths" fakecrate "$(paths_of "$t" updater)"
ok "cargo: image table" "enzarb-updater" "$(bash "$t/.github/scripts/image-components.sh" updater | jq -r .image)"
ok "cargo: image table (new component)" "enzarb-fake" "$(bash "$t/.github/scripts/image-components.sh" fakecrate | jq -r .image)"
ok "cargo: release version" "0.2.0" "$(bump_after "$t" updater fakecrate/src/lib.rs)"

# --- 2. fake npm file: dependency -------------------------------------------------------------
t=$(scratch)
mkdir -p "$t/fakepkg"
echo '{"name": "fakepkg", "version": "0.0.0"}' >"$t/fakepkg/package.json"
echo '// fake' >"$t/fakepkg/index.js"
jq '.dependencies.fake = "file:../fakepkg"' "$t/ui/package.json" >"$t/p.json"
mv "$t/p.json" "$t/ui/package.json"
bash "$t/scripts/monorepo/gen-graph.sh" --root "$t"
ok "npm: affected" "ui" "$(affected "$t" fakepkg/index.js)"
contains "npm: release paths" fakepkg "$(paths_of "$t" ui)"
ok "npm: image table" "enzarb-ui" "$(bash "$t/.github/scripts/image-components.sh" ui | jq -r .image)"
ok "npm: release version" "0.2.0" "$(bump_after "$t" ui fakepkg/index.js)"

# --- 3. freshness -----------------------------------------------------------------------------
t=$(scratch)
if ! bash "$t/scripts/monorepo/gen-graph.sh" --check --root "$t" >/dev/null 2>&1; then
  echo "FAIL: freshness: the committed graph.generated.sh is stale" >&2
  fail=1
fi
echo '# stale' >>"$t/scripts/monorepo/graph.generated.sh"
if bash "$t/scripts/monorepo/gen-graph.sh" --check --root "$t" >/dev/null 2>&1; then
  echo "FAIL: freshness: edited generated file not detected" >&2
  fail=1
fi
bash "$t/scripts/monorepo/gen-graph.sh" --root "$t"
jq '.components.updater.timeout = 31' "$t/scripts/monorepo/components.json" >"$t/m.json"
mv "$t/m.json" "$t/scripts/monorepo/components.json"
if bash "$t/scripts/monorepo/gen-graph.sh" --check --root "$t" >/dev/null 2>&1; then
  echo "FAIL: freshness: edited manifest not detected" >&2
  fail=1
fi

# --- 4. known disagreements -------------------------------------------------------------------
for p in workflows/crates/flow/tests/fixtures/app/x docs/workflows/examples/x workflows/crates/engine/tests/stdlib/prompts/x.md; do
  contains "ui inputs: $p" ui "$(affected "$src" "$p")"
done
ui_paths=$(paths_of "$src" ui)
for d in workflows/crates/flow/tests/fixtures docs/workflows/examples workflows/crates/engine/tests/stdlib/prompts; do
  contains "ui release paths" "$d" "$ui_paths"
done
contains "sandbox-container affected by the Playwright pin" sandbox-container "$(affected "$src" ui/package-lock.json)"
contains "sandbox-container release paths" ui/package-lock.json "$(paths_of "$src" sandbox-container)"

# --- 5. manifest hygiene ----------------------------------------------------------------------
t=$(scratch)
jq '.components.ui.extra_inputs += ["no/such/file/**"]' "$t/scripts/monorepo/components.json" >"$t/m.json"
mv "$t/m.json" "$t/scripts/monorepo/components.json"
if bash "$t/scripts/monorepo/gen-graph.sh" --root "$t" >/dev/null 2>&1; then
  echo "FAIL: hygiene: a manifest glob matching nothing was accepted" >&2
  fail=1
fi

[ "$fail" -eq 0 ] || exit 1
echo "ok: gen-graph.sh picks up cargo and npm path deps and is checked for freshness"
