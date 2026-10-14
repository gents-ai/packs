#!/usr/bin/env bash
# Parity driver for the review-changes workflow against the code_review graph.
#
#   workflow-parity.sh fixture <dir>
#       Builds a throwaway git repo at <dir>. Base B has src/lib.rs and
#       src/other.rs; head H puts an off-by-one at src/lib.rs:12.
#       Prints "B=<sha> H=<sha>".
#   workflow-parity.sh --mid-run-edit --home <h> --graphql <url>
#       Starts `workflow run review-changes` over the fixture through an agent,
#       waits until phase Scan is active, then fixes src/lib.rs:12 uncommitted
#       and commits H2 with a bug in src/other.rs. The result must still be about
#       H: head_sha == H, a confirmed finding at src/lib.rs:12, no mention of
#       src/other.rs, every evidence excerpt present in `git show H:<path>`.
#       The job is addressed only by its session_id:tool_call_id address. The
#       cleanup stop is issued in-session (`workflow stop <address>`) from an
#       EXIT trap, so it also runs when an assertion fails; never `gents job
#       stop`, because control is local-only in v1.
#   workflow-parity.sh --ab [--home <h> --graphql <url>]
#       Runs graph code_review and the workflow on the fixture on one backend
#       and prints both confirmed counts and locations as JSON.
#   --dry-run
#       Combinable with any mode, or alone for every mode. Prints each command,
#       touches no network and no home, and exits 0.
#
# GENTS overrides the gents binary (default: gents on PATH).
set -euo pipefail

GENTS="${GENTS:-gents}"
DRY=0
MODES=()
FIXTURE_DIR=""
HOME_DIR=""
GRAPHQL=""
POLL_TRIES="${PARITY_POLL_TRIES:-120}"

die() { echo "workflow-parity: $*" >&2; exit 2; }
dry() { [ "$DRY" = 1 ]; }

# show prints a command: on stdout for a dry run, on stderr as a trace otherwise,
# so stdout stays free for fixture's "B=... H=..." line.
show() {
  if dry; then printf '+'; printf ' %q' "$@"; printf '\n'
  else { printf '+'; printf ' %q' "$@"; printf '\n'; } >&2
  fi
}
run() { show "$@"; dry || "$@"; }
# cap VAR PLACEHOLDER cmd...: VAR gets the command's stdout, or PLACEHOLDER on a dry run.
cap() {
  local __var=$1 __ph=$2 __out
  shift 2
  show "$@"
  if dry; then printf -v "$__var" '%s' "$__ph"; else __out="$("$@")" || return; printf -v "$__var" '%s' "$__out"; fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    fixture) MODES+=(fixture); FIXTURE_DIR="${2:?fixture needs a directory}"; shift ;;
    --mid-run-edit) MODES+=(mid) ;;
    --ab) MODES+=(ab) ;;
    --dry-run) DRY=1 ;;
    --home) HOME_DIR="${2:?--home needs a value}"; shift ;;
    --graphql) GRAPHQL="${2:?--graphql needs a value}"; shift ;;
    *) die "unknown argument: $1" ;;
  esac
  shift
done
if [ ${#MODES[@]} -eq 0 ]; then
  dry || die "usage: workflow-parity.sh fixture <dir> | --mid-run-edit --home <h> --graphql <url> | --ab | --dry-run"
  MODES=(fixture mid ab)
fi
if dry; then
  : "${FIXTURE_DIR:=<fixture-dir>}" "${HOME_DIR:=<home>}" "${GRAPHQL:=<graphql-url>}"
fi

scope_args() { SCOPE=(); [ -z "$HOME_DIR" ] || SCOPE+=(--home "$HOME_DIR"); [ -z "$GRAPHQL" ] || SCOPE+=(--graphql "$GRAPHQL"); }

# write_lib <range operator>: a 14 line file whose line 12 is the loop header.
write_lib() {
  local op=$1 dir=$2
  {
    printf '%s\n' '/// Sums the values.' 'pub fn sum(values: &[i32]) -> i32 {' '    let mut total = 0;' '    // Walk every element exactly once.' '    // The loop bound is the length of the slice.' '    // Nothing else reads the slice.' '    // Overflow is out of scope here.' '    // The accumulator starts at zero.' '    // Each pass adds one element.' '    // The index is only used to read.' '    // The result is returned below.'
    printf '    for i in 0%s values.len() {\n' "$op"
    printf '%s\n' '        total += values[i];' '    }' '    total' '}'
  } > "$dir/src/lib.rs"
}
write_other() {
  if [ "$1" = bug ]; then
    printf '%s\n' '/// Halves the value.' 'pub fn half(value: i32) -> i32 {' '    value / 3' '}' > "$2/src/other.rs"
  else
    printf '%s\n' '/// Halves the value.' 'pub fn half(value: i32) -> i32 {' '    value / 2' '}' > "$2/src/other.rs"
  fi
}
fixture_base() { mkdir -p "$1/src"; write_lib ".." "$1"; write_other ok "$1"; }
fixture_head() { write_lib "..=" "$1"; }
fixture_h2() { write_other bug "$1"; }
fixture_fix() { write_lib ".." "$1"; }

GIT_ID=(-c user.name=parity -c user.email=parity@example.invalid)

# fixture <dir>: sets B and H.
fixture() {
  local dir=$1
  run git init -q "$dir"
  run fixture_base "$dir"
  run git -C "$dir" add src
  run git -C "${dir}" "${GIT_ID[@]}" commit -q -m "base"
  cap B "<base-sha>" git -C "$dir" rev-parse HEAD
  run fixture_head "$dir"
  run git -C "$dir" add src
  run git -C "${dir}" "${GIT_ID[@]}" commit -q -m "off-by-one in sum"
  cap H "<head-sha>" git -C "$dir" rev-parse HEAD
  echo "B=$B H=$H"
}

# ask_agent <session> <content> [extra submit args]: one request through an agent,
# with the fixture pinned as the request cwd (only the request input carries it).
ask_agent() {
  local session=$1 content=$2 input
  shift 2
  scope_args
  input="$(jq -nc --arg cwd "$FIXTURE_DIR" '{cwd: $cwd}')"
  run "$GENTS" request submit "${SCOPE[@]+"${SCOPE[@]}"}" --session-id "$session" --input "$input" --content "$content" "$@"
}

# start_workflow: sets SESSION and KEY (the JobView address) for review-changes over the fixture.
start_workflow() {
  local args jobs
  SESSION="parity-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  args="{\"repository\":\".\",\"base\":\"$B\",\"head\":\"HEAD\"}"
  ask_agent "$SESSION" "Run the workflow tool with: run review-changes $args" --no-wait
  scope_args
  KEY=""
  for _ in $(seq "$POLL_TRIES"); do
    cap jobs '[]' "$GENTS" job list --session "$SESSION" --output json "${SCOPE[@]+"${SCOPE[@]}"}"
    dry && { KEY="<session_id:tool_call_id>"; break; }
    KEY="$(jq -r 'first(.. | objects | .address? | select(type == "string")) // empty' <<<"$jobs")"
    [ -z "$KEY" ] || break
    sleep 1
  done
  [ -n "$KEY" ] || die "no job address for session $SESSION"
}

# stop_job: the in-session cleanup stop; `gents job stop` is refused (control is local-only).
stop_job() { ask_agent "$SESSION" "Run the workflow tool with: stop $KEY" || true; }

# wait_scan_active: polls the job (by address) until phase Scan is active.
wait_scan_active() {
  local view
  scope_args
  for _ in $(seq "$POLL_TRIES"); do
    cap view '{}' "$GENTS" job show "$KEY" --output json "${SCOPE[@]+"${SCOPE[@]}"}"
    dry && return 0
    jq -e '[.. | objects | select(.title? == "Scan")] | any(.state? == "active")' <<<"$view" >/dev/null && return 0
    sleep 1
  done
  die "phase Scan never became active for $KEY"
}

# finish_workflow <out-file>: waits for the terminal state and leaves the JSON value the agent
# replied for `workflow result` in out-file (the request output file wraps it in an envelope),
# and fails unless its status is complete (a blocked run is a completed job).
finish_workflow() {
  local out=$1 watch=()
  [ -z "$GRAPHQL" ] || watch=(--graphql "$GRAPHQL")
  run "$GENTS" job watch "$KEY" --until-terminal "${watch[@]+"${watch[@]}"}"
  ask_agent "$SESSION" "Run the workflow tool with: result $KEY and reply with only its JSON value" --output-file "$out"
  dry && return 0
  jq '.output.presentation.body_markdown | fromjson' "$out" >"$out.value" || die "no JSON result in the agent reply ($out)"
  mv "$out.value" "$out"
  jq -e '.status == "complete"' "$out" >/dev/null || die "workflow result is not complete ($out)"
}

# SUMMARY: the findings of a result document, as {confirmed, locations}; fails without .findings.
SUMMARY='{confirmed: ([.findings[]] | length), locations: [.findings[] | "\(.path):\(.line)"]}'

mid_run_edit() {
  local out check
  [ -n "$HOME_DIR" ] && [ -n "$GRAPHQL" ] || die "--mid-run-edit needs --home and --graphql"
  dry || FIXTURE_DIR="$(mktemp -d)"
  dry && FIXTURE_DIR="<fixture-dir>"
  fixture "$FIXTURE_DIR"
  start_workflow
  trap stop_job EXIT
  wait_scan_active
  # Mid-run: fix lib.rs:12 in the working tree only, then commit H2 with a bug in other.rs.
  run fixture_fix "$FIXTURE_DIR"
  run fixture_h2 "$FIXTURE_DIR"
  run git -C "$FIXTURE_DIR" add src/other.rs
  run git -C "$FIXTURE_DIR" "${GIT_ID[@]}" commit -q -m "bug in other"
  out="${FIXTURE_DIR}.result.json"
  finish_workflow "$out"
  if dry; then
    show jq -e ".head_sha == \"$H\"" "$out"
    show jq -e '[.findings[]? | select(.path == "src/lib.rs" and (.line | tostring) == "12")] | length > 0' "$out"
    show grep -q src/other.rs "$out"
    show git -C "$FIXTURE_DIR" show "$H:<path>"
    return 0
  fi
  jq -e --arg h "$H" '.head_sha == $h' "$out" >/dev/null || die "head_sha is not $H"
  jq -e '[.findings[]? | select(.path == "src/lib.rs" and (.line | tostring) == "12")] | length > 0' "$out" >/dev/null || die "no finding at src/lib.rs:12"
  ! grep -q 'src/other.rs' "$out" || die "result mentions src/other.rs"
  jq -j '.findings[]? | .path, "\u0000", (.evidence // ""), "\u0000"' "$out" | while IFS= read -r -d '' path && IFS= read -r -d '' excerpt; do
    check="$(git -C "$FIXTURE_DIR" show "$H:$path")"
    case "$check" in *"$excerpt"*) ;; *) die "evidence for $path is not in git show $H:$path" ;; esac
  done
  echo "mid-run-edit: ok ($KEY)"
}

ab() {
  local graph_out wf_out run_id="<run-id>" cmd
  dry || FIXTURE_DIR="$(mktemp -d)"
  dry && FIXTURE_DIR="<fixture-dir>"
  fixture "$FIXTURE_DIR"
  scope_args
  cmd=("$GENTS" graph run code_review --field repository="$FIXTURE_DIR" --field base="$B" --field head="$H" --output json "${SCOPE[@]+"${SCOPE[@]}"}")
  show "${cmd[@]}"
  dry || run_id="$("${cmd[@]}" | jq -er .run_id)"
  # The watch stream is progress only; the exit status says whether the run succeeded.
  cmd=("$GENTS" graph watch "$run_id" --output json "${SCOPE[@]+"${SCOPE[@]}"}")
  show "${cmd[@]}"
  dry || "${cmd[@]}" >/dev/null
  cmd=("$GENTS" graph result "$run_id" --output json "${SCOPE[@]+"${SCOPE[@]}"}")
  show "${cmd[@]}"
  dry || graph_out="$("${cmd[@]}" | jq '{findings: [.results[] | select(.name == "findings") | .documents[] | select(.verdict == "confirmed")]}')"
  start_workflow
  wf_out="${FIXTURE_DIR}.result.json"
  finish_workflow "$wf_out"
  if dry; then
    show jq -n --slurpfile workflow "$wf_out" "{graph: (<graph output> | $SUMMARY), workflow: (\$workflow[0] | $SUMMARY)}"
    return 0
  fi
  jq -n --argjson graph "$graph_out" --slurpfile workflow "$wf_out" \
    "{graph: (\$graph | $SUMMARY), workflow: (\$workflow[0] | $SUMMARY)}"
}

for mode in "${MODES[@]}"; do
  case "$mode" in
    fixture)
      fixture "$FIXTURE_DIR" ;;
    mid) mid_run_edit ;;
    ab) ab ;;
  esac
done
