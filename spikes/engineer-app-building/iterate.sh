#!/bin/bash
# One iteration of the build eval: fresh node, build the game, grade it, record it.
#   iterate.sh RUN_ID a|b PACK_DIR|none [BINARY]
# Prints a results row at the end. Stops only the processes it started.
set -u
RUN_ID=$1; HOST=$2; PACK=$3; GENTS=${4:-gents}
ROOT=~/.gents-spikes; RESULTS=$ROOT/results; DIR=$ROOT/runs/$RUN_ID
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
HOME_DIR=$DIR/home
"$GENTS" init --home "$HOME_DIR" --node-name "$RUN_ID" --inference-url "$URL" --backend-preset vllm \
  --openai-wire-api chat-completions --model-name "$MODEL" --engineer --tool-root "$DIR/workspace" > "$DIR/init.log" 2>&1
cd "$DIR/workspace"
"$GENTS" server --home "$HOME_DIR" --http-port "$PORT" --no-codex-shim --no-keychain-prompts > "$DIR/server.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null' EXIT
for _ in $(seq 1 60); do
  curl -s -m 2 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT/api/v0/graphql" -d '{"query":"{__typename}"}' \
    -H 'content-type: application/json' | grep -q 200 && break
  sleep 2
done

SKILLS=none
if [ "$PACK" != none ]; then
  "$GENTS" pack install "$PACK" --home "$HOME_DIR" > "$DIR/pack_install.log" 2>&1
  IDS=$(python3 -I -c "import json,sys; print(', '.join(s['skill_id'] for s in json.load(open(sys.argv[1]+'/pack_config.json'))['skills']))" "$PACK")
  VERSION=$(python3 -I -c "import json,sys; print(json.load(open(sys.argv[1]+'/manifest.json'))['version'])" "$PACK")
  SKILLS="pack_builder@$VERSION"
  "$GENTS" chat --home "$HOME_DIR" --timeout-secs 300 "Use the config tool to set skill_ids on your own Context to exactly these IDs: $IDS. Change nothing else. Do not do any other work. Reply with one line when it is saved." > "$DIR/attach.log" 2>&1
fi

START=$(date +%s)
"$GENTS" chat --home "$HOME_DIR" --message-file "$RESULTS/spec/brief.md" --output-file "$DIR/final_reply.txt" --timeout-secs 43200 > "$DIR/build.log" 2>&1
CHAT_EXIT=$?
echo "chat exit $CHAT_EXIT after $(( ($(date +%s) - START) / 60 )) min" | tee "$DIR/status.txt"

# Give a started game a moment to finish its one round before grading.
for _ in $(seq 1 40); do
  n=$("$GENTS" query count --home "$HOME_DIR" --collection IsleEvent --filter '{"kind":{"_eq":"win"}}' 2>/dev/null \
      | python3 -I -c "import sys,json; print(json.load(sys.stdin)['total_count'])" 2>/dev/null || echo 0)
  [ "${n:-0}" -gt 0 ] && break
  sleep 15
done

python3 -I "$RESULTS/check_game.py" "$HOME_DIR" > "$DIR/grade.json" 2>"$DIR/grade.err"
cat "$DIR/grade.json"
STATUS=$(python3 -I -c "import json,sys; print('working' if json.load(open(sys.argv[1]))['working'] else 'not_working')" "$DIR/grade.json" 2>/dev/null || echo ungraded)
FAILED=$(python3 -I -c "
import json,sys
d=json.load(open(sys.argv[1]))
print('; '.join([k for k,v in d['checks'].items() if not v]+d['bad']))" "$DIR/grade.json" 2>/dev/null)
SESSION=$("$GENTS" session list --home "$HOME_DIR" --output json 2>/dev/null | python3 -I -c "
import sys,json
d=[s for s in json.load(sys.stdin)['items'] if s['agent_id'].endswith(':default')]
d.sort(key=lambda s:s['created_at']); print(d[-1]['session_id'])")
python3 -I "$RESULTS/record_run.py" --home "$HOME_DIR" --session "$SESSION" --node-counts --run-id "$RUN_ID" \
  --task game-build --model "$MODEL" --skills "$SKILLS" --binary "$("$GENTS" version | head -1 | cut -c1-40)" \
  --status "$STATUS" --bad "${FAILED:-}" --note "chat exit $CHAT_EXIT"
