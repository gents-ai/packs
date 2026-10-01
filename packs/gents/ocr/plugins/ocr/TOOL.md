# ocr

Reads documents and images into Markdown you can reason over, including the
text inside figures. Use it whenever the content is in a PDF, EPUB, DOCX,
PPTX, XLSX, ODT, ODS, ODP, HTML, Markdown, CSV, text or image file and you
need its words, structure or numbers. Do not guess what a file says from its
name: read it.

## Input

One JSON object. Name the source in one of two ways:

- `path`: the file or folder to read. A relative path starts at the working
  folder; a folder is read as a whole (every non-hidden file below it in name
  order, or exactly the relative paths listed in `files`, in that order).
  Only the file or folder named is readable for the call.
- `name` and `data_base64`: one file sent inline, `name` carrying the
  extension (`report.pdf`). Up to 64 MiB of base64.

Options, all optional:

- `pages`: which pages (PDF), slides (PPTX, ODP), sheets (XLSX, ODS) or
  sections (EPUB chapters in reading order) to read, 1-based, like `"1-3,7"`
  or `"40-"`. Formats with one section ignore it.
- `ocr`: `auto` (default) reads a PDF page or an image with OCR only when it
  has no usable text layer; `always` runs OCR on every page (the text layer
  is then ignored); `never` turns OCR off, so a scanned page comes back
  empty with a warning and figures come back without their text.
- `figure_images`: `true` attaches each figure image so you can look at it
  (see Output). Default `false`.
- `max_image_px`: longest side, 256 to 4096 (default 2000), that OCR inputs
  and attached images are scaled down to.
- `min_figure_px`: images whose shorter side is below this (default 96) are
  skipped as decorative and only counted in the warnings.
- `cursor`: the `next.cursor` of the previous call, to continue where it
  stopped (see Reading in pieces). Repeat every other field unchanged.
- `mode`: `read` (default) returns the Markdown; `plan` lists what each file
  holds without reading it (see Plan).
- `max_bytes`: the most content one call returns, 4096 to 3800000 (default
  3800000); `max_seconds`: the wall clock after which a call starts nothing
  new, 1 to 860 (default 860). A call that stops on either returns a cursor.

Formats are detected from the file's content, not its name. A file that is not
one of the supported formats, or is corrupt, encrypted or DRM-protected, fails
with one sentence saying why.

## Reading in pieces

A file of any size is read in pieces: files are streamed, never loaded
whole, and a call returns at most `max_bytes` of Markdown (about 3.8 MB).
When a call stops before the end of what you asked for (the output limit, the
wall clock, or OCR time) the result carries

```json
"next": {"cursor": "ocr1.eyJ2Ijox...", "source": "big.pdf"}
```

Call again with the same input plus `"cursor": "<that value>"` and the next
piece starts exactly where this one ended, with nothing repeated and nothing
skipped. Repeat until a result has no `next`. A cursor is opaque: pass it back
unchanged. It is refused, with a sentence saying why, when the other fields
differ from the request it came from or when the file changed.

A continuation document says how its Markdown joins the text before it, in
`joint`: `blank` (a blank line), `line` (a line break) or `none` (the earlier
piece stopped inside a line), so pieces can be concatenated exactly. A
continuation inside a table also carries `table_header`, the table's header
and rule lines. Figures keep their ids across pieces. Warnings and `pages`
describe only what a piece covers or the whole file, as before.

In a directory call the pieces run across files in order and `next.source`
names the file the next piece starts in. Read a long scan or a huge file with
the cursor, or split it with `pages` and run the ranges in parallel.

## Plan

`"mode": "plan"` opens each file only far enough to count its parts and
returns, per file, without any Markdown:

```json
{"documents": [
  {"source": "report.pdf", "format": "pdf", "bytes": 18334, "read": "pages",
   "unit": "page", "count": 812, "chunks": [{"pages": "1-20"}, {"pages": "21-40"}]},
  {"source": "log.txt", "format": "text", "bytes": 905000000, "read": "cursor",
   "estimated_calls": 239},
  {"source": "broken.pdf", "format": "unknown", "bytes": 31, "read": "cursor",
   "error": "the PDF is corrupt or truncated and cannot be read"}
], "omitted": 0}
```

- `read: "pages"`: the file is made of pages, slides, sheets or EPUB
  sections (`unit`, `count`) and `chunks` are `pages` values that cover every
  unit exactly once (at most 64 per file); read each with `pages`, in any
  order or in parallel.
- `read: "cursor"`: one flow of text (text, Markdown, CSV, HTML, DOCX, ODT,
  images): read it by following the cursor. `estimated_calls` is the file size
  divided by `max_bytes`, a rough guide only.
- `error` names a file that cannot be read; the other files are still planned.
- `omitted` counts files left out because the plan reached `max_bytes` or the
  wall clock: plan them with `files`. `pages` and `cursor` do not apply to a plan.

## Output

One JSON object:

```json
{"documents": [{
  "source": "report.pdf", "format": "pdf", "pages": 12,
  "markdown": "<!-- document: report.pdf (pdf) -->\n\n<!-- page 1 -->\n\n# Title ...",
  "figures": [{"id": "fig-1", "page": 2, "caption": "Figure 1. Revenue",
               "text": "North 42\nSouth 37", "width": 640, "height": 480, "ocr": true}],
  "warnings": []
}]}
```

- `markdown` opens with a `<!-- document: ... -->` line, then one marker per
  unit: `<!-- page N -->` (PDF), `<!-- slide N -->`, `<!-- sheet N: name -->`,
  `<!-- section N: file -->` (EPUB). Headings are `#` lines, lists are `-` or
  `1.` items, tables are Markdown tables (spreadsheets too; dates are shown as
  dates), code is fenced, quotes are `>` lines, links keep their address.
- A figure appears inline where it occurs, as one quote block:
  `> **[Figure fig-1]** caption (640x480 px)` followed by `Text in figure:` and
  the text OCR found inside it. The caption is the figure's own caption
  (`figcaption`, a Word Caption paragraph or a "Figure N" line next to the
  image, else its alt text). An image used again in the same document is listed
  once. `figures`
  repeats the same records with `page` (the unit number), `ocr` (whether OCR
  ran on it) and, when an image is attached, `part`.
- `pages` is the number of pages, slides, sheets or sections in the whole
  file, whatever `pages` you asked for.
- `warnings` says everything that is missing or uncertain: pages that could
  not be read and why, images skipped as decorative, unmapped glyphs, charts
  that are not read. Read it before trusting a page.
- `next` is present when the result stops before the end: see Reading in pieces.

With `figure_images: true` and at least one image attached, the whole result
is `{"response": {"documents": [...]}, "parts": [{"type": "image", "data":
"<base64>", "mimeType": "image/jpeg"}, ...]}`. When this plugin runs as a
model tool, gents turns `response` into the text of the tool result and each
entry of `parts` into an image you can see, in order; `figures[].part` is the
index of that figure's image. A caller that reads the JSON directly gets the
same object.

## Graph mode

The pack's `ocr` graph runs this plugin as two nodes. A request that carries
a `run_id` is a graph node's; nothing else sends one. Name a folder in `path`
and, to pick files, `files`; a single file path is refused because the
extract nodes could not be handed the file again.

- An `OcrJob` (`run_id`, `path`, optional `files`, `ocr`, `figure_images`,
  `max_image_px`, `min_figure_px`) is planned into `OcrChunk` records: one per
  page range of at most 20 pages, slides or sections (one per sheet), or one
  per file that is read by cursor. A run reads at most 1000 chunks.
- An `OcrChunk` is read into one `OcrDocument`, an `OcrPage` per page, slide,
  sheet or section (a single-section file is page 1) and an `OcrFigure` per
  figure. Every record carries `run_id`, `chunk` and `source`.

| Record | Fields |
| --- | --- |
| `OcrDocument` | `format`, `page_count`, `markdown` (the chunk's Markdown), `complete`, `cursor` (when not complete), `warnings`, `error` (a file that could not be read) |
| `OcrPage` | `page` (1-based, in the file), `markdown` (from the unit's marker comment) |
| `OcrFigure` | `figure`, `page`, `caption`, `text`, `width`, `height`, `ocr`, `image_base64` and `mime` (with `figure_images`) |

A chunk is read up to 1.5 MB of Markdown and images. When more remains,
`complete` is `false` and `cursor` continues it: call the tool with the same
`path`, `files` (the one `source`), `pages` and options plus that `cursor`.

## Behaviour to know

- PDF: pages with a text layer are read from it in reading order (columns,
  a title or abstract across them first, headings by font size, lists, tables
  found from cells that line up in columns even when the gaps between cells are
  narrow, hyphenation joined; a figure stays in its own column); a page with
  no usable text layer is rendered and read by OCR. Raster
  images on a page become figures; vector charts are not figures, and text
  drawn as outlines is only read through OCR. Rotated text is not read and is
  counted in `warnings`. Repeated images (logos) are listed once. On a page
  read by OCR the images on it are not listed separately: their text is part
  of the page text.
- OCR is slow inside the sandbox: a few seconds per image or scanned page plus
  a few seconds for every line of text in it, so a dense scanned page takes
  minutes. Use `ocr: "never"` when the words in images do not matter, and
  `pages` to read a long scan in ranges.
- On OCR output (scanned pages and image files) columns are read left to
  right and a line never spans a column gutter. A `#` heading mark is given only
  to a short line that stands alone and is clearly taller than the other
  lines; any other line is plain text, so absence of `#` on OCR text says
  nothing about structure. Small or blurred images read worse.
- OCR reads Latin-script text. Expect errors on small, blurred or handwritten
  text; OCR lines carry no confidence, so check numbers that matter.
- Pages, slides, sheets and sections are processed one at a time, and the
  output of a call is capped below 4 MiB. A call stops at a page, section, row
  or line boundary (inside a page or line only when that one unit is larger
  than a whole call) and returns `next`. OCR stops starting new images (image
  files and PDF pages) once the next one might not finish inside the 900 s wall
  clock: the call returns what it has with a cursor at the first page it left,
  and in a directory run an image that never got OCR time is listed with
  `not read: the OCR time budget ...`: call again with `files` listing them.
- Memory follows the unit being read, not the size of the file: a text,
  CSV or EPUB file, an XLSX sheet or a DOCX body of a gigabyte or more is read
  with tens of MiB, and a PDF is indexed through its cross-reference data and
  read in windows of a few pages. Continuing deep inside one compressed part
  (a DOCX body, an XLSX sheet, an EPUB chapter over 8 MiB) reads past the
  compressed bytes before it, so each later piece of such a file takes
  longer than the first.
- A PDF whose cross-reference data is damaged, or that is encrypted, is read
  whole when it is at most 256 MiB, and fails with that limit and the way out
  above it. Annotations and form fields are not read. An image is at most 50
  megapixels (a PDF image over that is skipped with a warning and its page is
  not rendered for OCR); a single stream over 128 MiB inside a PDF is left out
  with a warning.
- ODT, ODS and ODP text (`content.xml`) is read as one tree and is limited to
  16 MiB; larger files fail with a sentence saying to export CSV, XLSX or text.
  A spreadsheet keeps its shared strings in memory up to 192 MiB, and the
  warnings say how many did not fit.
- Office files: text boxes, footnotes, tables, pictures and alt text are
  read; headers, footers, comments, charts and equation layout are not.
- Directory calls keep going when one file fails: that file appears as a
  document with `format` `unknown` and the reason in `warnings`. A call that
  names a single file fails instead.
- A path outside the working folder and the operator's allowed folders needs
  the operator's approval; when it is refused, send the file inline with
  `name` and `data_base64` or tell the user the one sentence the call gave.
