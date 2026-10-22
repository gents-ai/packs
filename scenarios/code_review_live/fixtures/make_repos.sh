#!/bin/sh
# Builds review_seeded_bug (head: off-by-one at src/lib.rs:12) and
# review_clean_refactor (head: div_ceil, same behaviour) next to this script,
# each with a base and a head commit.
set -eu
cd "$(dirname "$0")"
build() {
  r="review_$1"
  rm -rf "$r"; mkdir -p "$r/src"; cp base/src/lib.rs "$r/src/"
  git -C "$r" init -q; git -C "$r" add -A
  git -C "$r" -c user.name=fixture -c user.email=fixture@example.invalid commit -qm base
  sed "$2" base/src/lib.rs > "$r/src/lib.rs"
  git -C "$r" -c user.name=fixture -c user.email=fixture@example.invalid commit -qam head
}
build seeded_bug 's/per_page)\.min/per_page + 1).min/'
build clean_refactor 's|(total + per_page - 1) / per_page|total.div_ceil(per_page)|'
