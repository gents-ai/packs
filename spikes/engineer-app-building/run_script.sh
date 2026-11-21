#!/bin/bash
# run_script.sh RUN_ID a|b shell|noshell game|helpdesk [ROUNDS]  (1dc441aa2a plus the script tool, skills v0.6, gents/script given to the Engineer)
export PATH=~/.gents-spikes/bin/script-a:$PATH GENTS_UX_PREVIEW_ASSETS=$HOME/.gents-spikes/assets/ux-preview-d MAX_CONCURRENT=16 TASK=$4
export EXTRA_PACK=$HOME/.gents-spikes/packs-built/gents.script.pack EXTRA_PLUGIN=gents/script
[ "$3" = noshell ] && export NO_SHELL=1
exec ~/.gents-spikes/results/loop_v3.sh "$1" "$2" ~/.gents-spikes/results/packs/v0.6/pack_builder "${5:-3}"
