#!/bin/bash
# verify.sh HOME SESSION OUT.json [start-server PORT]
# Adds rows to the file and asks for an import, as a user would, then grades the result.
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
W=$(dirname "$H")/workspace
# Add the new rows once, and close one ticket, as the file would change in use.
python3 -I -c "
import sys,time
p,added=sys.argv[1],sys.argv[2]
s=open(p).read()
if 'T-1013' not in s:
    s=s.rstrip('\n')+'\n'+open(added).read()
    s=s.replace('2026-11-08T09:00:00Z,open','2026-11-08T09:00:00Z,closed')
else:
    n=int(time.time())%100000
    rows=[('C-01','Harbor Bakery','Receipt printer jams','The receipt printer jams on every third receipt since this morning.'),
          ('C-05','Marrow Kitchen','Cannot log in at all','Nobody on staff can log in and the restaurant opens in an hour.'),
          ('C-02','Lumen Dental','Wording on reminder text','Could the reminder text say clinic instead of office?')]
    s=s.rstrip('\n')+'\n'+''.join(f'T-{n}{i},{c},{name},{subj},\"{body}\",2026-11-10T06:0{i}:00Z,open\n' for i,(c,name,subj,body) in enumerate(rows))
open(p+'.new','w').write(s)
import os; os.replace(p+'.new',p)" "$W/tickets.csv" "$R/spec-helpdesk/added.csv"
SINCE=$(date -u +%Y-%m-%dT%H:%M:%SZ)
MSG=$(dirname "$OUT")/verify_msg.txt
echo "I added three tickets to tickets.csv. Import them." > "$MSG"
gents request submit --home "$H" --content-file "$MSG" --session-id "$SESSION" --valid-until none --no-wait >> "$OUT.log" 2>&1
quiet=0; start=$(date +%s)
while [ $(( $(date +%s) - start )) -lt 2400 ]; do
  if [ "$(open_requests)" = 0 ]; then quiet=$((quiet + 1)); else quiet=0; fi
  [ $quiet -ge 3 ] && break
  sleep 20
done
python3 -I "$R/check_helpdesk.py" "$H" "$SINCE" 3 > "$OUT" 2>"$OUT.err"
[ -n "$SERVER" ] && kill "$SERVER"
python3 -I -c "
import json,sys
g=json.load(open(sys.argv[1])); print('working' if g['working'] else '; '.join([k for k,v in g['checks'].items() if not v]+g['bad']), '| fires',g['fires'],'player sessions',g['player_sessions'])" "$OUT"
