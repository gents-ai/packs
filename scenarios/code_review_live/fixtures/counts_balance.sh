#!/bin/sh
# usage: counts_balance.sh LABEL CANDIDATE CONFIRMED REFUTED UNVERIFIED
set -eu
if [ "$2" -eq $(($3 + $4 + $5)) ]; then
  echo "$1 counts balance"
else
  echo "$1 counts do not balance: candidate=$2 confirmed=$3 refuted=$4 unverified=$5" >&2
  exit 1
fi
