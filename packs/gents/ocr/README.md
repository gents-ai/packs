# OCR pack

Reads the documents people actually have into Markdown a model can work with:
PDF, EPUB, DOCX, PPTX, XLSX, OpenDocument (ODT, ODS, ODP), HTML, Markdown,
CSV, plain text and PNG, JPEG, GIF, BMP, TIFF and WebP images. Scanned pages
and the text inside figures are read with a bundled OCR engine, and every
figure appears where it occurs in the text, with its caption and the words
found inside it. The pack ships one plugin, `ocr`; it is a WebAssembly
module, so it behaves the same on every operating system gents runs on.

## Installation

```sh
gents pack install ./packs/gents/ocr --home <home>
gents pack install gents/ocr --home <home>   # once published to the registry
```

The desktop app's Packs panel installs the same pack from the registry or the
store. Nothing in the pack needs configuring after that.

## Bindings and prerequisites

The pack declares no inference slots.

Building from a source checkout compiles the plugin to `wasm32-wasip1` with
the compiler `rust-toolchain.toml` pins, so it needs a Rust toolchain and
`rustup target add wasm32-wasip1`; a pack fetched pre-built (a `.pack`, the
home's store or the registry) skips the build.

The plugin reads files through a directory the caller binds for one call:

```sh
gents plugin run ocr --home <home> --bind-dir ~/Documents/reports --input '{"files": ["q3.pdf"]}'
```

`gents pack test` cases bind with their `bind` field, and a scenario's
`prepare` step can bind a directory too. A model that calls the plugin as a
tool has no bound directory, so it sends a file inline (`name` and
`data_base64`); reading a whole folder is an operator action.

## Authority

The plugin declares `bind_dir` (input field `path`) and no standing manifold
grant. A binding is read-only, names exactly one directory, exists for one
call and is never recorded as an install grant. The plugin has no network,
environment or write access, and cannot read outside the bound directory.
Its `limits` are 1536 MiB of memory, a 900 s wall clock and 4 MiB of output,
which is the host's own ceiling for plugin output.

## Inputs and outputs

One JSON object in. Name the source with `path` (the bound directory, or one
file inside it, optionally `files` to pick and order files) or with `name` and
`data_base64` (up to 64 MiB of base64).

| Field | Meaning |
| --- | --- |
| `pages` | pages, slides, sheets or EPUB sections to read, 1-based, like `"1-3,7"` or `"40-"` |
| `ocr` | `auto` (default): OCR only where there is no usable text layer; `always`; `never` |
| `figure_images` | attach each figure image for the model to look at (default `false`) |
| `max_image_px` | longest side OCR inputs and attached images are scaled down to, 256 to 4096 (default 2000) |
| `min_figure_px` | images with a shorter side below this are skipped as decorative (default 96) |
| `cursor` | the `next.cursor` of the previous call: continue exactly where it stopped, with every other field unchanged |
| `mode` | `read` (default) or `plan`: list what each file holds and the `pages` ranges that cover it, without reading it |
| `max_bytes`, `max_seconds` | the most JSON bytes (4096 to 3800000, default 3800000) and wall-clock seconds (1 to 860, default 860) of one call before it returns a cursor |

One JSON object out:

```json
{"documents": [{"source": "q3.pdf", "format": "pdf", "pages": 12,
  "markdown": "<!-- document: q3.pdf (pdf) -->\n\n<!-- page 1 -->\n\n# Q3 results ...",
  "figures": [{"id": "fig-1", "page": 2, "caption": "Figure 1. Revenue",
               "text": "North 42\nSouth 37", "width": 640, "height": 480, "ocr": true}],
  "warnings": []}]}
```

A call that stops before the end (the output limit, the wall clock, OCR time)
also returns `"next": {"cursor": "ocr1....", "source": "big.pdf"}`; calling
again with the same input and that `cursor` continues at the exact page, row or
line where it stopped, and a continuation document names how its text joins
the earlier piece in `joint` (`blank`, `line` or `none`). A file of any size is
read this way: it is streamed, never loaded whole. `mode: "plan"` returns, per
file, its unit (page, slide, sheet, section), its count and `chunks` of `pages`
that cover every unit once, or `read: "cursor"` for a single flow of text; see
`plugins/ocr/TOOL.md` for the schema.

Markers are `<!-- page N -->` (PDF), `<!-- slide N -->` (PPTX, ODP),
`<!-- sheet N: name -->` (XLSX, ODS) and `<!-- section N: file -->` (EPUB
chapters in spine order). A figure is one quote block in the text:

```markdown
> **[Figure fig-1]** Figure 1. Revenue by region (520x360 px)
>
> Text in figure:
> Revenue by region
> North 42 million
```

With `figure_images: true` and at least one image attached, the result becomes
`{"response": {"documents": [...]}, "parts": [{"type": "image", "data":
"<base64>", "mimeType": "image/jpeg"}]}` and each figure's `part` is its index
in `parts`. This is the exact shape gents turns into model-visible content: a
plugin offered to a behavior as a tool returns its output through
`ToolResultContent::from_tool_output`, which makes the text of `response` and
one image per entry of `parts` (checked in gents `message.rs`, `plugin/tool.rs`
and the loop's tool-result threading). A caller that reads the JSON directly,
such as `gents plugin run`, gets the same object.

## Completion and failure

A call that names one file either returns its document or fails: the plugin
exits non-zero and the reason is one sentence, for example `the PDF is
corrupt or truncated and cannot be read`, `unsupported format .xyz; supported
are ...`, `the EPUB is DRM-protected and cannot be read`, or `the PDF is
password-protected and cannot be opened without the password`. A call over a
directory or several `files` keeps going: a file that fails is listed with
`format` `unknown` and `failed: <reason>` in its `warnings`, and the call only
fails when every file did. An image file is OCR-bound: once the call's OCR
time budget is spent, each remaining image of a directory run is listed
unread with `not read: the OCR time budget of this call ran out; call again
with pages or files listing what remains`, so the call still returns what it
read.

Partial results are never silent. `warnings` names pages that could not be
read and why, pages whose OCR was skipped because the time budget ran out
(OCR stops starting new images, image files and PDF pages alike, once the next one might not finish inside the 900 s wall clock), glyphs without a Unicode
mapping, rotated text, charts that are not read, images skipped as
decorative, images over 50 megapixels that were skipped without being decoded
(also when a page that holds one is not rendered for OCR), figure images past
a 256 MiB per-page memory cap, an image already listed earlier in the
document and skipped, repeated headers and footers that were left out, and
any limit that applies to what was read. The output is capped below the host's
4 MiB ceiling; a call that hits it stops at a page, section, row or line
boundary (or, for one enormous line, at a character) and returns `next`, so
nothing is dropped and nothing is repeated when the call is continued.

## Validation

```sh
gents pack check ./packs/gents/ocr
gents pack test ./packs/gents/ocr
GENTS=gents scripts/test-pack.sh packs/gents/ocr
cargo test --manifest-path packs/gents/ocr/plugins/ocr/Cargo.toml
```

## Operational history

1.0.0: first release. PDF, EPUB, Office, OpenDocument, HTML, text, CSV and
image input; OCR with the ocrs text detection and recognition models.

## Formats

| Format | Detected by | Unit | Read |
| --- | --- | --- | --- |
| PDF | `%PDF-` | page | text layer in reading order (columns, with a title or abstract across them read first; headings by font size; lists; tables found from cells that line up in rows and columns, also with narrow gaps between the cells; hyphenation joined, ligatures expanded, running headers and footers left out), raster images as figures placed in their own column, scanned pages by OCR; encrypted files open with the empty password only |
| EPUB | zip with `mimetype` | spine section | XHTML chapters in OPF spine order with headings, lists (children indented by the parent's marker width), tables, links, footnotes (`[^id]` references with `[^id]:` definitions), images with `figcaption` or alt text; fonts-only obfuscation is fine, DRM fails |
| DOCX | zip with `word/document.xml` | one | heading and list styles (also through style numbering; a Title is `#` and the headings below it start at `##`), bold and italic, links, tables, footnotes, text boxes, pictures with their Caption paragraph, else a "Figure N" line next to them, else their alt text |
| PPTX | zip with `ppt/presentation.xml` | slide | titles, text boxes and bullets, tables, pictures with a "Figure N" line next to them or their alt text, speaker notes |
| XLSX | zip with `xl/workbook.xml` | sheet | every visible sheet as a table, streamed; shared and inline strings, booleans, errors, dates shown as dates |
| ODT, ODS, ODP | zip with OpenDocument `mimetype` | one, sheet, slide | headings, lists, tables, links, notes, pictures |
| HTML, XHTML | extension or `<html` | one | headings, lists, tables, code, quotes, links, figures; relative images are read from beside the file inside the bound directory, `data:` images too |
| Markdown, text | extension | one | passed through with line endings normalised; over the output limit they continue at a line with `next`, never dropped |
| CSV, TSV | extension | one | a table; the delimiter is sniffed from the first line |
| PNG, JPEG, GIF, BMP, TIFF, WebP | magic bytes | one | OCR, laid out like a scanned page: a line is never joined across a column gutter, so columns come out in reading order |

The format comes from the content first and the extension second, so a
renamed file is still read as what it is, and a wrong extension fails loudly.
Legacy `.doc`, `.xls`, `.ppt` and RTF are refused with the way out (save as
DOCX, XLSX or PPTX). Not read, and said so in `warnings` when present: vector
graphics and SVG, charts, comments, headers and footers of Office files, and
images on spreadsheet sheets.

## How figures and scans are read

Each raster image at least `min_figure_px` on its short side becomes a
figure: its text is read by OCR and it stays where it occurs. Its caption is
the `figcaption`; otherwise the one line (with its wrapped continuation, never
the body text after it) that starts with "Figure N", "Fig. N", "Chart N" or
similar and sits next to the image, or a Word Caption paragraph; otherwise the
alt text. An image used again in the same document (a logo on every chapter or
slide) is decoded, read and listed once, and the later uses are counted in
`warnings`. With `figure_images` the image is attached: an original PNG,
JPEG, GIF or WebP within `max_image_px` goes through byte for byte, anything
else is scaled and re-encoded as JPEG.

A PDF page is read from its text layer whenever it has one. A page without a
usable text layer (or with a scan behind a few stamped characters) is
rendered with a pure-Rust renderer and read by OCR; when `ocr` is `never` it is
reported instead. Pages are processed one at a time.

On OCR output, the layout is judged from line boxes, which vary with ascenders
and descenders, so a heading mark (`#`) is given only to a short line that
stands alone and is clearly taller than the median line; every other line is
body text and no other structure is guessed. Columns are separated and read
left to right; a table in an OCR'd page is not recovered unless its cells are
far enough apart to read as separate pieces. Small or blurred images read
worse (a page shrunk to a few hundred pixels across garbles letters): give the
OCR the largest image you have.

There is no file size limit: memory follows the page, row or line being read,
not the file (see Performance). What is bounded: an image to 50 megapixels
(refused before anything is decoded, in a PDF with a warning naming its size);
ODT, ODS and ODP text to 16 MiB; a PDF with damaged cross-reference data or
encryption is read whole up to 256 MiB; one stream inside a PDF to 128 MiB
(left out with a warning). Continuing deep inside one compressed part (a DOCX
body, an XLSX sheet) reads past the compressed bytes before it, so a later
piece of such a file takes longer than the first.

## Performance

The plugin processes one page, section or file at a time, skips OCR whenever
a text layer exists, and streams tables. Measured through the gents runtime,
not natively:

| Input | Time above the 0.6 to 0.8 s call startup | Rate | Peak memory |
| --- | --- | --- | --- |
| 100-page generated text PDF (486 KB of text, fonts not embedded) | 2.4 to 2.6 s | about 40 pages/s | 363 to 369 MiB |
| 65-page LaTeX manual (embedded fonts), earlier measurement | 0.6 to 0.9 s | about 100 pages/s | |
| EPUB, 22 chapters, 3.9 MB of XHTML (the output limit stops the 40-chapter file there) | 0.2 to 0.3 s | 13 to 19 MB/s | 381 MiB |
| scanned page as PDF, 4 short lines (OCR) | 6.5 s | | 543 MiB |
| the same page as a PNG (OCR) | 5.1 s | | 522 MiB |
| two-column scanned page as PNG, 8 lines (OCR) | 6.5 s | | 522 MiB |
| dense scanned A4 page as PDF, 52 lines (OCR) | 119 s | about 2.3 s per line | 548 MiB (one run beside other heavy jobs: 892 MiB) |

Large inputs, generated with `plugins/ocr/tools/gen_big.rs` (never
committed) and read to the end by following the cursor, one call per piece of
up to 3.8 MB, through `wasmtime` with the pack's 1536 MiB memory limit
(`plugins/ocr/tools/measure.py`; peak is the resident size of the plugin
process, per call, maximum over all calls; one machine, one run each):

| Input | Calls | Total time | Slowest call | Peak memory |
| --- | --- | --- | --- | --- |
| text, 1.07 GiB | 307 | 24 s | 0.09 s | 35 MiB |
| CSV, 1.07 GiB | 353 | 63 s | 0.49 s | 38 MiB |
| EPUB, 4,400 chapters, 1.1 GiB of XHTML (325 MB file) | 315 | 64 s | 0.38 s | 36 MiB |
| DOCX, one 1.1 GiB body (300 MB file) | 294 | 613 s | 11.2 s (first call scans for a Title style; the last call re-inflates 1 GiB, 4.1 s) | 36 MiB |
| XLSX, one 1.1 GiB sheet (203 MB file) | 95 | 188 s | 3.4 s (grows with the position in the sheet) | 36 MiB |
| PDF, 274,000 pages (1.2 GiB), about 85 pages/s | 230 | 3,215 s (a pack build and test ran beside it) | 25.9 s (typically 9.5 to 11 s per 1,000 pages) | 63 MiB |

Opening the 274,000-page PDF and reading page 270,000 takes 0.25 s and 62 MiB,
and a plan of it 0.15 s: the cross-reference data is indexed at 16 bytes per
object, the page tree is entered by page counts, and only the objects of the
pages being read are copied out of the file.

Peak memory is the resident size of the whole `gents` process (idle about 365
MiB with the module loaded). The text PDF, EPUB and scan numbers come from
`plugins/ocr/tools/bench.sh` and `gents plugin run` (wasm, single thread, SIMD)
on a shared 36-core Linux machine, on the code of this release, a few runs
each; the table is order of magnitude, so re-run the script on your own
machine. The LaTeX manual row was measured on an earlier build of the same
reader and not repeated.

For comparison (measured on an earlier build), the same dense page takes 2.3 s natively on 9 threads and
about 5.6 s per 13 lines on one native core: inside WebAssembly OCR runs on
one thread, without the wider native vector units and under the runtime's
fuel and epoch metering, which makes it about eight times slower than one
native core. Native `cargo test` runs on the same models and is much faster.

OCR cost is a few seconds per image for text detection plus a few seconds for
every line of text, and does not depend on the image size. A call has a 900 s
wall clock and OCR stops starting new images once the next might not fit, so
a long scan is read in ranges with `pages`; separate calls can run in
parallel, one per range.

The repository's `.cargo/config.toml` enables `simd128` and `relaxed-simd` for
`wasm32-wasip1`. The runtime (wasmtime through afterburner) accepts relaxed
SIMD, and the OCR kernels use its fused multiply-add: about a quarter faster
than plain SIMD with identical text on the fixtures. Relaxed SIMD results may
differ in the last bit between processors without fused multiply-add, so an
ambiguous glyph could in principle read differently there.

## Models and licences

Two recognition models are embedded in the plugin and committed unchanged in
`plugins/ocr/models/`. Both come from the `ocrs` project and are licensed
CC BY-SA 4.0 (Creative Commons Attribution-ShareAlike), which permits
redistribution with attribution and with the models kept under the same licence.
The full notice is in [notice.md](notice.md).

| Model | Source | SHA-256 |
| --- | --- | --- |
| `text-detection.rten` (2.4 MB) | https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.rten | `f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca` |
| `text-recognition.rten` (9.3 MB) | https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.rten | `e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e` |

To fetch them again and check them:

```sh
cd packs/gents/ocr/plugins/ocr/models
for m in text-detection text-recognition; do
  curl -fLO "https://ocrs-models.s3-accelerate.amazonaws.com/$m.rten"
done
shasum -a 256 *.rten   # compare with the table above
```

The plugin code is separate from the models and licensed like the rest of
this repository; the `.afb` artifact embeds the models, so redistributing the
artifact carries the CC BY-SA 4.0 notice for them.

## Tests and tooling

- `plugins/ocr/tests/*.json` are plugin cases with exact expected output over
  the committed fixtures in `plugins/ocr/tests/fixtures/` (a text PDF, a
  scanned PDF, a layout PDF with a narrow-gap table, a caption followed by
  body text and a figure in the left column of two columns, an EPUB with a
  nested list, a footnote and a figure with caption, a DOCX with a Title, a
  table and a Caption paragraph, PPTX, XLSX, ODT, ODS, ODP, an HTML page,
  Markdown, CSV, text, PNG images and a two-column scanned page as PNG).
  `directory-bind.json` binds a directory with a corrupt PDF and an
  unsupported file; `hostile-pdf.json` binds a directory whose PDF declares a
  40000 x 40000 pixel image next to a readable note. The plugin case format
  has no way to expect a failed call (a plugin error fails the case), so the
  calls that must fail are pinned by the crate's `cargo test`, run by
  `scripts/test-pack.sh`: corrupt and encrypted input, a zip bomb, DRM, a
  `files` entry outside the bound directory (exact message), an oversized
  file, oversized images, an exhausted OCR budget for images and pages, and
  the output cap, alongside the parsers and the layout rules.
- `tests/install.json` is the pack-level case: installed into a fresh home,
  the pack registers the `ocr` plugin, a reinstall keeps it, and a remove
  releases it.
- Fixtures are generated, then committed: `cargo run --release --example
  gen_fixtures -- <dir>` (from `plugins/ocr`) writes them with nothing but the
  crate's own dependencies and `--bench` writes the larger benchmark inputs.
  Regenerating was checked to reproduce the committed files byte for byte on
  Linux x86_64 only, and macOS output is not claimed identical. Every fixture
  that embeds pixels (the PNGs, `scan.pdf`, and the EPUB, DOCX, PPTX and ODF
  files that hold `chart.png`) carries pixels rendered by the same renderer the
  plugin uses, and those can differ in the last bit on another processor;
  the tests read the committed files and never regenerate them, so a different
  render on another machine changes nothing until someone regenerates and
  commits.
- To refresh a case's expectation after an intended change, run the plugin on
  the fixture with `gents plugin run` and review the diff before committing it.
