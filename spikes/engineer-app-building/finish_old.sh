#!/bin/bash
# finish_old.sh RUN MODEL SHELLNOTE : stop an old-loop run, re-score it on one fresh game, record it.
export PATH=~/.gents-spikes/bin/mainport-b:$PATH
r=$1; D=~/.gents-spikes/runs/$r; R=~/.gents-spikes/results
SID=$(grep -E '^round [0-9]+ session' $D/status.txt | tail -1 | awk '{print $4}')
for pid in $(pgrep -f "loop.sh $r "); do kill $pid; done
gents request interrupt --home $D/home >/dev/null 2>&1
sleep 5
"$R/verify.sh" $D/home "$SID" $D/verify.json > $D/verify.out 2>&1
G=$(tail -1 $D/verify.out)
for pid in $(pgrep -f "gents server --home $D/home"); do kill $pid; done
case "$G" in working*) S=working; B= ;; *) S=not_working; B=${G%% |*} ;; esac
cd $R && python3 -I record_run.py --home $D/home --session "$SID" --node-counts --run-id $r --task game-build --model "$2" --skills pack_builder@0.5.0 --binary main-port-b --status $S --bad "$B" --note "$3; re-scored on one fresh game" | tail -1
