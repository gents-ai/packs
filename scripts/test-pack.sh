#!/usr/bin/env bash
# Runs one pack's self-contained suite:
#   1. gents pack test: the install-time check, a build, every plugin case;
#   2. the built .pack verified against its digest;
#   3. every case in <pack>/tests/*.json, each holding one expectation:
#        {"graphs": [...]}   graph ids the pack's graph compiles to
#        {"install": {"documents": [...], "slots": [...]}}
#                            a documents pack installed from its directory
#                            into a fresh home creates exactly these
#                            documents and declares these inference slots;
#                            a reinstall creates nothing new, and a remove
#                            deletes exactly what the install created
#        {"install": {"assets": [...]}}
#                            an assets pack installed into a fresh home
#                            materializes exactly these files
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

"$gents" pack test "$dir" >"$work/test.json"
digest="$(jq -r '.digest' "$work/test.json")"
pass "gents pack test ($digest)"
graphs="$(jq -c '.graphs' "$work/test.json")"

"$gents" pack build "$dir" --out "$work/built.pack" >"$work/build.json"
built="$(jq -r '.digest' "$work/build.json")"
[[ "$built" == "$digest" ]] || fail "build digest $built differs from test digest $digest"
if "$gents" pack verify "$work/built.pack" >"$work/verify.json"; then
  pass "gents pack verify"
else
  fail "gents pack verify rejected the built pack"
fi

fresh_home() {
  local home="$work/home-$1"
  "$gents" init --home "$home" >/dev/null
  printf '%s' "$home"
}

install_documents() {
  local case="$1" home args=() profile slot
  home="$(fresh_home "$(basename "$case" .json)")"
  "$gents" pack install "$dir" --home "$home" --preview >"$work/preview.json"
  expect_set "$(basename "$case"): inference slots" \
    "$(jq -c '.install.slots // []' "$case")" \
    "$(jq -c '[.inference.slots[].name]' "$work/preview.json")"
  # Bind every declared slot to the fresh home's one usable profile.
  profile="$(jq -r '[.inference.profiles[] | select(.usable)][0].profile_id' "$work/preview.json")"
  while read -r slot; do
    args+=(--inference-slot "$slot=$profile")
  done < <(jq -r '.inference.slots[].name' "$work/preview.json")

  "$gents" pack install "$dir" --home "$home" "${args[@]}" >"$work/install.json"
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
    "$(cd "$root" && find . -type f | sed 's|^\./||' | jq -Rsc 'split("\n") | map(select(. != ""))')"
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
