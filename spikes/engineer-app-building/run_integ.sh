#!/bin/bash
# run_integ.sh RUN_ID a|b shell|noshell game|helpdesk [ROUNDS]  (integrated branch binary, skills v0.6)
export PATH=~/.gents-spikes/bin/integ-a:$PATH GENTS_UX_PREVIEW_ASSETS=$HOME/.gents-spikes/assets/ux-preview-d MAX_CONCURRENT=16 TASK=$4
[ "$3" = noshell ] && export NO_SHELL=1
exec ~/.gents-spikes/results/loop_v3.sh "$1" "$2" ~/.gents-spikes/results/packs/v0.6/pack_builder "${5:-3}"
