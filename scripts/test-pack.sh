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
#                            a reinstall creates nothing new, `config apply`
#                            accepts the installed configuration, and a
#                            remove deletes exactly what the install created
#        {"install": {"assets": [...]}}
#                            an assets pack installed into a fresh home
#                            materializes exactly these files, and a
#                            remove releases them
#        {"install": {"graph": "<graph_id>", "slots": [...], "documents": [...]}}
#                            a graph pack installed from its directory
#                            activates this graph and binds these inference
#                            slots; a reinstall keeps the revision digest, and
#                            a remove deletes exactly these documents
#        {"defs": "<jq defs>", "jq": [{"name": "...", "expr": "<jq boolean>"}]}
#                            each expression (after the optional defs) is
#                            true over one document built
#                            once per pack: {"manifest", "config", "scenario",
#                            "assets": {path: text} for every UTF-8 asset}
#        {"eval_case": {"asset": "<path>", "after": "<heading line>"}}
#                            the first json fence after that heading in the
#                            asset is an eval case gents accepts
#        {"cli_flags": {"command": [...], "flags": [...]}}
#                            `gents <command> --help` documents every flag
#        {"runtime": {"repository": {"files": {...}, "dirs": [...]},
#                     "seed": {"collection": ..., "fields": {...}},
#                     "taken": {"collection": ..., "filter": {...}},
#                     "expect": [{"collection": ..., "filter": {...},
#                                 "fields": {...}}]}}
#                            the pack installed into a fresh home and served
#                            from a throwaway git repository (its commit is
#                            ${BASE_SHA} in seed fields; ${ATTEMPT} keeps a
#                            re-created seed unique) reaches every expected
#                            document state after the seed is created, with
#                            no model involved
#        {"install": {"plugins": [...]}}
#                            a plugins pack installed into a fresh home
#                            registers exactly these plugins, a reinstall
#                            keeps the same set, and a remove releases them;
#                            next to "documents" it also checks the plugins
#                            a documents or graph pack ships
#        runtime "repository" may also hold "copy": {"<repo path>": "<file
#                            path inside the pack>"} for binary files, which
#                            are copied into the repository before its commit,
#                            and "access": "read_write" to let plugins write
#                            to it (an allowed folder is read-only otherwise)
# Every documents or graph pack also gets the built-in checks: it declares
# inference slots and authors no inference documents, no task sets a goal
# token budget, and each dependency is a sibling pack whose manifest matches
# and is pre-stored in the install's home.
# Every variable the pack's config requires (`${VAR}`, or `${VAR:-}` so pack
# check accepts it; not the GENTS_PACK_* values gents supplies) names an
# operator path: the suite sets it to a scratch directory, and a runtime case
# to its repository.
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
servers=()
cleanup() {
  local pid
  for pid in ${servers[@]+"${servers[@]}"}; do kill "$pid" 2>/dev/null || true; done
  rm -rf "$work"
}
trap cleanup EXIT

namespace="$(jq -r '.namespace // "gents"' "$dir/manifest.json")"
name="$(jq -r '.name' "$dir/manifest.json")"
kind="$(jq -r '.kind' "$dir/manifest.json")"
pack="$namespace/$name"
failures=0

required_vars=()
while read -r var; do required_vars+=("$var"); done < <(
  jq -r '.. | strings' "$dir/$(jq -r '.config // "pack_config.json"' "$dir/manifest.json")" 2>/dev/null \
    | grep -oE '\$\{[A-Za-z_][A-Za-z0-9_]*(:-)?\}' | sed -E 's/^..//; s/(:-)?}$//' | grep -v '^GENTS_PACK_' | sort -u || true)
export_required() {
  local var
  for var in ${required_vars[@]+"${required_vars[@]}"}; do export "$var=$1"; done
}
export_required "$work"

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

# A Rust plugin's own `cargo test` (unit tests ported alongside its source,
# e.g. paging or bounds checks the golden/case files above cannot express)
# runs here, once per plugin, under the repo's pinned rust-toolchain.
# Skipped, not failed, when cargo is not on PATH.
if command -v cargo >/dev/null 2>&1; then
  while read -r plugin source; do
    [[ -n "$source" && -f "$dir/$source/Cargo.toml" ]] || continue
    if cargo test --manifest-path "$dir/$source/Cargo.toml" --quiet \
      >"$work/cargo-test-$plugin.log" 2>&1; then
      pass "plugin $plugin: cargo test"
    else
      fail "plugin $plugin: cargo test failed"
      cat "$work/cargo-test-$plugin.log" >&2
    fi
  done < <(jq -r '.plugins // [] | .[] | select(.language == "rust") | "\(.name) \(.source // "")"' "$dir/manifest.json")
else
  echo "note: cargo not on PATH; skipping $pack plugin unit tests" >&2
fi

# Prints the path of the pack's document for jq cases and built-in checks,
# building it on first use: one `pack show --config` and one pass over the
# declared assets, whatever the number of cases.
show_document() {
  local doc="$work/show-document.json" names="$work/asset-names.txt" path
  [[ -f "$doc" ]] && { printf '%s' "$doc"; return; }
  "$gents" pack show "$dir" --config --home "$work/show" >"$work/show-config.json"
  : >"$names"
  while read -r path; do
    [[ -f "$dir/$path" ]] && iconv -f UTF-8 -t UTF-8 "$dir/$path" >/dev/null 2>&1 && printf '%s\n' "$path" >>"$names"
  done < <(jq -r '.assets // [] | .[]' "$dir/manifest.json")
  local files=()
  while read -r path; do files+=("$path"); done <"$names"
  (cd "$dir" && jq -Rn --rawfile names "$names" \
    '($names | split("\n") | map(select(. != "") | {key: ., value: ""}) | from_entries) as $empty
      | reduce inputs as $line ($empty; .[input_filename] += $line + "\n")' ${files[@]+"${files[@]}"} </dev/null) >"$work/assets.json"
  jq -n --slurpfile manifest "$dir/manifest.json" --slurpfile shown "$work/show-config.json" \
    --slurpfile assets "$work/assets.json" \
    '{manifest: $manifest[0], config: $shown[0].config, scenario: $shown[0].scenario, assets: $assets[0]}' >"$doc"
  printf '%s' "$doc"
}

# Pre-stores every dependency pack (a sibling directory) in <home>, so an
# install resolves them offline from the home's pack store.
store_dependencies() {
  local home="$1" dep
  while read -r dep; do
    "$gents" pack build "$(dirname "$dir")/$dep" --out "$work/dep-$dep.pack" >/dev/null \
      && "$gents" pack fetch "$work/dep-$dep.pack" --store --home "$home" >/dev/null \
      || fail "could not pre-store dependency $dep"
  done < <(jq -r '.dependencies // [] | .[] | split("/") | last' "$dir/manifest.json")
}

# Checks every pack of a configuration kind must pass; one jq pass.
builtin_checks() {
  local doc dep
  [[ "$kind" == "documents" || "$kind" == "graph" ]] || return 0
  doc="$(show_document)"
  jq -e '(.manifest.inference_slots // []) | length > 0' "$doc" >/dev/null \
    && pass "declares inference slots" || fail "declares no inference slots"
  jq -e '[.config.inference_backends, .config.inference_profiles, .config.inference_sampling,
          .config.inference_execution, .config.inference_retry_policies] | all((. // []) | length == 0)' "$doc" >/dev/null \
    && pass "authors no inference documents" || fail "authors inference backends, profiles, sampling, execution or retry policies; slots bind them at install"
  jq -e '[.config.tasks[]? | select(.goal_token_budget != null)] | length == 0' "$doc" >/dev/null \
    && pass "no task sets a goal token budget" || fail "a task sets goal_token_budget; goal budgets are opt-in"
  while read -r dep; do
    local ns=gents name="${dep##*/}"
    [[ "$dep" == */* ]] && ns="${dep%%/*}"
    if jq -e --arg ns "$ns" --arg name "$name" '.name == $name and (.namespace // "gents") == $ns' \
      "$(dirname "$dir")/$name/manifest.json" >/dev/null 2>&1; then
      pass "dependency $dep is a sibling pack"
    else
      fail "dependency $dep has no sibling pack directory with that coordinate"
    fi
  done < <(jq -r '.dependencies // [] | .[]' "$dir/manifest.json")
}

# Initializes a fresh home and prints its path; its init report is <home>.json.
# The directory init runs in ($2, default the current one) becomes the home's
# operator ceiling.
fresh_home() {
  local home="$work/home-$1"
  (cd "${2:-.}" && "$gents" init --home "$home") >"$home.json"
  printf '%s' "$home"
}

# Prints one --inference-slot argument per line, binding every required slot the pack
# and its dependencies (sibling gents packs) declare to <home>'s own profile.
slot_args() {
  local home="$1" profile dep slot manifests=("$dir/manifest.json")
  profile="$(jq -r '.inference_profile_id' "$home.json")"
  while read -r dep; do
    manifests+=("$(dirname "$dir")/$dep/manifest.json")
  done < <(jq -r '.dependencies // [] | .[]' "$dir/manifest.json")
  while read -r slot; do
    printf -- '--inference-slot\n%s=%s\n' "$slot" "$profile"
  done < <(jq -rs '[.[] | .inference_slots // [] | .[] | select(.optional != true) | .name] | unique | .[]' "${manifests[@]}")
}

install_documents() {
  local case="$1" home args=() arg
  home="$(fresh_home "$(basename "$case" .json)")"
  while read -r arg; do args+=("$arg"); done < <(slot_args "$home")
  store_dependencies "$home"

  "$gents" pack install "$dir" --home "$home" --grant-authority ${args[@]+"${args[@]}"} >"$work/install.json"
  expect_set "$(basename "$case"): inference slots" \
    "$(jq -c '.install.slots // []' "$case")" \
    "$(jq -c '.inference.bindings | keys' "$work/install.json")"
  expect_set "$(basename "$case"): installs dependencies" \
    "$(jq -c '.install.dependencies // []' "$case")" \
    "$(jq -c '[.dependencies[] | if type == "object" then .name else . end]' "$work/install.json")"
  local want
  want="$(jq -c '.install.documents' "$case")"
  expect_set "$(basename "$case"): install creates" "$want" "$(jq -c '.apply.created' "$work/install.json")"
  if jq -e '.install | has("plugins")' "$case" >/dev/null; then
    expect_set "$(basename "$case"): install registers plugins" \
      "$(jq -c '.install.plugins' "$case")" "$(jq -c '[.apply.plugins[].name]' "$work/install.json")"
  fi

  "$gents" pack install "$dir" --home "$home" --grant-authority ${args[@]+"${args[@]}"} >"$work/reinstall.json"
  if jq -e '.apply.created == [] and .apply.removed == []' "$work/reinstall.json" >/dev/null; then
    expect_set "$(basename "$case"): reinstall keeps" "$want" \
      "$(jq -c '.apply | .replaced + .kept + .adopted' "$work/reinstall.json")"
  else
    fail "$(basename "$case"): reinstall created or removed documents"
  fi
  config_apply_accepts "$(basename "$case")" "$home"

  "$gents" pack remove "$pack" --home "$home" >"$work/remove.json"
  expect_set "$(basename "$case"): remove deletes" "$want" "$(jq -c '.removed.removed' "$work/remove.json")"
}

# `pack install` does not run `config apply`'s live checks (event-source
# filters and the `doc.*` fields their task templates read, against the
# installed schema), which scenarios and operators hit; re-apply the
# installed configuration through them.
config_apply_accepts() {
  local label="$1" home="$2" root="$work/apply-$1"
  if "$gents" config export --home "$home" --root "$root" --force >/dev/null 2>"$work/apply-$label.err" \
    && NO_COLOR=1 "$gents" config apply --home "$home" --root "$root" >"$work/apply-$label.json" 2>>"$work/apply-$label.err"; then
    pass "$label: config apply accepts the installed configuration"
  else
    fail "$label: config apply refused the installed configuration: $(grep -E 'ERROR|Error' "$work/apply-$label.err" | tail -3 | tr '\n' ' ')"
  fi
}

# External service declarations use the canonical configuration owner; the
# fixture endpoint is never contacted.
register_services() {
  local home="$1" root="$work/service-config"
  jq -e '(.external_dependencies // []) | length > 0' "$dir/manifest.json" >/dev/null || return 0
  "$gents" config export --home "$home" --root "$root" --force >/dev/null || return 1
  jq --slurpfile manifest "$dir/manifest.json" '
    .tool_service_registries = ((.tool_service_registries // []) +
      [$manifest[0].external_dependencies[] | {
        service_id: .service_id, display_name: .service_id,
        description: "test registration", hostname: "localhost", lan_ip: "127.0.0.1",
        mcp_port: 9, mcp_path: "/mcp", send_agent_did: false, enabled: true
      }])
  ' "$root/pack_config.json" >"$root/config-next.json"
  mv "$root/config-next.json" "$root/pack_config.json"
  "$gents" config apply --home "$home" --root "$root" >"$work/register.json" 2>"$work/register.err" \
    || { fail "install: could not register services: $(tail -1 "$work/register.err")"; return 1; }
}

install_graph() {
  local case="$1" home args=() arg want
  home="$(fresh_home "$(basename "$case" .json)")"
  while read -r arg; do args+=("$arg"); done < <(slot_args "$home")
  store_dependencies "$home"
  register_services "$home" || return 0

  "$gents" pack install "$dir" --home "$home" --grant-authority ${args[@]+"${args[@]}"} >"$work/install.json"
  expect_set "$(basename "$case"): inference slots" \
    "$(jq -c '.install.slots // []' "$case")" \
    "$(jq -c '.bindings.inference_slots | keys' "$work/install.json")"
  if [[ "$(jq -r '.install.graph_id' "$work/install.json")" == "$(jq -r '.install.graph' "$case")" ]]; then
    pass "$(basename "$case"): installs graph $(jq -r '.install.graph' "$case")"
  else
    fail "$(basename "$case"): installed graph $(jq -r '.install.graph_id' "$work/install.json"), want $(jq -r '.install.graph' "$case")"
  fi

  "$gents" pack install "$dir" --home "$home" --grant-authority ${args[@]+"${args[@]}"} >"$work/reinstall.json"
  if [[ "$(jq -r '.install.revision_digest' "$work/install.json")" == "$(jq -r '.install.revision_digest' "$work/reinstall.json")" ]]; then
    pass "$(basename "$case"): reinstall keeps the revision digest"
  else
    fail "$(basename "$case"): reinstall changed the revision digest"
  fi

  want="$(jq -c '.install.documents' "$case")"
  "$gents" pack remove "$pack" --home "$home" >"$work/remove.json"
  # Graph trigger, event source and revision ids derive from the graph's
  # digest, so they are compared with the digest elided.
  expect_set "$(basename "$case"): remove deletes" "$want" "$(jq -c '.removed.removed
    | map(gsub("graph-trigger-[0-9a-f]{64}-[0-9a-f]{16}"; "graph-trigger-*") | gsub("^GraphRevision/.*"; "GraphRevision/*")) | unique' "$work/remove.json")"
}

install_assets() {
  local case="$1" home root
  home="$(fresh_home "$(basename "$case" .json)")"
  "$gents" pack install "$dir" --home "$home" --grant-authority >"$work/install.json"
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

# Polls a GraphQL endpoint until the query's rows include one whose fields
# match; prints the last response. Waits with a deadline and backoff, so a
# fast machine finishes at once and a slow one still passes.
await_rows() {
  local url="$1" query="$2" want="$3" deadline=$((SECONDS + ${AWAIT_SECS:-120})) delay=0.1 response
  while :; do
    response="$(curl -fsS "$url" -H 'content-type: application/json' \
      -d "$(jq -cn --arg q "$query" '{query: $q}')" 2>/dev/null || true)"
    if jq -e --argjson want "$want" \
      '[.data[]?[]? | select(. as $row | $want | to_entries | all(.value == $row[.key]))] | length > 0' \
      <<<"$response" >/dev/null 2>&1; then
      printf '%s' "$response"
      return 0
    fi
    ((SECONDS < deadline)) || { printf '%s' "$response"; return 1; }
    sleep "$delay"
    delay="$(awk -v d="$delay" 'BEGIN { d *= 2; print (d > 2 ? 2 : d) }')"
  done
}

# The GraphQL read for one runtime expectation: its collection, filtered by
# `filter`, selecting the fields it expects.
expectation_query() {
  jq -r '"{ \(.collection)(filter: {\(.filter // {} | to_entries
      | map("\(.key): {_eq: \(.value | tostring | tojson)}") | join(", "))}) {
      \(.fields // {} | keys | if length == 0 then ["_docID"] else . end | join(" ")) } }"' <<<"$1"
}

runtime_case() {
  local case="$1" name repo home port url log pid args=() arg base path
  name="$(basename "$case" .json)"
  # Runtime cases seed through the operator document command; fail at once on a gents without it.
  "$gents" document create --help >/dev/null 2>&1 \
    || { fail "$name: this gents has no 'document create' command; set GENTS to a gents build that includes it"; return; }
  repo="$work/repo-$name"
  mkdir -p "$repo"
  while read -r path; do mkdir -p "$repo/$path"; done < <(jq -r '.runtime.repository.dirs // [] | .[]' "$case")
  while read -r path; do
    mkdir -p "$(dirname "$repo/$path")"
    jq -j --arg p "$path" '.runtime.repository.files[$p]' "$case" >"$repo/$path"
  done < <(jq -r '.runtime.repository.files // {} | keys[]' "$case")
  while read -r path; do
    mkdir -p "$(dirname "$repo/$path")"
    cp "$dir/$(jq -r --arg p "$path" '.runtime.repository.copy[$p]' "$case")" "$repo/$path"
  done < <(jq -r '.runtime.repository.copy // {} | keys[]' "$case")
  git -C "$repo" init -q
  git -C "$repo" add -A
  git -C "$repo" -c user.name=packs -c user.email=packs@localhost commit -qm fixture --allow-empty
  base="$(git -C "$repo" rev-parse HEAD)"

  # The workspace callback may only create workspaces inside the operator
  # ceiling, so the home is initialized from the repository.
  home="$(fresh_home "$name" "$repo")"
  export_required "$repo"
  local access
  access="$(jq -r '.runtime.repository.access // empty' "$case")"
  "$gents" plugin dirs add "$repo" --home "$home" ${access:+--access "$access"} >"$work/$name-allowed.json"
  while read -r arg; do args+=("$arg"); done < <(slot_args "$home")
  store_dependencies "$home"
  "$gents" pack install "$dir" --home "$home" --grant-authority ${args[@]+"${args[@]}"} >"$work/$name-install.json"

  port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
  url="http://127.0.0.1:$port/api/v0/graphql"
  log="$work/$name-server.log"
  # A repository placement's host path "." is the server's working directory.
  (cd "$repo" && NO_COLOR=1 exec "$gents" server --home "$home" --http-port "$port" --p2p-transport none --no-codex-shim) >"$log" 2>&1 &
  pid=$!
  servers+=("$pid")

  local collection fields attempt=0 deadline=$((SECONDS + 240)) first_query first_want seeded=""
  collection="$(jq -r '.runtime.seed.collection' "$case")"
  first_query="$(expectation_query "$(jq -c '.runtime.taken // .runtime.expect[0]' "$case")")"
  first_want="$(jq -c '(.runtime.taken // .runtime.expect[0]).fields // {}' "$case")"
  # The callback engine logs nothing when it picks up its bindings, and a
  # document created before that is history it never processes. So a fresh
  # seed (${ATTEMPT} makes it unique) is created until `taken` (any matching
  # row, default the first expectation) shows the engine took one.
  while ((SECONDS < deadline)); do
    kill -0 "$pid" 2>/dev/null || { fail "$name: gents server exited: $(tail -3 "$log" | tr '\n' ' ')"; return; }
    attempt=$((attempt + 1))
    fields="$(jq -c --arg base "$base" --arg attempt "$attempt" --arg repo "$repo" '.runtime.seed.fields
      | map_values(if type == "string"
          then gsub("\\$\\{BASE_SHA\\}"; $base) | gsub("\\$\\{ATTEMPT\\}"; $attempt) | gsub("\\$\\{REPOSITORY\\}"; $repo)
          else . end)' "$case")"
    # A served home admits writes only from its own principal, so the seed is
    # created by the operator command. Refused until the runtime has registered
    # the collection; retried below.
    "$gents" document create "$collection" --home "$home" --graphql "$url" --json "$fields" \
      >/dev/null 2>"$work/$name-seed.err" && seeded=yes
    # An authorization refusal or a bad field can never succeed on retry.
    if [[ -z "$seeded" ]] && grep -qiE 'not authorized|permission denied|has no field|--json' "$work/$name-seed.err"; then
      fail "$name: could not create the seed $collection: $(tail -1 "$work/$name-seed.err")"
      return
    fi
    if [[ -n "$seeded" ]] && AWAIT_SECS=10 await_rows "$url" "$first_query" "$first_want" >/dev/null; then
      break
    fi
    [[ -n "$seeded" ]] || sleep 1
  done
  [[ -n "$seeded" ]] || { fail "$name: could not create the seed $collection: $(tail -1 "$work/$name-seed.err" 2>/dev/null)"; return; }

  local expect query want response
  while read -r expect; do
    query="$(expectation_query "$expect")"
    want="$(jq -c '.fields' <<<"$expect")"
    if response="$(await_rows "$url" "$query" "$want")"; then
      pass "$name: $(jq -r '.collection' <<<"$expect") reaches $want"
    else
      fail "$name: $(jq -r '.collection' <<<"$expect") never reached $want; last seen $response"
      grep -E ' (WARN|ERROR) ' "$log" | tail -5 >&2 || true
    fi
  done < <(jq -c '.runtime.expect[]' "$case")
  kill "$pid" 2>/dev/null || true
  export_required "$work"
}

install_plugins() {
  local case="$1" home want name
  name="$(basename "$case")"
  home="$(fresh_home "$(basename "$case" .json)")"
  want="$(jq -c '.install.plugins' "$case")"
  "$gents" pack install "$dir" --home "$home" --grant-authority >"$work/install.json"
  expect_set "$name: install registers" "$want" "$(jq -c '[.installed_plugins[].name]' "$work/install.json")"
  "$gents" plugin list --home "$home" >"$work/plugins.json"
  expect_set "$name: plugin list after install" "$want" "$(jq -c '[.plugins[].name]' "$work/plugins.json")"

  "$gents" pack install "$dir" --home "$home" --grant-authority >"$work/reinstall.json"
  expect_set "$name: reinstall keeps" "$want" "$(jq -c '[.installed_plugins[].name]' "$work/reinstall.json")"

  "$gents" pack remove "$pack" --home "$home" >"$work/remove.json"
  expect_set "$name: remove releases" "$want" "$(jq -c '[.removed.plugins[].name]' "$work/remove.json")"
  "$gents" plugin list --home "$home" >"$work/plugins.json"
  expect_set "$name: plugin list after remove" "[]" "$(jq -c '[.plugins[].name]' "$work/plugins.json")"
}

run_jq_case() {
  local case="$1" doc name expr defs row
  doc="$(show_document)"
  defs="$(jq -r '.defs // ""' "$case")"
  while read -r row; do
    name="$(jq -r '.name' <<<"$row")"
    expr="$(jq -r '.expr' <<<"$row")"
    if jq -e "$defs $expr" "$doc" >/dev/null 2>"$work/jq.err"; then
      pass "$(basename "$case"): $name"
    else
      fail "$(basename "$case"): $name$(head -1 "$work/jq.err" | sed 's/^/ (/;s/$/)/')"
    fi
  done < <(jq -c '.jq[]' "$case")
}

# The first json fence after the heading line is an eval case gents accepts.
eval_case() {
  local case="$1" asset after example
  asset="$(jq -r '.eval_case.asset' "$case")"
  after="$(jq -r '.eval_case.after' "$case")"
  example="$(awk -v after="$after" '
    !found { if ($0 == after) found = 1; next }
    !fenced && /^```json[ \t]*$/ { fenced = 1; next }
    fenced && /^```/ { exit }
    fenced { print }' "$dir/$asset")"
  if [[ -z "$example" ]]; then
    fail "$(basename "$case"): no json block after '$after' in $asset"
  elif printf '%s' "$example" | "$gents" eval checks --validate-case - >"$work/eval-case.json" 2>&1; then
    pass "$(basename "$case"): the example in $asset names only registered checks with valid params"
  else
    fail "$(basename "$case"): $(tr '\n' ' ' <"$work/eval-case.json" | cut -c1-300)"
  fi
}

# `gents <command> --help` documents every flag the pack tells a model to use.
cli_flags() {
  local case="$1" help flag
  local cmd=()
  while read -r flag; do cmd+=("$flag"); done < <(jq -r '.cli_flags.command[]' "$case")
  help="$("$gents" "${cmd[@]}" --help 2>&1 || true)"
  while read -r flag; do
    if grep -qF -- "$flag" <<<"$help"; then
      pass "$(basename "$case"): gents ${cmd[*]} has $flag"
    else
      fail "$(basename "$case"): gents ${cmd[*]} has no $flag"
    fi
  done < <(jq -r '.cli_flags.flags[]' "$case")
}

builtin_checks

shopt -s nullglob
cases=("$dir"/tests/*.json)
[[ ${#cases[@]} -gt 0 ]] || fail "has no tests/*.json cases"
for case in "${cases[@]}"; do
  if jq -e 'has("graphs")' "$case" >/dev/null; then
    expect_set "$(basename "$case"): graphs" "$(jq -c '.graphs' "$case")" "$graphs"
  elif jq -e '.install | has("graph")' "$case" >/dev/null 2>&1; then
    [[ "$kind" == "graph" ]] || fail "$(basename "$case"): graph case in a $kind pack"
    install_graph "$case"
  elif jq -e '.install | has("documents")' "$case" >/dev/null 2>&1; then
    [[ "$kind" == "documents" || "$kind" == "graph" ]] || fail "$(basename "$case"): documents case in a $kind pack"
    install_documents "$case"
  elif jq -e '.install | has("assets")' "$case" >/dev/null 2>&1; then
    [[ "$kind" == "assets" ]] || fail "$(basename "$case"): assets case in a $kind pack"
    install_assets "$case"
  elif jq -e 'has("runtime")' "$case" >/dev/null; then
    runtime_case "$case"
  elif jq -e '.install | has("plugins")' "$case" >/dev/null; then
    [[ "$kind" == "plugins" ]] || fail "$(basename "$case"): plugins case in a $kind pack"
    install_plugins "$case"
  elif jq -e 'has("jq")' "$case" >/dev/null; then
    run_jq_case "$case"
  elif jq -e 'has("eval_case")' "$case" >/dev/null; then
    eval_case "$case"
  elif jq -e 'has("cli_flags")' "$case" >/dev/null; then
    cli_flags "$case"
  else
    fail "$(basename "$case"): not a graphs, install, runtime, jq, eval_case or cli_flags case"
  fi
done

# A plugin may ship tools/wasm_check.py: checks of its real WebAssembly build that case files and
# native tests cannot express (damaged inputs, output limits, large inputs). It runs once per plugin
# source, against the pack installed in a fresh home (env GENTS and PACK_HOME), and needs python3.
wasm_checks() {
  local plugin source check home name seen=" " args=()
  while read -r plugin source; do
    check="$dir/$source/tools/wasm_check.py"
    [[ -n "$source" && -f "$check" && "$seen" != *" $source "* ]] || continue
    seen+="$source "
    command -v python3 >/dev/null 2>&1 || { echo "note: python3 not on PATH; skipping $plugin wasm checks" >&2; continue; }
    name="wasm-$plugin"
    home="$(fresh_home "$name")"
    args=()
    while read -r arg; do args+=("$arg"); done < <(slot_args "$home")
    store_dependencies "$home"
    "$gents" pack install "$dir" --home "$home" --grant-authority ${args[@]+"${args[@]}"} >"$work/$name-install.json"
    if GENTS="$gents" PACK_HOME="$home" python3 "$check" >"$work/$name.log" 2>&1; then
      pass "plugin $plugin: wasm checks ($(tail -n 2 "$work/$name.log" | head -n 1))"
    else
      fail "plugin $plugin: wasm checks failed"
      cat "$work/$name.log" >&2
    fi
  done < <(jq -r '.plugins // [] | .[] | select(.language == "rust") | "\(.name) \(.source // "")"' "$dir/manifest.json")
}
wasm_checks

if ((failures > 0)); then
  echo "$pack: $failures failed" >&2
  exit 1
fi
echo "$pack: all passed"
