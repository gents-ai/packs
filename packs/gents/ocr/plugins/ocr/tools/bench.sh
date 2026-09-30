#!/usr/bin/env bash
# Measures the built plugin through the gents runtime: text PDF pages per
# second, EPUB throughput and OCR seconds per page, with peak memory where the
# platform's time(1) reports it. Works on Linux and macOS.
#
# Usage: tools/bench.sh <gents binary> [pack dir]     (pack dir defaults to ../..)
# Needs jq, perl and a Rust toolchain with wasm32-wasip1 (the pack is built on install).
set -euo pipefail

gents="$1"
pack="$(cd "${2:-$(dirname "$0")/../../..}" && pwd)"
plugin="$pack/plugins/ocr"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$plugin" && cargo run --quiet --release --example gen_fixtures -- "$work/fx" --bench >/dev/null)
"$gents" init --home "$work/home" >/dev/null
"$gents" pack install "$pack" --home "$work/home" >/dev/null

# Peak resident memory of the whole gents process, when time(1) can tell.
if /usr/bin/time -l true >/dev/null 2>&1; then
  timer=(/usr/bin/time -l)
  peak() { awk '/maximum resident set size/ { printf "%.0f MiB", $1 / 1048576 }' "$1"; }
elif /usr/bin/time -v true >/dev/null 2>&1; then
  timer=(/usr/bin/time -v)
  peak() { awk -F': ' '/Maximum resident set size/ { printf "%.0f MiB", $2 / 1024 }' "$1"; }
else
  timer=()
  peak() { printf 'n/a'; }
fi

now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

run() { # dir input
  local dir="$1" input="$2" start end
  start="$(now)"
  if ((${#timer[@]})); then
    "${timer[@]}" "$gents" plugin run ocr --home "$work/home" --bind-dir "$dir" --input "$input" >"$work/out.json" 2>"$work/time.txt"
  else
    "$gents" plugin run ocr --home "$work/home" --bind-dir "$dir" --input "$input" >"$work/out.json" 2>"$work/time.txt"
  fi
  end="$(now)"
  printf '  %s s, peak %s, %s\n' "$(awk -v a="$end" -v b="$start" 'BEGIN { printf "%.2f", a - b }')" "$(peak "$work/time.txt")" "$(jq -r '(.response // .).documents | map("\(.pages) page(s), \(.markdown | length) chars") | join("; ")' "$work/out.json")"
}

mkdir -p "$work/empty" "$work/text" "$work/epub" "$work/scan" "$work/dense"
echo x >"$work/empty/a.txt"
cp "$work/fx/text-100p.pdf" "$work/text/"
cp "$work/fx/book-large.epub" "$work/epub/"
cp "$work/fx/scan-page.pdf" "$work/scan/"
cp "$work/fx/scan-dense.pdf" "$work/dense/"

echo "startup (one tiny file):"; run "$work/empty" '{}'
echo "100-page text PDF:";      run "$work/text" '{}'
echo "large EPUB:";             run "$work/epub" '{}'
echo "one scanned page, 4 lines (OCR):"; run "$work/scan" '{}'
echo "one dense scanned page, 52 lines (OCR):"; run "$work/dense" '{}'
