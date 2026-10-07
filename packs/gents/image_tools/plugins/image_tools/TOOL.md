# image_tools

Deterministic image operations: right-sized, correctly oriented pictures to
look at, and exact facts about them. Use `view` before you describe or reason
about a picture, `info` for what a file is, and the other steps to crop,
resize, convert, tile, annotate, compare and read codes. Formats: PNG, JPEG,
GIF, BMP, TIFF, WebP (animated GIF and WebP: one frame at a time). The format
is found from the content, never the file name.

## Input

One JSON object. Name the image in one of two ways:

- `path`: the file or folder to work on. A relative path starts at the working
  folder. Inside a folder, `file` (one name) or `files` (a list) pick images by
  their path relative to the folder; with neither, every image in the folder
  (in name order, up to 10000) is used. Hidden entries, links and other files
  are counted and skipped, and folders more than 16 levels deep are named in a
  warning, not read. Only the file or folder named is readable.
- `data_base64`: one image sent inline (up to 64 MiB of base64; standard or
  URL-safe letters, padding optional, or a `data:...;base64,` URI), `name` its
  name.

Then say what to do, either one step with its options beside it
(`{"path": "photo.jpg", "op": "resize", "width": 800}`) or a chain
(`{"path": "photo.jpg", "ops": [{"op": "crop", ...}, {"op": "view"}]}`) that
runs the steps in order on the pixels in memory, without re-encoding between
them. Other request fields:

- `frame`: which frame of an animated GIF or WebP, 0-based (default 0).
- `orient`: `true` (default) turns the picture upright by its Exif orientation
  before the first step that needs pixels, so every coordinate you give and
  every result is in the picture as people see it; `false` keeps the stored
  layout (then `auto_orient` turns it).
- `output`: how the produced image is encoded: `format` (`png` default,
  `jpeg`, `webp` lossless, `gif`, `bmp`, `tiff`), `quality` (1 to 100, JPEG
  and GIF), `part` (attach the image to the result; default: yes unless a file
  is written) and `keep_icc` (keep the colour profile where the format can
  carry it).
- `save`: write the produced image into the folder; see Writing files.
- `cursor`, `page_bytes`: see Reading in pieces.

## Steps

| `op` | Options | What it does |
| --- | --- | --- |
| `info` | `gps` | Format, size, colour type, bit depth, alpha, frame count, file size, Exif (present, orientation, whether GPS is present; coordinates only with `gps: true`), ICC profile (present, bytes), XMP present. Reads the header only. |
| `view` | `max_side` (1568, at most 8000), `max_bytes` (1000000), `format` (`auto`, `png`, `jpeg`) | The picture as a model-ready PNG, or JPEG when PNG would be over `max_bytes`, upright and no longer than `max_side` on its longest side. Large JPEGs are decoded small. Last step. |
| `resize` | `mode` (`fit`, `fill`, `exact`), `width`, `height`, `filter` (`lanczos3` default, `catmull_rom`, `bilinear`, `box`, `nearest`), `upscale` | `fit`: largest size inside the box keeping the aspect (one bound is enough; a smaller picture is left alone unless `upscale`). `fill`: cover the box, crop the centre. `exact`: stretch. Never larger than the box. |
| `crop` | `x`, `y`, `width`, `height` | The box, in pixels of the picture as it is at that step; a box outside the picture is an error, never clamped. |
| `rotate` | `degrees` | A multiple of 90, clockwise (negative: counter-clockwise). |
| `flip` | `axis` (`horizontal` mirrors left-right, `vertical` top-bottom) | |
| `auto_orient` | | Turn upright by the Exif orientation (all 8 values). |
| `convert` | `format`, `quality` | Choose the format of the image this chain produces. |
| `strip_metadata` | `keep_icc` | The result carries no Exif, GPS, XMP or text; orientation is baked into the pixels first. Every image this plugin writes is metadata-free; this step reports what the source had. |
| `tile` | `size` (1024), `overlap` (64, or a quarter of a smaller tile), `index` (true), `max_bytes` (700000) | Split into overlapping tiles for a vision model: every pixel is in at least one tile, neighbours overlap by at least `overlap`, numbered left to right then top to bottom. Also returns an index picture with every tile outlined and numbered. Pages through `cursor`. Last step. |
| `montage` (or `contact_sheet`) | `cols`, `cell` (256), `gap` (8), `labels` (true), `background` | Several images (`files`, or the whole folder, at most 400) in one labelled grid, each fitted in its cell; an unreadable one gets a crossed-out cell and its error. Must be the only step. |
| `annotate` | `shapes` | Draw from pixel coordinates: `{"type":"box","x","y","width","height","color","thickness","label","fill"}`, `{"type":"arrow","from":[x,y],"to":[x,y]}`, `{"type":"line","from","to"}`, `{"type":"label","x","y","text","color","background","scale"}`. Colours are names (red, green, blue, yellow, orange, magenta, cyan, white, black, gray) or `#rgb`, `#rrggbb`, `#rrggbbaa`. Text is plain ASCII (other characters draw as `?`). |
| `diff` | `against` (a file in the folder) or `against_base64`, `tolerance` (0), `ignore` (list of `{x,y,width,height}`), `merge_gap` (8), `highlight` (true) | Compare with another image: share of changed pixels, structural similarity (`ssim`, 0 to 1), the bounding boxes of the changed regions and a highlighted picture. A pixel changed when any channel differs by more than `tolerance`; ignored areas are left out of the count. Different sizes compare over the larger canvas (the uncovered area counts as changed, `ssim` is `null`). At most 200 regions are listed (the largest; `regions_total` counts all, `regions_listed` and a warning appear when it is cut). Last step. |
| `palette` | `colors` (5, up to 16) | Dominant colours with their share of the pixels; mostly transparent pixels are counted apart. |
| `decode_codes` | `formats` | Read QR codes and 1D and 2D barcodes (`qr_code`, `aztec`, `data_matrix`, `pdf_417`, `code_128`, `code_39`, `code_93`, `codabar`, `ean_8`, `ean_13`, `upc_a`, `upc_e`, `itf`): text, format, corner points and box, in the picture's pixels. Rotated, inverted, low-contrast, tiny, large and damaged codes are tried; `attempt` says which try found them. |
| `hash` | `against` or `against_base64`, `threshold` (10) | `sha256` of the file's bytes, `pixels_sha256` of the decoded pixels, a 64-bit perceptual `phash` and `dhash` (16 hex digits). With `against`: the Hamming distance of each hash and `similar` (phash distance within `threshold`), plus whether the bytes or pixels are identical. |

`view`, `tile` and `diff` produce their own image and must be the last step.
A chain of only `info`, `palette`, `decode_codes` and `hash` writes no image;
any other step makes the chain end by writing its picture.

## Output

```json
{"results": [{
  "source": "photo.jpg",
  "input": {"format": "jpeg", "width": 4000, "height": 3000, "bytes": 2210000},
  "orientation_applied": 6,
  "steps": [{"op": "view", "from": [3000, 4000], "to": [1176, 1568], "format": "jpeg", "quality": 80, "output": 0}],
  "outputs": [{"role": "view", "format": "jpeg", "width": 1176, "height": 1568, "bytes": 612000,
               "sha256": "...", "pixels_sha256": "...", "part": 0}],
  "warnings": ["the PNG was 4100000 bytes, over 1000000; sent as JPEG at quality 80"]
}], "warnings": []}
```

- One record per image, in order. A step's facts are in `steps`; the images it
  produced are in `outputs` (`role`: `result`, `view`, `tile`, `index`, `diff`,
  `montage`), referred to by position. `input.width` and `height` are the file's
  stored size; `orientation_applied` is the Exif orientation that was applied.
- An image that cannot be read gets `{"source", "error"}` (one sentence, with
  `step` when a step failed) and the other images still run. A bad request
  (unknown option, bad value, path not allowed) fails the whole call with one
  sentence.
- `pixels_sha256` hashes the RGBA pixels before encoding, `sha256` the encoded
  file. Use `pixels_sha256` to compare images across formats.
- With an image attached the whole result is `{"response": {...}, "parts":
  [{"type": "image", "data": "<base64>", "mimeType": "image/png"}, ...]}`;
  `part` is the index in `parts`. Only PNG, JPEG, WebP and GIF can be parts.
- `warnings` says everything lost or changed: a downscale, JPEG instead of PNG,
  transparency flattened, GIF colours reduced, a JPEG decoded at 1/8 size,
  files skipped. Read it before trusting a picture.

Coordinates are pixels, `x` to the right and `y` down from the top-left corner
of the picture as it is at that step.

## Reading in pieces

A call returns at most `page_bytes` (default 3800000, 20000 to 3800000) of
JSON plus base64 images. When it stops before the end (many images, many
tiles, or the 600 second wall clock) the result carries
`"next": {"cursor": "imgt1....", "source": "big.png"}`. Call again with the
same request plus `"cursor": "<that value>"` and the next piece starts exactly
where this one ended, nothing repeated and nothing skipped; repeat until there
is no `next`. A cursor is opaque. It is refused, with a sentence, when the
request differs from the one it came from or when the file changed.
A call attaches at most 20 images, and none over 8000 pixels on a side (its
record says `not_attached`; end the chain with `view` to look at it). For a
folder, each call returns as many images as fit.

## Writing files

`save` (with `file` or `suffix`) writes the result into the folder, and only
a call that sets it asks for read-write access:

- `file`: one name inside the folder, such as `small/photo.jpg` (the extension
  also picks the format). For a `tile` step each tile is written as
  `name_r1c1.ext` and so on, the index picture as `name_index.ext`; a `diff`
  highlight is `name_diff.ext`.
- `suffix`: for several images, a text added to each source's name, such as
  `_small`, so `holiday/a.png` becomes `holiday/a_small.png`.
- `overwrite`: `false` by default; an existing file, the source included, is
  never replaced unless it is `true`.

Each written file is listed in `outputs` with its `file` name. Files are
written atomically and every name stays inside the folder: a name with `..`,
an absolute path or a path through a symbolic link is refused with one
sentence. A `path` that is one file cannot be written beside: bind the folder
and name the image in `files`.

## Graph mode

The pack's `image-plan` and `image-run` callbacks run this plugin as two
nodes. A request with a `run_id` is a node's. An `ImageJob` (`run_id`, `path`,
optional `files`, `data_base64` and `name`, `ops`, `format`, `quality`, `orient`,
`frame`) is planned into `ImageChunk` records: one per image (one for all images
when the job is a `montage`). `ops` is text: a JSON array of steps, one step
object, or one op name such as `view`. An `ImageChunk` is run into one
`ImageResult` and an `ImageOutput` per produced image.

| Record | Fields (each also has `run_id`, `chunk` and `source`) |
| --- | --- |
| `ImageResult` | `ok`, `error`, `input_format`, `input_width`, `input_height`, `steps` (JSON text of the step facts), `warnings`, `outputs` (count), `complete`, `cursor` (when not complete: send the chunk again with it) |
| `ImageOutput` | `role`, `tile`, `format`, `width`, `height`, `bytes`, `sha256`, `pixels_sha256`, `mime`, `image_base64` |

An image another node produced is passed on as `data_base64` and `name`.

## Behaviour to know

- Limits: an image over 50 megapixels or 32768 pixels on a side, or a file over
  256 MiB, is refused from its header before any pixel memory is used. TIFF
  files are read at their first page; a CMYK JPEG is converted to RGB; 16-bit
  images are reduced to 8 bits; transparency is kept except in JPEG.
- A later GIF or WebP frame is the whole canvas as the animation shows it at that
  frame; the decoder follows the file's own disposal rules.
- Output is deterministic: the same pixels and options give the same bytes on
  every platform. Time goes to decoding and resampling; speed is not measured
  beyond one check that a large photo and a large PNG are viewed well inside the
  limits.
- A folder with symbolic links skips them (counted in `warnings`); naming one in
  `files` is refused. Paths are checked once, before anything is read.
- A call stops starting new images after 600 seconds and returns a cursor.
