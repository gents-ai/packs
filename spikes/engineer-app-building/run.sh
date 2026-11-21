#!/bin/bash
# run.sh RUN_ID a|b PACK_DIR PROMPT_FILE|builtin [ROUNDS]  (combined spike binary, no shell)
export PATH=${WORKTREES:?set WORKTREES to the directory holding gents worktrees}/gents-spike-combined/target/fast-dev:$PATH GENTS_UX_PREVIEW_ASSETS=${WORKTREES:?set WORKTREES to the directory holding gents worktrees}/gents-spike-combined/apps/gents-desktop/dist-ux-preview NO_SHELL=1
[ "$4" != builtin ] && export PROMPT=$4
exec ~/.gents-spikes/results/loop.sh "$1" "$2" "$3" "${5:-4}"
