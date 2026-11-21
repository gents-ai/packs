#!/bin/bash
# verify.sh HOME SESSION OUT.json [start-server PORT]
# Asks the builder for one fresh game, as a user would, then grades only that game:
# its log, and the fires and player sessions created while it ran. Earlier attempts don't count.
set -u
H=$1; SESSION=$2; OUT=$3; R=~/.gents-spikes/results
SERVER=
if [ "${4:-}" = start-server ]; then
  ( cd "$(dirname "$H")/workspace" && exec gents server --home "$H" --http-port "$5" --no-codex-shim --no-keychain-prompts > "$(dirname "$H")/verify-server.log" 2>&1 ) &
  SERVER=$!
  for _ in $(seq 1 60); do
    curl -s -m 2 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$5/api/v0/graphql" -d '{"query":"{__typename}"}' -H 'content-type: application/json' | grep -q 200 && break
    sleep 2
  done
fi
open_requests() {
  gents query find --home "$H" --collection AgentRequest --field lifecycle_state --order '[{"created_at":"DESC"}]' --limit 200 2>/dev/null | python3 -I -c "
import sys,json
done={'completed','failed','interrupted','superseded','dead'}
print(sum(1 for r in json.load(sys.stdin)['results'] if r['lifecycle_state'] not in done))" 2>/dev/null || echo 1
}
KNOWN=$(dirname "$OUT")/known_games.json
gents query find --home "$H" --collection IsleGame --field game_id --limit 1000 2>>"$OUT.log" | python3 -I -c "
import sys,json
try: print(json.dumps([r['game_id'] for r in json.load(sys.stdin)['results']]))
except Exception: print('[]')" > "$KNOWN"
SINCE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
MSG=$(dirname "$OUT")/verify_msg.txt
echo "Start one new game now with max_rounds 1, so I can watch it." > "$MSG"
gents request submit --home "$H" --content-file "$MSG" --session-id "$SESSION" --valid-until none --no-wait >> "$OUT.log" 2>&1
quiet=0; start=$(date +%s)
while [ $(( $(date +%s) - start )) -lt 2400 ]; do
  if [ "$(open_requests)" = 0 ]; then quiet=$((quiet + 1)); else quiet=0; fi
  [ $quiet -ge 3 ] && break
  sleep 20
done
python3 -I "$R/check_game.py" "$H" "$SINCE" "$KNOWN" > "$OUT" 2>"$OUT.err"
[ -n "$SERVER" ] && kill "$SERVER"
python3 -I -c "
import json,sys
g=json.load(open(sys.argv[1])); print('working' if g['working'] else '; '.join([k for k,v in g['checks'].items() if not v]+g['bad']), '| fires',g['fires'],'player sessions',g['player_sessions'])" "$OUT"
