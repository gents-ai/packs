#!/usr/bin/env bash
# Measures the built plugin through the gents runtime: text PDF pages per
# second, EPUB throughput and OCR seconds per page, with the peak memory of the
# gents process. Works on Linux and macOS.
#
# Usage: tools/bench.sh <gents binary> [pack dir]     (pack dir defaults to ../..)
# Needs jq, perl, python3 and a Rust toolchain with wasm32-wasip1 (the pack is built on install).
set -euo pipefail

gents="$1"
pack="$(cd "${2:-$(dirname "$0")/../../..}" && pwd)"
plugin="$pack/plugins/ocr"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$plugin" && cargo run --quiet --release --example gen_fixtures -- "$work/fx" --bench >/dev/null)
"$gents" init --home "$work/home" >/dev/null
"$gents" pack install "$pack" --home "$work/home" >/dev/null

# Peak resident memory of the whole gents process, from the kernel's own
# accounting through tools/peakrss.py (the same on Linux and macOS).
tools="$(cd "$(dirname "$0")" && pwd)"
peak() { printf '%s MiB' "$(cat "$work/peak.txt")"; }

now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

run() { # dir input
  local dir="$1" input="$2" start end
  start="$(now)"
  python3 "$tools/peakrss.py" "$work/peak.txt" "$gents" plugin run ocr --home "$work/home" --bind-dir "$dir" --input "$input" >"$work/out.json"
  end="$(now)"
  printf '  %s s, peak %s, %s\n' "$(awk -v a="$end" -v b="$start" 'BEGIN { printf "%.2f", a - b }')" "$(peak)" "$(jq -r '(.response // .).documents | map("\(.pages) page(s), \(.markdown | length) chars") | join("; ")' "$work/out.json")"
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
