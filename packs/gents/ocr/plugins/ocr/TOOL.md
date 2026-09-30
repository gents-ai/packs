# ocr

Reads documents and images into Markdown you can reason over, including the
text inside figures. Use it whenever the content is in a PDF, EPUB, DOCX,
PPTX, XLSX, ODT, ODS, ODP, HTML, Markdown, CSV, text or image file and you
need its words, structure or numbers. Do not guess what a file says from its
name: read it.

## Input

One JSON object. Name the source in one of two ways:

- `path`: the directory the caller bound for this call (or one file inside
  it). Every non-hidden file below it is read in name order, or exactly the
  relative paths listed in `files`, in that order.
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

Formats are detected from the file's content, not its name. A file that is not
one of the supported formats, or is corrupt, encrypted or DRM-protected, fails
with one sentence saying why.

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
  that are not read, truncation. Read it before trusting a page.

With `figure_images: true` and at least one image attached, the whole result
is `{"response": {"documents": [...]}, "parts": [{"type": "image", "data":
"<base64>", "mimeType": "image/jpeg"}, ...]}`. When this plugin runs as a
model tool, gents turns `response` into the text of the tool result and each
entry of `parts` into an image you can see, in order; `figures[].part` is the
index of that figure's image. A caller that reads the JSON directly gets the
same object.

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
- Pages or sections are processed one at a time. Output is capped below 4 MiB;
  when a document is cut, `warnings` says where and which `pages` value
  continues. OCR stops starting new images (image files and PDF pages) once the next one
  might not finish inside the 900 s wall clock of a call; the warning names what
  was left, and in a directory run each unread image is listed with
  `not read: the OCR time budget ...`: call again with `files` listing them.
  Plain text and Markdown over the limit are cut at a line, with the lines
  and bytes read in `warnings`.
- A file can be at most 256 MiB, and an image at most 50 megapixels (a PDF image
  over that is skipped with a warning and its page is not rendered for OCR).
- Office files: text boxes, footnotes, tables, pictures and alt text are
  read; headers, footers, comments, charts and equation layout are not.
- Directory calls keep going when one file fails: that file appears as a
  document with `format` `unknown` and the reason in `warnings`. A call that
  names a single file fails instead.
- Without a bound directory the plugin cannot read `path`; send the file
  inline with `name` and `data_base64` instead.
