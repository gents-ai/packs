#!/usr/bin/env bash
# Runs one pack's self-contained suite:
#   1. gents pack build, then the built .pack verified against its digest;
#   2. gents pack test: the install-time check and every plugin's cases;
#   3. every case in <pack>/tests/*.json, each holding one expectation:
#        {"graphs": [...]}   graph ids the pack's graph compiles to
#        {"install": {"documents": [...], "slots": [...], "dependencies": [...]}}
#                            a documents pack installed from its directory
#                            into a fresh home creates exactly these
#                            documents, binds these inference slots and
#                            installs these dependency packs;
#                            a reinstall creates nothing new, and a remove
#                            deletes exactly what the install created
#        {"install": {"assets": [...]}}
#                            an assets pack installed into a fresh home
#                            materializes exactly these files, and a
#                            remove releases them
# Scenarios (experiment.json) need a model endpoint and are not run here.
#
# Usage: scripts/test-pack.sh <pack-dir>    GENTS overrides the gents binary.
set -euo pipefail

[[ $# -eq 1 && -f "$1/manifest.json" ]] || {
  echo "usage: $0 <pack-dir with a manifest.json>" >&2
  exit 2
}
dir="$(cd "$1" && pwd)"
gents="${GENTS:-gents}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

namespace="$(jq -r '.namespace // "gents"' "$dir/manifest.json")"
name="$(jq -r '.name' "$dir/manifest.json")"
kind="$(jq -r '.kind' "$dir/manifest.json")"
pack="$namespace/$name"
failures=0

pass() { printf 'ok    %s: %s\n' "$pack" "$1"; }
fail() {
  printf 'FAIL  %s: %s\n' "$pack" "$1" >&2
  failures=$((failures + 1))
}
# Prints the members of two JSON string arrays that differ, or nothing.
set_diff() {
  jq -rn --argjson want "$1" --argjson got "$2" \
    '(($want - $got) | map("missing " + .)) + (($got - $want) | map("unexpected " + .)) | join(", ")'
}
expect_set() {
  local what="$1" want="$2" got="$3" diff
  diff="$(set_diff "$want" "$got")"
  if [[ -z "$diff" ]]; then pass "$what"; else fail "$what: $diff"; fi
}

[[ "$namespace" == "gents" ]] || fail "namespace is $namespace, not gents"

# Built first: pack check refuses a documents or graph pack whose plugins
# are not compiled yet.
"$gents" pack build "$dir" --out "$work/built.pack" >"$work/build.json"
built="$(jq -r '.digest' "$work/build.json")"
if "$gents" pack verify "$work/built.pack" >"$work/verify.json"; then
  pass "gents pack build and verify ($built)"
else
  fail "gents pack verify rejected the built pack"
fi

if "$gents" pack test "$dir" >"$work/test.json"; then
  pass "gents pack test"
else
  fail "gents pack test failed"
fi
digest="$(jq -r '.digest // empty' "$work/test.json")"
[[ -z "$digest" || "$digest" == "$built" ]] || fail "test digest $digest differs from build digest $built"
while read -r plugin passed failed; do
  if ((failed == 0)); then
    pass "plugin $plugin: $passed cases"
  else
    fail "plugin $plugin: $failed of $((passed + failed)) cases failed"
  fi
done < <(jq -r '.plugins // [] | .[] | "\(.plugin) \(.passed) \(.failures | length)"' "$work/test.json")
jq -r '.plugins // [] | .[].failures[]' "$work/test.json" >&2
graphs="$(jq -c '.graphs // []' "$work/test.json")"

# Initializes a fresh home and prints its path; its init report is <home>.json.
fresh_home() {
  local home="$work/home-$1"
  "$gents" init --home "$home" >"$home.json"
  printf '%s' "$home"
}

install_documents() {
  local case="$1" home args=() profile slot
  home="$(fresh_home "$(basename "$case" .json)")"
  # Bind every slot the pack and its dependencies (sibling gents packs)
  # declare to the fresh home's own profile.
  profile="$(jq -r '.inference_profile_id' "$home.json")"
  local manifests=("$dir/manifest.json") dep
  while read -r dep; do
    manifests+=("$(dirname "$dir")/$dep/manifest.json")
  done < <(jq -r '.dependencies // [] | .[]' "$dir/manifest.json")
  while read -r slot; do
    args+=(--inference-slot "$slot=$profile")
  done < <(jq -rs '[.[] | .inference_slots // [] | .[].name] | unique | .[]' "${manifests[@]}")

  "$gents" pack install "$dir" --home "$home" "${args[@]}" >"$work/install.json"
  expect_set "$(basename "$case"): inference slots" \
    "$(jq -c '.install.slots // []' "$case")" \
    "$(jq -c '.inference.bindings | keys' "$work/install.json")"
  expect_set "$(basename "$case"): installs dependencies" \
    "$(jq -c '.install.dependencies // []' "$case")" \
    "$(jq -c '[.dependencies[] | if type == "object" then .name else . end]' "$work/install.json")"
  local want
  want="$(jq -c '.install.documents' "$case")"
  expect_set "$(basename "$case"): install creates" "$want" "$(jq -c '.apply.created' "$work/install.json")"

  "$gents" pack install "$dir" --home "$home" "${args[@]}" >"$work/reinstall.json"
  if jq -e '.apply.created == [] and .apply.removed == []' "$work/reinstall.json" >/dev/null; then
    expect_set "$(basename "$case"): reinstall keeps" "$want" \
      "$(jq -c '.apply | .replaced + .kept + .adopted' "$work/reinstall.json")"
  else
    fail "$(basename "$case"): reinstall created or removed documents"
  fi

  "$gents" pack remove "$pack" --home "$home" >"$work/remove.json"
  expect_set "$(basename "$case"): remove deletes" "$want" "$(jq -c '.removed.removed' "$work/remove.json")"
}

install_assets() {
  local case="$1" home root
  home="$(fresh_home "$(basename "$case" .json)")"
  "$gents" pack install "$dir" --home "$home" >"$work/install.json"
  root="$(jq -r '.installed_assets' "$work/install.json")"
  expect_set "$(basename "$case"): install materializes" \
    "$(jq -c '.install.assets' "$case")" \
    "$(cd "$root" && find . -type f ! -name '.*' | sed 's|^\./||' | jq -Rsc 'split("\n") | map(select(. != ""))')"

  if "$gents" pack remove "$pack" --home "$home" >"$work/remove.json" && [[ ! -e "$root" ]]; then
    pass "$(basename "$case"): remove releases the assets"
  else
    fail "$(basename "$case"): gents pack remove did not release $root"
  fi
}

shopt -s nullglob
cases=("$dir"/tests/*.json)
[[ ${#cases[@]} -gt 0 ]] || fail "has no tests/*.json cases"
for case in "${cases[@]}"; do
  if jq -e 'has("graphs")' "$case" >/dev/null; then
    expect_set "$(basename "$case"): graphs" "$(jq -c '.graphs' "$case")" "$graphs"
  elif jq -e '.install | has("documents")' "$case" >/dev/null; then
    [[ "$kind" == "documents" ]] || fail "$(basename "$case"): documents case in a $kind pack"
    install_documents "$case"
  elif jq -e '.install | has("assets")' "$case" >/dev/null; then
    [[ "$kind" == "assets" ]] || fail "$(basename "$case"): assets case in a $kind pack"
    install_assets "$case"
  else
    fail "$(basename "$case"): not a graphs or install case"
  fi
done

if ((failures > 0)); then
  echo "$pack: $failures failed" >&2
  exit 1
fi
echo "$pack: all passed"
