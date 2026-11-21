#!/bin/bash
# run_main.sh RUN_ID a|b shell|noshell [ROUNDS]  (main-port binary, prompt v3 built in, skills v0.5)
P=~/go/src/github.com/gents-ai/gents-spike-main-port
export PATH=~/.gents-spikes/bin/mainport-c:$PATH GENTS_UX_PREVIEW_ASSETS=$HOME/.gents-spikes/assets/ux-preview-c MAX_CONCURRENT=16
[ "$3" = noshell ] && export NO_SHELL=1
exec ~/.gents-spikes/results/loop_v2.sh "$1" "$2" ~/.gents-spikes/results/packs/v0.5/pack_builder "${4:-4}"
