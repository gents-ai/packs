#!/usr/bin/env bash
# Live matrix runner for rhai_workflows (rows/r<N>.json is the table, rows 1-22, then the corpus legs).
# Per backend and row it builds a home, installs both packs as operator, wires the
# caller with the row's packs only, narrows the wired block to the row's targets,
# starts a server, seeds the row prompt and checks the job with `gents job`.
#   matrix.sh [--list] [--rows 1,3,5|corpus] [--backend 1|2] [--dry-run]
# Needs GENTS (or RW_GENTS_BIN); backend n reads LIVE_ENDPOINT_<n> and LIVE_MODEL_<n>.
# Corpus legs (no --rows, or --rows corpus): each *.rhai file in GROK_CORPUS_DIR runs as a local path on wf-research.
# Evidence per row: <run dir>/<backend>/row-<N>/, with <run dir> a new rhai_workflows_live-matrix-<time>
# directory under $RUN_DIR (default $TMPDIR).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCEN="$HERE"
ENGINE="$(cd "$HERE/../../packs/gents/rhai_workflows" && pwd)"
WEBP="$(cd "$HERE/../../packs/gents/rhai_workflows_web" && pwd)"
WEB="$(cd "$HERE/../../packs/gents/web" && pwd)"
GENTS="${GENTS:-${RW_GENTS_BIN:-}}"
TIMEOUT="${MATRIX_TIMEOUT_SECS:-1800}"
DRY=0 LIST=0 ROWS="" BACKENDS="1 2" CORPUS=0
DONE='{"watch_exit":0,"state":"completed"}'

while [ $# -gt 0 ]; do
  case "$1" in
    --list) LIST=1 ;;
    --dry-run) DRY=1 ;;
    --rows) ROWS="${2:?--rows needs a list such as 1,3,5}"; shift ;;
    --backend) BACKENDS="${2:?--backend needs 1 or 2}"; shift ;;
    *) echo "usage: matrix.sh [--list] [--rows 1,3,5|corpus] [--backend 1|2] [--dry-run]" >&2; exit 2 ;;
  esac
  shift
done
case "$BACKENDS" in "1 2" | 1 | 2) ;; *) echo "--backend takes 1 or 2" >&2; exit 2 ;; esac

all_rows() { local n=1; while [ -f "$HERE/rows/r$n.json" ]; do echo "$n"; n=$((n + 1)); done; }

if [ "$LIST" = 1 ]; then
  for n in $(all_rows); do
    jq -r '"row \(.row): \(.title) [\(.legs | join(", "))]"' "$HERE/rows/r$n.json"
  done
  echo "client legs, every run: gents job list, show, watch and stop; gents query over AgentToolCall.job_progress, pause_reason included"
  echo "corpus legs: each *.rhai file in GROK_CORPUS_DIR as a local path run on wf-research; a refusal is reported, not failed"
  exit 0
fi

ROW_LIST="$(all_rows)"
if [ -z "$ROWS" ]; then
  CORPUS=1
elif [ "$ROWS" = corpus ]; then
  CORPUS=1 ROW_LIST=""
else
  ROW_LIST="$(echo "$ROWS" | tr ',' ' ')"
  for n in $ROW_LIST; do
    [ -f "$HERE/rows/r$n.json" ] || { echo "no row $n" >&2; exit 2; }
  done
fi
if [ -z "$GENTS" ]; then
  [ "$DRY" = 1 ] || { echo "set GENTS or RW_GENTS_BIN" >&2; exit 2; }
  GENTS=gents
fi
RUN_DIR="${RUN_DIR:-${TMPDIR:-/tmp}}"
case "$RUN_DIR" in */rhai_workflows_live-matrix-*) ;; *) RUN_DIR="${RUN_DIR%/}/rhai_workflows_live-matrix-$(date -u +%Y%m%dT%H%M%SZ)" ;; esac
RUNID="$(basename "$RUN_DIR")"

# x runs a command (dry-run prints it); xo does the same with stdout in a file.
x() { if [ "$DRY" = 1 ]; then printf '+'; printf ' %q' "$@"; printf '\n'; else "$@"; fi; }
xo() { local f="$1"; shift; if [ "$DRY" = 1 ]; then printf '+'; printf ' %q' "$@"; printf ' > %q\n' "$f"; else "$@" >"$f"; fi; }
fail() { echo "fail: row $N backend $B: $*"; exit 1; }
until_ok() { local end=$((SECONDS + $1)); shift; while ! "$@"; do [ "$SECONDS" -lt "$end" ] || return 1; sleep 2; done; }

# The Tools document as `config tools show` prints it, without _docID, JSON fields decoded.
show_tools() {
  "$GENTS" config tools show "$1" "$2" "$3" | jq 'with_entries(if (.key | IN("integrations", "agents", "host")) and (.value | type) == "string" then .value |= fromjson else . end) | del(._docID)'
}
# tools_edit <tools id> <--home|--graphql> <value> <saved file> <jq filter> [jq args]
tools_edit() {
  local id="$1" flag="$2" at="$3" out="$4" filter="$5"
  shift 5
  show_tools "$id" "$flag" "$at" | jq "$@" "$filter" >"$out"
  "$GENTS" config tools set --file "$out" "$flag" "$at" >/dev/null
}
# narrow_caller <home> <targets json>: only ever removes targets from the wired block.
narrow_caller() {
  local home="$1" keep="$2" bad
  bad="$(show_tools wf-live-caller-tools --home "$home" | jq -r --argjson keep "$keep" '([.integrations.plugins[] | select(.plugin == "gents/workflow") | .job.agent_targets] | .[0]) as $w | $keep[] | select(IN($w[]) | not)')"
  [ -z "$bad" ] || { echo "narrowing would widen: $(echo "$bad" | head -1)" >&2; exit 2; }
  tools_edit wf-live-caller-tools --home "$home" "$R/caller-tools-narrowed.json" '(.integrations.plugins[] | select(.plugin == "gents/workflow") | .job) |= (.agent_targets = $keep | .policy.dynamic_targets |= map(select(IN($keep[])))) | .agents.target_ids = $keep' --argjson keep "$keep"
}

start_server() {
  (cd "$W" && NO_COLOR=1 exec "$GENTS" server --home "$H" --http-port "$PORT" --p2p-transport none --no-codex-shim --tool-ceiling readonly --tool-root "$W") >>"$R/server.log" 2>&1 &
  SERVER=$!
  until_ok 240 curl -fsS -o /dev/null "http://127.0.0.1:$PORT/healthz" || fail "the server did not come up"
}
stop_server() { if [ -n "${SERVER:-}" ]; then kill "$SERVER" 2>/dev/null || true; wait "$SERVER" 2>/dev/null || true; SERVER=; fi; }

# Jobs are read through GraphQL; the address is session_id:tool_call_id.
show_json() { "$GENTS" job show "$1" --graphql "$URL" --output json; }
newest() { "$GENTS" job list --graphql "$URL" --plugin gents/workflow --state all --output json | jq -r '.[0].address // empty'; }
first_address() { ADDR="$(newest)"; [ -n "$ADDR" ]; }
moved() { local a; a="$(newest)"; [ "$a" != "$1" ] || [ "$(show_json "$a" | jq -r .state)" != "$2" ]; }
no_live() { show_json "$ADDR" | jq -e '[.children[] | select(.state == "live")] | length == 0' >/dev/null; }
has_live() { show_json "$ADDR" | jq -e '[.children[] | select(.state == "live")] | length > 0' >/dev/null; }
wf_call() { printf 'Call the workflow tool once with argv %s and tell me exactly what it returns. Then end your turn.' "$1"; }

# Client leg: AgentToolCall.job_progress (pause_reason included) and the call outputs, read with gents query.
# AgentToolCall.terminal_output is null for a plugin call, so each call's result text is filled in from the
# request timeline, by tool_call_id, wherever the query left it null.
calls_query() {
  "$GENTS" query find --collection AgentToolCall --filter '{"tool_name":{"_eq":"workflow"}}' --field tool_call_id --field request_id --field lifecycle_state --field terminal_output --field job_progress --limit 100 --graphql "$URL" >"$R/query-calls.json" || return
  local rid
  for rid in $(jq -r '[.results[].request_id] | unique | .[]' "$R/query-calls.json"); do
    "$GENTS" trace timeline --home "$H" --graphql "$URL" --request-id "$rid" --output-file "$R/timeline.json" >/dev/null 2>&1 || continue
    jq --slurpfile t "$R/timeline.json" '([$t[0].events[] | select(.kind == "tool_call") | {key: .tool_call_id, value: .result}] | from_entries) as $m | .results |= map(.terminal_output //= $m[.tool_call_id])' "$R/query-calls.json" >"$R/query-calls.tmp" && mv "$R/query-calls.tmp" "$R/query-calls.json"
  done
}
# calls_done <n>: at least n workflow calls ended.
calls_done() { calls_query && [ "$(jq '[.. | objects | select(.lifecycle_state? | IN("completed", "failed", "timedOut", "cancelled"))] | length' "$R/query-calls.json")" -ge "$1" ]; }
# calls_name <text>: a workflow call's result holds the text.
calls_name() { calls_query && grep -qF -- "$1" "$R/query-calls.json"; }
# code_seen <code regex>: in a workflow call's output or the server log.
code_seen() { calls_query || true; grep -Eq -- "$1" "$R/query-calls.json" "$R/server.log" 2>/dev/null; }
refusal_seen() { local code; code="$(jq -r .code <<<"$EXPECT")"; until_ok "$TIMEOUT" code_seen "$code" || fail "no $code in the workflow calls or the server log"; }
# scratch_file <name>: the file the run wrote under its tool call id, wherever the session folder is.
scratch_file() { find "$R" -path "*/.gents/workflows/runs/${ADDR##*:}/$1" | sed -n 1p; }
# whole_result <bytes> <call id>: a call other than the run itself returned a string of at least that size.
whole_result() { calls_query && jq -e --argjson n "$1" --arg run "$2" '[.. | objects | select((.tool_call_id? // $run) != $run) | .. | strings | length] | max >= $n' "$R/query-calls.json" >/dev/null; }
# notice_capped <call id>: the run's completion notice in the caller's session (an AgentMessage keyed
# background-completion-notification:, read through gents trace timeline) shows at most 4,000 B of
# the result, unescaped and without the truncation marker. [unverified: the timeline's message JSON shape]
notice_capped() {
  local req
  req="$("$GENTS" query find --collection AgentToolCall --filter "$(jq -nc --arg id "$1" '{tool_call_id: {_eq: $id}}')" --field request_id --limit 1 --graphql "$URL" | jq -r '[.. | objects | .request_id? | strings] | .[0] // empty')"
  [ -n "$req" ] && "$GENTS" trace timeline --request-id "$req" --graphql "$URL" --output-file "$R/timeline-caller.json" &&
    jq -e --arg id "$1" '[.. | objects | select(.header.message_key? // "" | startswith("background-completion-notification:")) | [.message | .. | strings] | join("") | select(contains("tool_call_id=\"" + $id + "\"")) | capture("<result>(?<b>[^<]*)</result>").b | gsub("&lt;"; "<") | gsub("&gt;"; ">") | gsub("&amp;"; "&") | rtrimstr("...") | utf8bytelength] | length > 0 and max <= 4000' "$R/timeline-caller.json" >/dev/null
}
# notice_has <call id> <text>: the completion notice of that run, in the caller's session, shows the text.
notice_has() {
  local req
  req="$("$GENTS" query find --collection AgentToolCall --filter "$(jq -nc --arg id "$1" '{tool_call_id: {_eq: $id}}')" --field request_id --limit 1 --graphql "$URL" | jq -r '[.. | objects | .request_id? | strings] | .[0] // empty')"
  [ -n "$req" ] && "$GENTS" trace timeline --request-id "$req" --graphql "$URL" --output-file "$R/timeline-caller.json" &&
    jq -e --arg id "$1" --arg t "$2" '[.. | objects | select(.header.message_key? // "" | startswith("background-completion-notification:")) | [.message | .. | strings] | join("") | select(contains("tool_call_id=\"" + $id + "\"") and contains($t))] | length > 0' "$R/timeline-caller.json" >/dev/null
}

# child_target <target> <show json>: the one child's request ran on that agent (AgentRequest.agent_id is the target id).
child_target() {
  local rid
  rid="$(jq -r '.children[0].request_id' "$2")"
  "$GENTS" query find --collection AgentRequest --filter "$(jq -nc --arg id "$rid" '{request_id: {_eq: $id}}')" --field agent_id --limit 1 --graphql "$URL" | jq -e --arg t "$1" '.results[0].agent_id == $t' >/dev/null
}

# Row 20: every row of the run named by $h20 as one JSON array of `gents job show` documents, and its head
# (the row no other row resumed).
chain_rows() {
  local a out='[]'
  for a in $("$GENTS" job list --graphql "$URL" --plugin gents/workflow --state all --output json | jq -r --arg h "$h20" '.[] | select(.handle == $h) | .address'); do
    out="$(jq -c --argjson s "$(show_json "$a")" '. + [$s]' <<<"$out")"
  done
  printf '%s' "$out"
}
chain_head() { chain_rows | jq -c '([.[] | .pin.resume_of | select(. != null)]) as $r | [.[] | select(.row_doc_id as $id | $r | index($id) | not)] | .[0] // empty'; }
chain_head_address() { chain_head | jq -r .address; }
chain_head_is() { [ "$(chain_head | jq -r '.state // empty')" = "$1" ]; }

# Client legs after the row's own: gents job list and show, and the query; the server may be stopped by then.
client_legs() {
  local at=(--graphql "$URL")
  [ -n "$SERVER" ] || at=(--home "$H")
  "$GENTS" job list "${at[@]}" --state all --output json >"$R/client-list.json"
  [ -z "$ADDR" ] || "$GENTS" job show "$ADDR" "${at[@]}" --output json >"$R/client-show.json"
  "$GENTS" query find --collection AgentToolCall --field tool_call_id --field job_progress --limit 100 "${at[@]}" >"$R/client-progress.json"
}

# The manual Grok pager and desktop checks cannot run headless (L2 section 9.2 item 6).
manual_checklist() {
  local a="${ADDR:-<session_id:tool_call_id>}"
  echo "manual checks for row $N (run address $a):"
  echo "  grok pager: open the run in the pager; its state and pause_reason match: gents job show $a"
  echo "  desktop: the session timeline shows one job row for $a, with the same state and pause_reason as gents job show"
}

# send <prompt>: queue another prompt in the job's own session. A seed opens a new session, and
# the engine resolves a <key|handle> only in the caller's session, so control legs go through the
# follow-up task with --continue-session. The address splits at its last colon.
send() {
  SEQ=$((SEQ + 1))
  local session=()
  [ -z "$ADDR" ] || session=(--continue-session "${ADDR%:*}")
  "$GENTS" task run rhai-workflows-live-follow --home "$H" --graphql "$URL" ${session[@]+"${session[@]}"} --args "$(jq -nc --arg p "$1" '{prompt: $p}')" >"$R/follow-$SEQ.json"
}
# follow <prompt>: send it; ADDR becomes the job it changes or starts.
follow() {
  local prev="$ADDR" st
  st="$(show_json "$ADDR" | jq -r .state)"
  send "$1"
  until_ok "$TIMEOUT" moved "$prev" "$st" || fail "no job changed after follow-up $SEQ"
  ADDR="$(newest)"
}

# watch_check <label> <expect json>: watch to the end or the pause, then compare with the row's expectation.
watch_check() {
  local label="$1" want="$2" rc=0 line v
  line="$("$GENTS" job watch "$ADDR" --graphql "$URL" --until-terminal)" || rc=$?
  printf '%s\n' "$line" >"$R/watch-$label.json"
  show_json "$ADDR" >"$R/show-$label.json"
  v="$(jq -r .watch_exit <<<"$want")"
  [ "$rc" = "$v" ] || fail "$label: watch exit $rc, want $v"
  v="$(jq -r .state <<<"$want")"
  jq -e --arg s "$v" '.state == $s' "$R/show-$label.json" >/dev/null || fail "$label: state $(jq -r .state "$R/show-$label.json"), want $v"
  v="$(jq -r '.pause_reason // empty' <<<"$want")"
  [ -z "$v" ] || jq -e --arg s "$v" '.progress.pause_reason == $s' "$R/show-$label.json" >/dev/null || fail "$label: pause_reason is not $v"
  if [ -n "$v" ]; then
    calls_query
    grep -q "$v" "$R/query-calls.json" || fail "$label: no $v in AgentToolCall.job_progress"
  fi
  v="$(jq -r '.code // empty' <<<"$want")"
  [ -z "$v" ] || { calls_query || true; grep -rEq --exclude-dir=home --exclude-dir=work -- "$v" "$R" || fail "$label: no $v in the evidence"; }
}

# The legs of each row after the first seed; ADDR is the row's job.
legs() {
  case "$N" in
    1 | 2 | 4) watch_check final "$EXPECT" ;;
    3)
      # The follow-up names the run by its handle and the legs follow the chain: a model may resume the
      # run itself in the seed turn, and a refused resume leaves the head where it was.
      watch_check paused "$EXPECT"
      [ "$(jq '.children | length' "$R/show-paused.json")" = 1 ] || fail "a panel child started under the exhausted budget"
      h20="$(jq -r .handle "$R/show-paused.json")"
      send "$(wf_call "[\"resume\",\"$h20\",\"--budget\",\"4\"]") The new budget must be exactly 4: keep --budget and 4 in argv. If the call is refused, make the call the error names."
      until_ok "$TIMEOUT" chain_head_is completed || fail "no row of $h20 completed"
      ADDR="$(chain_head_address)"
      watch_check resumed "$DONE"
      chain_rows >"$R/chain.json"
      jq -e --slurpfile a "$R/show-paused.json" '
        (map(.handle) | unique == [$a[0].handle]) and (map(.pin.chain_root_doc_id) | unique == [$a[0].pin.chain_root_doc_id])
        and ([.[] | select(.pin.resume_of == null) | .row_doc_id] == [$a[0].row_doc_id])
        and (map(.row_doc_id) as $ids | [.[] | .pin.resume_of | select(. != null)] as $r | ($r | length) == (length - 1) and ($r | unique | length) == ($r | length) and all($r[]; . as $x | $ids | index($x)))
        and (map(select(.state == "completed")) | length == 1)
        and ([.[] | select(.state == "completed") | .pin.agent_budget] | .[0] >= 4)' "$R/chain.json" >/dev/null || fail "the chain is not one line ending in a completed row with a budget of 4 or more"
      ;;
    5)
      watch_check paused "$EXPECT"
      follow "$(wf_call "[\"resume\",\"$ADDR\"]")"
      watch_check resumed "$DONE" ;;
    6)
      # The run is named by its short handle, and the legs follow its chain: a model may start the workflow again
      # in either turn, which makes a second run under another handle that is not the one answered.
      watch_check question "$EXPECT"
      h20="$(jq -r .handle "$R/show-question.json")"
      send "$(wf_call "[\"answer\",\"$h20\",\"teal\"]") The answer text teal is the third argv item; send no args and do not run the workflow again."
      until_ok "$TIMEOUT" chain_head_is completed || fail "no row of $h20 completed"
      ADDR="$(chain_head_address)"
      watch_check resumed "$DONE" ;;
    7)
      until_ok "$TIMEOUT" has_live || fail "no live child before the stop"
      follow "$(wf_call "[\"stop\",\"$ADDR\"]")"
      watch_check stopped "$EXPECT"
      jq -e '[.children[] | select(.state == "live")] | length == 0' "$R/show-stopped.json" >/dev/null || fail "a child is still live after the in-session stop"
      # CLI leg: control is local-only, so stop the server first.
      follow "$PROMPT"
      until_ok "$TIMEOUT" has_live || fail "no live child before the CLI stop"
      stop_server
      "$GENTS" job stop "$ADDR" --home "$H" >"$R/job-stop.json"
      "$GENTS" job show "$ADDR" --home "$H" --output json >"$R/show-cli-stopped.json"
      # With no server the stop only latches each live child's interrupt; the node ends them when it next runs.
      jq -e '.state == "cancelled"' "$R/show-cli-stopped.json" >/dev/null || fail "the CLI stop left the row not cancelled"
      "$GENTS" query find --home "$H" --collection AgentRequest --field request_id --field interrupt_requested_at >"$R/query-latch.json" 2>/dev/null || fail "the latch query failed"
      jq -e --slurpfile q "$R/query-latch.json" '[.children[] | select(.state == "live") | .request_id as $id | select(($q[0].results | map(select(.request_id == $id and .interrupt_requested_at != null)) | length) == 0)] | length == 0' "$R/show-cli-stopped.json" >/dev/null || fail "a live child has no interrupt latched after the CLI stop"
      start_server
      until_ok "$TIMEOUT" no_live || fail "a child is still live after the node restarted"
      show_json "$ADDR" >"$R/show-cli-settled.json" ;;
    8)
      until_ok "$TIMEOUT" has_live || fail "no live child before the restart"
      kill -9 "$SERVER"; wait "$SERVER" 2>/dev/null || true; SERVER=
      start_server
      watch_check restarted "$EXPECT" ;;
    9)
      watch_check paused '{"watch_exit":3,"state":"paused"}'
      stop_server
      rebuild_engine
      "$GENTS" pack install "$ENGINE_NEW" --home "$H" --grant-authority "${ESLOTS[@]}" >"$R/install-rebuilt.json"
      # The caller's wiring pins the plugin digest, so the new engine needs the wiring and narrowing done again.
      "$GENTS" "${wire[@]}" >"$R/wire-rebuilt.json"
      narrow_caller "$H" "$targets"
      start_server
      local orig="$ADDR" code
      h20="$(jq -r .handle "$R/show-paused.json")"
      code="$(jq -r .code <<<"$EXPECT")"
      send "$(wf_call "[\"resume\",\"$orig\",\"--budget\",\"3\"]")"
      # The refusal is a failed row with the code, or a resume call that failed with it and started no row.
      until_ok 240 refused_resume "$code" "$orig" || fail "the refused resume left no row or call with $code"
      # The run is named by its handle and its chain followed: a model may start the workflow a second time under another handle.
      send "$(wf_call "[\"resume\",\"$h20\",\"--budget\",\"3\",\"--engine\",\"current\"]")"
      until_ok "$TIMEOUT" chain_head_is completed || fail "no row of $h20 completed"
      ADDR="$(chain_head_address)"
      watch_check resumed "$DONE" ;;
    10)
      # Narrow while the first panel is live: its 3 spawns are admitted, so a cap of 3 leaves none for
      # the second panel. The host writes a step's spawns and its terminal count in one row, so a cap
      # set after the first panel ends would land after the second was admitted. The job ends failed either
      # way: the engine refuses the second panel with workflow_policy_denied (a refusal at panel start is
      # returned as a fail, not thrown), or the host refuses its spawns with plugin_job_policy_denied.
      until_ok "$TIMEOUT" has_live || fail "no live child in the first panel"
      tools_edit wf-live-caller-tools --graphql "$URL" "$R/caller-tools-mid-run.json" '(.integrations.plugins[] | select(.plugin == "gents/workflow") | .job.max_spawns) = 3'
      "$GENTS" job watch "$ADDR" --graphql "$URL" --until-terminal >"$R/watch-end.json" || true
      if show_json "$ADDR" | jq -e '.state == "failed"' >/dev/null; then
        watch_check final "$(jq -c '.watch_exit = 1 | .state = "failed"' <<<"$EXPECT")"
      else
        watch_check final "$EXPECT"
      fi
      jq -e '[.children[] | select(.state == "live")] | length == 0' "$R/show-final.json" >/dev/null || fail "a child is still live" ;;
    11) watch_check final "$EXPECT" ;;
    12) refusal_seen ;;
    22)
      watch_check final "$EXPECT"
      jq -e --arg o "$(jq -r .origin <<<"$EXPECT")" '.pin.origin == $o' "$R/show-final.json" >/dev/null || fail "the run's pin origin is not $(jq -r .origin <<<"$EXPECT")"
      jq -e '(.children | length) == 1 and .children[0].state == "completed"' "$R/show-final.json" >/dev/null || fail "the run did not spawn one completed child"
      until_ok "$TIMEOUT" child_target "$(jq -r .target <<<"$EXPECT")" "$R/show-final.json" || fail "the child did not run on $(jq -r .target <<<"$EXPECT")"
      until_ok "$TIMEOUT" notice_has "${ADDR##*:}" "$(jq -r .marker <<<"$EXPECT")" || fail "the completion notice lacks the agent's output marker" ;;
    13)
      # The scenario pack is not trusted here, so the first spawn on wf-research is refused.
      if until_ok 240 first_address; then
        "$GENTS" job watch "$ADDR" --graphql "$URL" --until-terminal >"$R/watch-final.json" || true
        show_json "$ADDR" >"$R/show-final.json"
      fi
      refusal_seen ;;
    14)
      until_ok "$TIMEOUT" calls_done "$(jq -r .calls <<<"$EXPECT")" || fail "the keep calls did not all end"
      # Failed calls end too, so each keep is read from its result text.
      for k in refused replaced renamed; do
        grep -qF -- "$(jq -r --arg k "$k" '.[$k]' <<<"$EXPECT")" "$R/query-calls.json" || fail "no $k keep result in the workflow calls"
      done
      # The kept name runs as a job with a real agent() call: one child.
      until_ok "$TIMEOUT" first_address || fail "the kept workflow started no job row"
      watch_check run "$(jq -c .run <<<"$EXPECT")"
      jq -e '(.children | length) == 1 and .children[0].state == "completed"' "$R/show-run.json" >/dev/null || fail "the kept run did not spawn one completed child"
      child_target wf-live-worker "$R/show-run.json" || fail "the kept run's child did not run on wf-live-worker" ;;
    15)
      until_ok "$TIMEOUT" calls_done "$(jq -r .calls <<<"$EXPECT")" || fail "the export calls did not all end"
      export_install
      # The cutoff to installed origin is export, operator install and trusted_packs; job.policy.dynamic stays at check.
      tools_edit wf-live-caller-tools --home "$H" "$R/caller-tools-trusted.json" '(.integrations.plugins[] | select(.plugin == "gents/workflow") | .job.policy.trusted_packs) += [$p]' --arg p "$EXPORTED_PACK"
      start_server
      send "$(wf_call '["run","wf-live-exported"]')"
      until_ok "$TIMEOUT" first_address || fail "the exported workflow started no job row"
      watch_check run "$(jq -c .run <<<"$EXPECT")"
      jq -e --arg o "$(jq -r .origin <<<"$EXPECT")" '.pin.origin == $o and (.children | length) == 1 and .children[0].state == "completed"' "$R/show-run.json" >/dev/null || fail "the run is not installed origin with one completed child"
      child_target "$(jq -r .target <<<"$EXPECT")" "$R/show-run.json" || fail "the exported run's child did not run on $(jq -r .target <<<"$EXPECT")" ;;
    16)
      watch_check final "$EXPECT"
      [ -s "$(scratch_file report.md)" ] || fail "no report.md under runs/${ADDR##*:}/"
      # The job's result is read with workflow result: the run's own call only returns its address.
      send "$(wf_call "[\"result\",\"$ADDR\"]")"
      until_ok "$TIMEOUT" calls_name "runs/${ADDR##*:}/report.md" || fail "the result does not name the run's report path" ;;
    18)
      watch_check final "$EXPECT"
      [ -s "$(scratch_file scratch.txt)" ] || fail "no scratch.txt under runs/${ADDR##*:}/" ;;
    19)
      watch_check final "$EXPECT"
      until_ok 240 notice_capped "${ADDR##*:}" || fail "the completion notice is missing or shows more than 4000 B of the result"
      send "$(wf_call "[\"result\",\"$ADDR\"]")"
      until_ok "$TIMEOUT" whole_result "$(jq -r .result_bytes <<<"$EXPECT")" "${ADDR##*:}" || fail "workflow result did not return the whole value" ;;
    20)
      # The follow-ups name the run by its short handle, not by a 150-character address a model can
      # miscopy. The model may also resume the run itself in the seed turn, so the legs follow the
      # chain by handle: the head is the row nothing resumed, and the chain must stay one line.
      watch_check paused "$EXPECT"
      h20="$(jq -r .handle "$R/show-paused.json")"
      send "$(wf_call "[\"resume\",\"$h20\",\"--budget\",\"2\"]")"
      until_ok "$TIMEOUT" chain_head_is awaiting_user || fail "no row of $h20 reached awaiting_user"
      ADDR="$(chain_head_address)"
      watch_check question '{"watch_exit":3,"state":"awaiting_user"}'
      # The run waits for its answer: the prompt says answer, not resume, which the second backend's model swapped twice in five runs.
      send "$(printf 'Call the workflow tool once with argv ["answer","%s","teal"]. The run is waiting for this answer, so use the answer verb, not resume. Tell me exactly what it returns. Then end your turn.' "$h20")"
      until_ok "$TIMEOUT" chain_head_is completed || fail "no row of $h20 completed"
      ADDR="$(chain_head_address)"
      watch_check resumed "$DONE"
      chain_rows >"$R/chain.json"
      jq -e --slurpfile a "$R/show-paused.json" '
        ($a[0].handle | length > 0) and (map(.handle) | unique == [$a[0].handle]) and (map(.pin.chain_root_doc_id) | unique == [$a[0].pin.chain_root_doc_id])
        and ([.[] | select(.pin.resume_of == null) | .row_doc_id] == [$a[0].row_doc_id])
        and (map(.row_doc_id) as $ids | [.[] | .pin.resume_of | select(. != null)] as $r | ($r | length) == (length - 1) and ($r | unique | length) == ($r | length) and all($r[]; . as $x | $ids | index($x)))
        and any(.[]; .state == "awaiting_user") and (map(select(.state == "completed")) | length == 1)' "$R/chain.json" >/dev/null || fail "the handle, chain root or resume_of chain changed across resume and answer"
      until_ok "$TIMEOUT" notice_has "${ADDR##*:}" "$(jq -r .marker <<<"$EXPECT")" || fail "the result does not reflect the original args" ;;
    21)
      watch_check paused "$EXPECT"
      "$GENTS" job list --graphql "$URL" --state paused --output json >"$R/list-paused.json"
      jq -e --arg a "$ADDR" 'any(.[]; .address == $a and .progress.pause_reason == "budget_exceeded")' "$R/list-paused.json" >/dev/null || fail "job list --state paused does not show the row with budget_exceeded"
      jq -e --arg id "${ADDR##*:}" --arg a "$ADDR" '[.. | objects | select(.tool_call_id? == $id) | .terminal_output? | strings | select(contains($a))] | length > 0' "$R/query-calls.json" >/dev/null || fail "the inline result does not name $ADDR"
      # Named by its handle, with the chain followed: a model that resumes in some other way still leaves one head to watch.
      h20="$(show_json "$ADDR" | jq -r .handle)"
      send "$(wf_call "[\"resume\",\"$h20\",\"--budget\",\"3\"]") The budget is the two argv items --budget and 3."
      until_ok "$TIMEOUT" chain_head_is completed || fail "no row of $h20 completed"
      ADDR="$(chain_head_address)"
      watch_check resumed "$DONE" ;;
    corpus-*)
      # A script that is Grok-only, or whose spawn the home refuses, is reported as such, not failed.
      local rc=0
      if until_ok 300 first_address; then
        "$GENTS" job watch "$ADDR" --graphql "$URL" --until-terminal >"$R/corpus-watch.json" || rc=$?
        show_json "$ADDR" >"$R/corpus-show.json"
        if [ "$rc" = 0 ]; then echo "corpus $N: completed"; else echo "corpus $N: refused or ended, watch exit $rc, $(jq -r '.outcome.code // .state' "$R/corpus-show.json")"; fi
      else
        echo "corpus $N: no job started, the call was refused before step 0 (see $R/server.log)"
      fi ;;
    *) fail "no leg script for this row" ;;
  esac
}

# Row 9: a copy of the engine pack, outside the evidence directory, with its source changed and its
# .afb rebuilt: the installed engine digest is the sha256 of the .afb.
refused() {
  local a
  a="$(newest)"
  [ "$a" != "$ADDR" ] && show_json "$a" >"$R/show-refused.json" && grep -q "$1" "$R/show-refused.json"
}
refused_resume() { refused "$1" || { calls_name "$1" && [ "$(chain_head_address)" = "$2" ]; }; }
rebuild_engine() {
  ENGINE_NEW="$RUN_DIR/$B/engine-rebuilt-row-$N"
  mkdir -p "$ENGINE_NEW"
  rsync -a --exclude target/ "$ENGINE/" "$ENGINE_NEW/"
  # The copy lives outside the repo, so the pinned toolchain (the one with wasm32-wasip1) goes with it.
  cp "$HERE/../../rust-toolchain.toml" "$ENGINE_NEW/"
  printf '\n// rebuilt by matrix.sh row 9\n' >>"$ENGINE_NEW/plugins/workflow/source/lib.rs"
  jq '.version |= (split(".") | .[2] = ((.[2] | tonumber) + 1 | tostring) | join("."))' "$ENGINE/manifest.json" >"$ENGINE_NEW/manifest.json"
  rm -f "$ENGINE_NEW/plugins/workflow.afb"
  # The plugin builds below its own target/; a CARGO_TARGET_DIR from the caller moves the artifact and the build reports the target missing.
  env -u CARGO_TARGET_DIR "$GENTS" pack build "$ENGINE_NEW" --out "$ENGINE_NEW.pack" >"$R/build-rebuilt.json"
  [ -f "$ENGINE_NEW/plugins/workflow.afb" ] || fail "pack build left no workflow.afb in the rebuilt engine"
  ! cmp -s "$ENGINE/plugins/workflow.afb" "$ENGINE_NEW/plugins/workflow.afb" || fail "the rebuilt engine has the same .afb as the installed one"
}

# Row 15: write the files the export receipt returned, then install them by digest. The server is
# stopped first, as for every install. [unverified: the receipt reaches terminal_output as text]
export_install() {
  local out dir digest pk
  out="$(python3 -I - "$R/query-calls.json" "$R/exported" <<'PYEXPORT'
import json, os, re, sys

def strings(v):
    if isinstance(v, str):
        yield v
        try:
            d = json.loads(v)
        except ValueError:
            return
        if isinstance(d, (dict, list)):
            yield from strings(d)
    elif isinstance(v, dict):
        for x in v.values():
            yield from strings(x)
    elif isinstance(v, list):
        for x in v:
            yield from strings(x)

text = next((s for s in strings(json.load(open(sys.argv[1]))) if "(digest sha256:" in s), None)
if text is None:
    sys.exit("no export receipt in the evidence")
ns, pack, digest = re.search(r"as pack (\S+)/(\S+) \(digest (sha256:[0-9a-f]+)\)", text).groups()
b = text.encode()
root = os.path.join(sys.argv[2], pack)
header = re.compile(rb"=== (\S+) \((\d+) bytes\) ===\n")
pos = 0
while (m := header.search(b, pos)):
    path, n = m.group(1).decode(), int(m.group(2))
    dest = os.path.join(root, path)
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    open(dest, "wb").write(b[m.end():m.end() + n])
    pos = m.end() + n
print(digest, root, ns + "/" + pack)
PYEXPORT
)" || fail "could not read the export receipt"
  read -r digest dir pk <<<"$out"
  stop_server
  "$GENTS" pack install "$dir" --home "$H" --expect-digest "$digest" >"$R/install-exported.json" || fail "install of the exported pack by digest failed"
  EXPORTED_PACK="$pk"
}

run_row() {
  set -e
  B="$1" N="$2" SEQ=0 SERVER= ADDR=
  # A corpus leg ($3 is the script) runs on row 13's authority.
  local row="$HERE/rows/r$2.json" targets profile p wire
  [ -z "${3:-}" ] || row="$HERE/rows/r13.json"
  EXPECT="$(jq -c .expect "$row")" PROMPT="$(jq -r .prompt "$row")"
  [ -z "${3:-}" ] || PROMPT="$(printf 'Call the workflow tool once with argv ["run"] and path "%s". When the result arrives, tell me what it says, including any error code.' "$3")"
  if [ "$N" = 17 ]; then echo "row 17 backend $B: $(jq -r .expect.reference "$row")"; return 0; fi
  targets="$(jq -c .targets "$row")"
  R="$RUN_DIR/$B/row-$N" H="$R/home" W="$R/work"
  echo "row $N backend $B: evidence $R/"
  x mkdir -p "$W"
  x cp -R "$HERE/fixtures/." "$W/"
  GENTS_LIVE_ROOT="$W"; export GENTS_LIVE_ROOT
  x "$GENTS" pack scenario init "$SCEN" --home "$H"
  if [ "$DRY" = 1 ]; then
    profile="<profile>"; PORT="<port>"
    echo "+ $GENTS config profile list --home $H --output json  # items[0].profile_id"
  else
    profile="$("$GENTS" config profile list --home "$H" --output json | jq -r '.items[0].profile_id')"
    PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
    GENTS_HOME="$H"; export GENTS_HOME
  fi
  URL="http://127.0.0.1:$PORT/api/v0/graphql"
  # The engine takes bindings for its own slots only; the scenario pack takes those and its own.
  ESLOTS=() SLOTS=()
  while read -r p; do ESLOTS+=(--inference-slot "$p=$profile"); done < <(jq -r '[.inference_slots // [] | .[] | select(.optional != true) | .name] | unique | .[]' "$ENGINE/manifest.json")
  while read -r p; do SLOTS+=(--inference-slot "$p=$profile"); done < <(jq -rs '[.[] | .inference_slots // [] | .[] | select(.optional != true) | .name] | unique | .[]' "$ENGINE/manifest.json" "$SCEN/manifest.json")
  # Engine first, with its raised limits; the scenario pack ships no plugin and gets none.
  xo "$R/install-engine.json" "$GENTS" pack install "$ENGINE" --home "$H" --grant-authority "${ESLOTS[@]}"
  # wf-research and wf-read-only-web ship in rhai_workflows_web, which brings gents/web; only a row that targets wf-research (row 13, the corpus legs) installs it. Its grant covers every gents/web plugin.
  if jq -e '.targets | index("wf-research")' "$row" >/dev/null; then
    WSLOTS=()
    while read -r p; do WSLOTS+=(--inference-slot "$p=$profile"); done < <(jq -rs '[.[] | .inference_slots // [] | .[] | select(.optional != true) | .name] | unique | .[]' "$ENGINE/manifest.json" "$WEBP/manifest.json" "$WEB/manifest.json")
    # gents/web is a sibling dependency, resolved offline from the home's pack store as scripts/test-pack.sh does.
    xo "$R/build-web.json" "$GENTS" pack build "$WEB" --out "$RUN_DIR/$B/dep-web.pack"
    xo "$R/store-web.json" "$GENTS" pack fetch "$RUN_DIR/$B/dep-web.pack" --store --home "$H"
    xo "$R/install-web-personas.json" "$GENTS" pack install "$WEBP" --home "$H" --grant-authority "${WSLOTS[@]}"
  fi
  xo "$R/install-scenario.json" "$GENTS" pack install "$SCEN" --home "$H" "${SLOTS[@]}"
  wire=(pack wire gents/workflow --agent wf-live-caller --home "$H")
  while read -r p; do wire+=(--targets-from "$p"); done < <(jq -r '.wire_from[]' "$row")
  xo "$R/wire.json" "$GENTS" "${wire[@]}"
  if [ "$DRY" = 1 ]; then
    echo "+ narrow_caller $H '$targets'"
    case "$N" in 12 | 14 | 22) ;; *) false ;; esac && echo "+ tools_edit wf-live-caller-tools --home $H $R/caller-tools-dynamic-run.json  # job.policy.dynamic = run"
    echo "+ ($GENTS server --home $H --http-port $PORT --p2p-transport none --no-codex-shim --tool-ceiling readonly --tool-root $W, from $W)"
    echo "+ $GENTS pack scenario seed $SCEN --http-port $PORT --job-id $RUNID-row-$N --prompt '<the row prompt>'"
    echo "legs: $(jq -r '.legs | join(", ")' "$row")"
    manual_checklist
    return 0
  fi
  narrow_caller "$H" "$targets"
  # A pack may not ship dynamic run; the operator sets it (row 12's wf-read-write request is still refused: dynamic_targets).
  if [ "$N" = 12 ] || [ "$N" = 14 ] || [ "$N" = 22 ]; then
    tools_edit wf-live-caller-tools --home "$H" "$R/caller-tools-dynamic-run.json" '(.integrations.plugins[] | select(.plugin == "gents/workflow") | .job.policy.dynamic) = "run"'
    # Row 14's kept script runs as dynamic on the read-only worker the row keeps, which narrowing left out of dynamic_targets.
    if [ "$N" = 14 ]; then
      tools_edit wf-live-caller-tools --home "$H" "$R/caller-tools-dynamic-targets.json" '(.integrations.plugins[] | select(.plugin == "gents/workflow") | .job.policy.dynamic_targets) = ["wf-live-worker"]'
    fi
  fi
  if [ "$N" = 11 ]; then
    # Test-only: the worker persona gets the tool, and its notes ask it to use it.
    tools_edit wf-live-worker-tools --home "$H" "$R/worker-tools-test-only.json" '.integrations = {plugins: [{plugin: "gents/workflow"}]}'
    printf '%s\n' 'Before you answer, call the workflow tool now with argv ["run","wf-live-run"]; it is in your tool list. Do not answer until you have called it, then quote the error code it returned.' >"$W/notes.txt"
  fi
  trap stop_server EXIT
  start_server
  "$GENTS" pack scenario seed "$SCEN" --http-port "$PORT" --job-id "$RUNID-row-$N" --prompt "$PROMPT" >"$R/seed.txt"
  # Rows 12, 14 and 15 end inline (no job row); row 13 and a corpus leg may too, so their legs wait for the row themselves.
  case "$N" in 12 | 13 | 14 | 15 | corpus-*) ;; *) until_ok "$TIMEOUT" first_address || fail "no job row appeared" ;; esac
  legs
  client_legs
  manual_checklist
  echo "pass: row $N backend $B"
}

# corpus_legs <backend>: every *.rhai file found in GROK_CORPUS_DIR at run time; this script names none.
corpus_legs() {
  local i=0 f rc
  if [ -z "${GROK_CORPUS_DIR:-}" ] || [ ! -d "$GROK_CORPUS_DIR" ]; then
    echo "corpus legs: skipped, GROK_CORPUS_DIR is not a folder"
    return 0
  fi
  for f in "$GROK_CORPUS_DIR"/*.rhai; do
    [ -f "$f" ] || continue
    i=$((i + 1))
    set +e
    (run_row "$1" "corpus-$i" "$f")
    rc=$?
    set -e
    [ "$rc" -eq 0 ] || fails=$((fails + 1))
  done
}

fails=0
for b in $BACKENDS; do
  if [ "$DRY" != 1 ]; then
    ep="LIVE_ENDPOINT_$b" md="LIVE_MODEL_$b"
    [ -n "${!ep:-}" ] && [ -n "${!md:-}" ] || { echo "set $ep and $md" >&2; exit 2; }
    export GENTS_LIVE_ENDPOINT="${!ep}" GENTS_LIVE_MODEL="${!md}"
  fi
  for n in $ROW_LIST; do
    set +e
    (run_row "$b" "$n")
    rc=$?
    set -e
    [ "$rc" -eq 0 ] || fails=$((fails + 1))
  done
  [ "$CORPUS" = 0 ] || corpus_legs "$b"
done
[ "$fails" -eq 0 ]
