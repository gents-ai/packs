#!/usr/bin/env bash
# Loads a research claim graph (claims/, edges/, entities/, perspectives/ JSONL) into a
# served gents home through the claim_graph gate, one ClaimBatch at a time, waiting for
# each batch's GateReceipt before the next.
#
# usage: tools/import_jsonl.sh [options] <graph-dir> <home> <graphql-url>
#
#   --source-root[=DIR]  check files and quotes against DIR (default: the parent of
#                        <graph-dir>); DIR must be inside a folder allowed with
#                        `gents plugin dirs add`. Without it those checks are skipped.
#   --date YYYYMMDD      the date part of every batch_id (default: today, UTC)
#   --suffix S           appended to every batch_id, to import again under new ids
#   --chunk-bytes N      the most JSON-encoded bytes of lines one batch carries
#                        (default 100000; one --json argument must stay under
#                        Linux's 128 KiB argument limit)
#   --timeout SECS       how long to wait for each receipt (default 300)
#   --dry-run            print the batch plan (one JSON object per batch) and stop
#   --strict             refuse a graph whose edges repeat a (from, type, to) key
#                        instead of dropping the later copies with a warning
#   GENTS                the gents binary (default: gents)
#
# Order: perspectives and entities first (the first gazetteer batch carries every
# perspective), then one or more batches per claims/<section>.jsonl, then the remaining
# edges. A supersedes or retracts edge travels in the batch of the claim it points at,
# because the gate skips the quote check only for claims such an edge points at, and
# sections are ordered so the superseding claim is admitted first.
#
# Resume: every batch is a family of attempts, <batch_id> then <batch_id>-r2, -r3, ...
# A family with a passing receipt is skipped. An attempt without a receipt yet is
# awaited, not resubmitted. A family whose attempts all failed (an earlier run) is
# resubmitted once as <batch_id>-rN, N one more than its attempts so far. A failure in
# this run stops the import with its errors, so the next run retries it. The gate refuses an edge key it already holds,
# so an edge repeating an earlier (from, type, to) is dropped, keeping the first, with a
# warning on stderr per dropped line (an error with --strict).
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//' >&2; exit 2; }

source_root="" use_source_root="" date="$(date -u +%Y%m%d)" suffix="" chunk=100000 timeout=300 dry_run="" strict=""
args=()
while (($#)); do
  case "$1" in
    --source-root) use_source_root=parent ;;
    --source-root=*) use_source_root=dir; source_root="${1#*=}" ;;
    --date) date="$2"; shift ;;
    --suffix) suffix="$2"; shift ;;
    --chunk-bytes) chunk="$2"; shift ;;
    --timeout) timeout="$2"; shift ;;
    --dry-run) dry_run=yes ;;
    --strict) strict=yes ;;
    -h | --help) usage ;;
    -*) echo "unknown option $1" >&2; usage ;;
    *) args+=("$1") ;;
  esac
  shift
done
((${#args[@]} == 3)) || usage
graph="$(cd "${args[0]}" && pwd)" home="${args[1]}" url="${args[2]}"
gents="${GENTS:-gents}"
[[ -d "$graph/claims" ]] || { echo "$graph has no claims/ directory" >&2; exit 2; }
case "$use_source_root" in
  parent) source_root="$(dirname "$graph")" ;;
  dir) source_root="$(cd "$source_root" && pwd)" ;;
esac
command -v jq >/dev/null || { echo "jq is required" >&2; exit 2; }

# Every non-empty line of a JSONL file, as {raw, rec} with its text unchanged.
lines() { jq -Rc 'select(test("\\S")) | {raw: ., rec: fromjson}' "$@"; }

shopt -s nullglob
edges="$(for f in "$graph"/edges/*.jsonl; do
  jq -Rc --arg f "edges/$(basename "$f")" 'select(test("\\S")) | {k: "edge", raw: ., rec: fromjson, at: "\($f):\(input_line_number)"}' "$f"
done)"
def_key='def key: .rec.from + "|" + .rec.type + "|" + .rec.to;'
duplicates="$(jq -sr "$def_key"'
  reduce .[] as $e ({seen: {}, out: []}; ($e | key) as $k
    | if .seen[$k] then .out += ["\($e.at) repeats edge \($k) first given at \(.seen[$k])"]
      else .seen[$k] = $e.at end) | .out[]' <<<"$edges")"
if [[ -n "$duplicates" ]]; then
  if [[ -n "$strict" ]]; then
    sed 's/^/error: /' <<<"$duplicates" >&2
    exit 1
  fi
  sed 's/^/warning: dropped /' <<<"$duplicates" >&2
  edges="$(jq -sc "$def_key"'
    reduce .[] as $e ({seen: {}, out: []}; ($e | key) as $k
      | if .seen[$k] then . else .seen[$k] = true | .out += [$e | del(.at)] end) | .out[]' <<<"$edges")"
fi
edges="$(jq -c 'del(.at)' <<<"$edges")"
plan="$(
  {
    for f in "$graph"/claims/*.jsonl; do
      lines "$f" | jq -c --arg s "$(basename "$f" .jsonl)" '{k: "claim", section: $s} + .'
    done
    for f in "$graph"/entities/*.jsonl; do lines "$f" | jq -c '{k: "entity"} + .'; done
    for f in "$graph"/perspectives/*.jsonl; do lines "$f" | jq -c '{k: "perspective"} + .'; done
    printf '%s\n' "$edges"
  } | jq -sc --argjson limit "$chunk" --arg date "$date" --arg suffix "$suffix" --arg root "$source_root" '
    def size: (.raw | tojson | length) + 2;
    # Greedy chunks of items under $limit encoded bytes.
    def chunks: reduce .[] as $i ([];
      if length == 0 or ((.[-1] | map(size) | add) + ($i | size) > $limit) then . + [[$i]]
      else .[-1] += [$i] end);
    def text($k): map(select(.k == $k) | .raw) | if length == 0 then null else join("\n") + "\n" end;
    def batch($id; $section; $items):
      {batch_id: ($id + $suffix), section: $section,
       claims: ($items | text("claim")), edges: ($items | text("edge")),
       entities: ($items | text("entity")), perspectives: ($items | text("perspective"))}
      | with_entries(select(.value != null));

    . as $all | map(select(.k == "claim")) as $claims
    | (reduce $claims[] as $c ({}; .[$c.rec.id] = $c.section)) as $sec
    | (map(select(.k == "edge" and (.rec.type == "supersedes" or .rec.type == "retracts")
        and $sec[.rec.to] != null))) as $deferred
    | ([$deferred[] | select($sec[.rec.from] != null and $sec[.rec.from] != $sec[.rec.to])
        | [$sec[.rec.from], $sec[.rec.to]]] | unique) as $deps
    # Kahn order over sections, ties by name.
    | ([$claims[].section] | unique) as $sections
    | {done: [], left: $sections}
    | until(.left == [];
        . as $s
        | ($s.left | map(select(. as $t | $deps | all(.[1] != $t or (.[0] as $f | $s.done | index($f)))))) as $ready
        | if $ready == [] then error("supersedes edges order the sections in a cycle: \($s.left)")
          else {done: ($s.done + [$ready[0]]), left: ($s.left - [$ready[0]])} end)
    | .done as $order
    | ($all | map(select(.k == "perspective"))) as $perspectives
    | ($all | map(select(.k == "entity")) | chunks) as $entity_chunks
    | (if ($entity_chunks | length) == 0 then [[]] else $entity_chunks end) as $entity_chunks
    | [range(0; $entity_chunks | length) as $n
        | batch("gazetteer-\($date)-\($n + 1)"; "gazetteer";
            (if $n == 0 then $perspectives else [] end) + $entity_chunks[$n])
        | select(has("entities") or has("perspectives"))]
    + [$order[] as $s
        | ($claims | map(select(.section == $s)) | chunks) as $parts
        | (reduce range(0; $parts | length) as $n ({}; reduce $parts[$n][] as $c (.; .[$c.rec.id] = $n))) as $part
        | ($deferred | map(select($sec[.rec.to] == $s))) as $mine
        | ($mine[] | select($sec[.rec.from] == $s and $part[.rec.from] > $part[.rec.to])
            | error("\(.rec.from) supersedes \(.rec.to) from a later batch of \($s); raise --chunk-bytes")),
          (range(0; $parts | length) as $n
            | batch("\($s)-\($date)-\($n + 1)"; $s;
                $parts[$n] + ($mine | map(select($part[.rec.to] == $n))))
            | if $root != "" then .source_root = $root else . end)]
    + ($all | map(select(.k == "edge")) - $deferred | chunks
        | [range(0; length) as $n | batch("edges-\($date)-\($n + 1)"; "edges"; .[$n])])
    | .[]
    | .extractor = "import_jsonl.sh"'
)"

if [[ -n "$dry_run" ]]; then
  jq -c '{batch_id, section, source_root,
    lines: (with_entries(select(.key | IN("claims", "edges", "entities", "perspectives")))
      | map_values(split("\n") | length - 1))}' <<<"$plan"
  exit 0
fi

# The GateReceipts or ClaimBatches of one batch's attempts: the batch id itself and its
# retries <id>-r2, <id>-r3, ...; the regex keeps <id>-10 out of <id>-1's family.
family() {
  local collection="$1" base="$2"
  shift 2
  "$gents" query find --home "$home" --graphql "$url" --collection "$collection" --limit 1000 \
    --filter "$(jq -cn --arg b "$base" '{_or: [{batch_id: {_eq: $b}}, {batch_id: {_like: ($b + "-r%")}}]}')" \
    --field batch_id "$@" 2>/dev/null \
    | jq -c --arg b "$base" '[.results[]? | select(.batch_id | test("^" + ($b | gsub("(?<c>[.^$*+?()\\[\\]{}|\\\\])"; "\\\(.c)")) + "(-r[0-9]+)?$"))]
        | sort_by(.batch_id | capture("-r(?<n>[0-9]+)$").n // "1" | tonumber)' 2>/dev/null || echo '[]'
}
receipts() {
  family GateReceipt "$1" --field verdict --field errors --field claims_admitted --field edges_admitted \
    --field entities_admitted --field perspectives_admitted --field quotes_checked --field quotes_skipped
}
# Waits for the receipt of attempt <id> and prints it.
await_receipt() {
  local id="$1" base="$2" deadline=$((SECONDS + timeout)) got
  while :; do
    got="$(receipts "$base" | jq -c --arg id "$id" '.[] | select(.batch_id == $id)')"
    [[ -n "$got" ]] && { printf '%s' "$got"; return; }
    ((SECONDS < deadline)) || {
      echo "no GateReceipt for $id after ${timeout}s; is the server running the claim-gate callback?" >&2
      exit 1
    }
    sleep 1
  done
}
print_errors() { jq -r '.errors | fromjson? // . | if type == "array" then .[] else . end' <<<"$1" >&2; }

total=$(jq -s length <<<"$plan") n=0
while read -r batch; do
  n=$((n + 1))
  base="$(jq -r .batch_id <<<"$batch")"
  attempts="$(family ClaimBatch "$base")"
  # An attempt still waiting for its receipt is awaited, never submitted again.
  while read -r pending; do
    [[ -n "$pending" ]] || continue
    if [[ -z "$(receipts "$base" | jq -c --arg id "$pending" '.[] | select(.batch_id == $id)')" ]]; then
      echo "[$n/$total] waiting for the receipt of $pending" >&2
      await_receipt "$pending" "$base" >/dev/null
    fi
  done < <(jq -r '.[].batch_id' <<<"$attempts")
  recorded="$(receipts "$base")"
  got="$(jq -c 'map(select(.verdict == "pass")) | .[0] // empty' <<<"$recorded")"
  id="$(jq -r '.batch_id // empty' <<<"$got")"
  if [[ -z "$got" ]]; then
    tries="$(jq -s '.[0] + .[1] | map(.batch_id) | unique | length' <(printf '%s' "$attempts") <(printf '%s' "$recorded"))"
    if ((tries == 0)); then
      id="$base"
    else
      id="$base-r$((tries + 1))"
      echo "[$n/$total] $base failed the gate before; retrying $base as $id" >&2
    fi
    fields="$(jq -c --arg id "$id" --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      '. + {batch_id: $id, submitted_at: $at}' <<<"$batch")"
    "$gents" document create ClaimBatch --home "$home" --graphql "$url" --json "$fields" >/dev/null
    got="$(await_receipt "$id" "$base")"
    if [[ "$(jq -r .verdict <<<"$got")" != pass ]]; then
      echo "[$n/$total] $id failed the gate:" >&2
      print_errors "$got"
      exit 1
    fi
  fi
  jq -r --arg n "$n/$total" --arg id "$id" \
    '"[\($n)] \($id) pass: \(.claims_admitted) claims, \(.edges_admitted) edges, \(.entities_admitted) entities, \(.perspectives_admitted) perspectives, quotes \(.quotes_checked) checked \(.quotes_skipped) skipped"' <<<"$got"
done <<<"$plan"
