#!/bin/sh
# Builds probe_repo next to this script: a base commit holding base/src/lib.rs,
# then a head commit that rewrites page_count with div_ceil (same behaviour).
set -eu
cd "$(dirname "$0")"
rm -rf probe_repo; mkdir -p probe_repo/src; cp base/src/lib.rs probe_repo/src/
git -C probe_repo init -q; git -C probe_repo add -A
git -C probe_repo -c user.name=fixture -c user.email=fixture@example.invalid commit -qm base
sed 's|(total + per_page - 1) / per_page|total.div_ceil(per_page)|' base/src/lib.rs > probe_repo/src/lib.rs
git -C probe_repo -c user.name=fixture -c user.email=fixture@example.invalid commit -qam head
