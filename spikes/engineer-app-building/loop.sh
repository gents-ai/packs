#!/bin/bash
# The build loop, with the node left running and no client attached.
#   loop.sh RUN_ID a|b PACK_DIR|none [MAX_ROUNDS] [BINARY]
# Sends the brief as a non-interactive request, waits for the node to settle,
# grades, and sends failing checks back to the same session until the build
# works or MAX_ROUNDS is reached. Records one row. Stops only its own server.
set -u
RUN_ID=$1; HOST=$2; PACK=$3; MAX_ROUNDS=${4:-5}; GENTS=${5:-gents}
ROOT=~/.gents-spikes; RESULTS=$ROOT/results; DIR=$ROOT/runs/$RUN_ID; H=$DIR/home
ROUND_TIMEOUT=14400; RUNAWAY_SESSIONS=300
case $HOST in
  a) URL=${INFERENCE_URL_A:?set INFERENCE_URL_A to an OpenAI-compatible /v1 endpoint} ;;
  b) URL=${INFERENCE_URL_B:?set INFERENCE_URL_B to an OpenAI-compatible /v1 endpoint} ;;
  *) echo "host must be a or b"; exit 2 ;;
esac
MODEL=$(curl -s -m 8 "$URL/models" | python3 -I -c "import sys,json; print(json.load(sys.stdin)['data'][0]['id'])") || { echo "no model at $URL"; exit 2; }
[ -e "$DIR" ] && { echo "$DIR exists; use a new run id"; exit 2; }
mkdir -p "$DIR/workspace"; cp "$RESULTS/spec/SPEC.md" "$DIR/workspace/"
PORT=$(python3 -I -c "
import socket
for p in range(9800,9900):
    s=socket.socket()
    try: s.bind(('127.0.0.1',p)); s.close(); print(p); break
    except OSError: pass")

"$GENTS" init --home "$H" --node-name "$RUN_ID" --inference-url "$URL" --backend-preset vllm \
  --openai-wire-api chat-completions --model-name "$MODEL" --max-concurrent "${MAX_CONCURRENT:-16}" --engineer --tool-root "$DIR/workspace" > "$DIR/init.log" 2>&1
# Prompt, skills and shell are set directly, before the runtime starts, so no model turn is spent on setup.
SKILLS=none; SETUP=()
if [ "$PACK" != none ]; then
  "$GENTS" pack install "$PACK" --home "$H" > "$DIR/pack_install.log" 2>&1
  SKILLS="$(basename "$PACK")@$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]+'/manifest.json'))['version'])" "$PACK")"
  SETUP+=(--skills-pack "$PACK")
fi
PROMPT_NAME=builtin
[ -n "${PROMPT:-}" ] && { SETUP+=(--prompt "$PROMPT"); PROMPT_NAME=$(basename "$PROMPT" .md); }
[ -n "${NO_SHELL:-}" ] && SETUP+=(--no-shell)
"$GENTS" config export --root "$DIR/export" --home "$H" > "$DIR/setup.log" 2>&1
python3 -I "$RESULTS/setup_node.py" "$DIR/export" "${SETUP[@]}" | tee -a "$DIR/status.txt"
"$GENTS" config apply --root "$DIR/export" --home "$H" >> "$DIR/setup.log" 2>&1 || { echo "config apply failed"; exit 2; }
cd "$DIR/workspace"
"$GENTS" server --home "$H" --http-port "$PORT" --no-codex-shim --no-keychain-prompts > "$DIR/server.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null' EXIT
for _ in $(seq 1 60); do
  curl -s -m 2 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/v0/graphql" -d '{"query":"{__typename}"}' \
    -H 'content-type: application/json' | grep -q 200 && break
  sleep 2
done

submit() {  # message-file [session]; prints session_id
  local args=(request submit --home "$H" --content-file "$1" --valid-until none --no-wait)
  [ -n "${2:-}" ] && args+=(--session-id "$2")
  "$GENTS" "${args[@]}" 2>>"$DIR/submit.err" | python3 -I -c "import sys,json; print(json.load(sys.stdin)['session_id'])"
}

open_requests() {
  "$GENTS" query find --home "$H" --collection AgentRequest --field lifecycle_state \
    --order '[{"created_at":"DESC"}]' --limit 200 2>/dev/null | python3 -I -c "
import sys,json
done={'completed','failed','interrupted','superseded','dead'}
print(sum(1 for r in json.load(sys.stdin)['results'] if r['lifecycle_state'] not in done))" 2>/dev/null || echo 1
}

sessions() {
  "$GENTS" query count --home "$H" --collection AgentSession 2>/dev/null \
    | python3 -I -c "import sys,json; print(json.load(sys.stdin)['total_count'])" 2>/dev/null || echo 0
}

RUNAWAY=
settle() {  # waits until nothing is open for three checks in a row
  local start quiet=0
  start=$(date +%s)
  while [ $(( $(date +%s) - start )) -lt $ROUND_TIMEOUT ]; do
    if [ "$(sessions)" -gt $RUNAWAY_SESSIONS ]; then RUNAWAY=1; return 1; fi
    if [ "$(open_requests)" = 0 ]; then quiet=$((quiet + 1)); else quiet=0; fi
    [ $quiet -ge 3 ] && return 0
    sleep 20
  done
  return 1
}

MESSAGE=$RESULTS/spec/brief.md; SESSION=; STATUS=not_working; FAILED=; ROUND=0
while [ $ROUND -lt "$MAX_ROUNDS" ]; do
  ROUND=$((ROUND + 1))
  SESSION=$(submit "$MESSAGE" "$SESSION")
  echo "round $ROUND session $SESSION $(date +%H:%M:%S)" | tee -a "$DIR/status.txt"
  settle
  [ -n "$RUNAWAY" ] && { FAILED="runaway: over $RUNAWAY_SESSIONS sessions"; break; }
  python3 -I "$RESULTS/check_game.py" "$H" > "$DIR/grade.$ROUND.json" 2>"$DIR/grade.$ROUND.err"
  python3 -I "$RESULTS/feedback.py" "$DIR/grade.$ROUND.json" "$DIR/next.txt"
  RESULT=$(python3 -I -c "
import json,sys
g=json.load(open(sys.argv[1]))
print('working' if g['working'] else '; '.join([k for k,v in g['checks'].items() if not v]+g['bad']))" "$DIR/grade.$ROUND.json")
  echo "round $ROUND grade: $RESULT" | tee -a "$DIR/status.txt"
  if [ "$RESULT" = working ]; then STATUS=working; FAILED=; break; fi
  FAILED=$RESULT
  MESSAGE=$DIR/next.txt
done

python3 -I "$RESULTS/record_run.py" --home "$H" --session "$SESSION" --node-counts --run-id "$RUN_ID" \
  --task game-build --model "$MODEL" --skills "$SKILLS" --binary "$("$GENTS" version | head -1 | cut -c1-40)" \
  --status "$STATUS" --bad "${FAILED:-}" --note "rounds $ROUND; prompt $PROMPT_NAME; shell ${NO_SHELL:+off}" | tail -1
