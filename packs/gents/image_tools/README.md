# Image tools pack

Deterministic image operations for agents, so a model gets right-sized,
correctly oriented pictures and exact facts about them: PNG, JPEG, GIF, BMP,
TIFF and WebP files, read, shown, cut, compared and measured without a model
in the loop. The pack ships the `image_tools` plugin (a WebAssembly module, so
it behaves the same on every operating system gents runs on, pure Rust, no C
code) and a ready-made **Image helper** agent that uses it.

| Operation | What it returns |
| --- | --- |
| `info` | Format, size, colour type, bit depth, alpha, frames, Exif orientation, GPS present yes or no (coordinates only when asked), ICC profile |
| `view` | The picture as a PNG or JPEG part a model can look at: upright, within `max_side` and `max_bytes`, every downscale named in `warnings` |
| `resize` | Fit, fill or exact, five filters |
| `crop`, `rotate`, `flip`, `auto_orient` | Exact pixel transforms; all eight Exif orientations |
| `convert`, `strip_metadata` | PNG, JPEG, WebP lossless, GIF, BMP, TIFF; metadata-free output, ICC kept on request |
| `tile` | Overlapping tiles of a large picture plus an index picture, paged by cursor |
| `montage` (or `contact_sheet`) | Several images in one labelled grid |
| `annotate` | Boxes, labels, arrows and lines from pixel coordinates |
| `diff` | Changed-pixel share, structural similarity, boxes of the changed regions and a highlighted picture, with tolerance and ignored regions |
| `palette` | Dominant colours and their share |
| `decode_codes` | QR codes and 1D and 2D barcodes: text, format and position |
| `hash` | Exact content hash and perceptual hashes with a near-duplicate verdict |

Several steps chain in one call without re-encoding in between. Animated GIF
and WebP: one frame at a time (`frame`).

## Use it from the desktop

No configuration.

1. Install `gents/image_tools` from the Packs panel.
2. Pick **Image helper** as the agent for a chat.
3. Ask about a picture: "What does `screenshots/login.png` show?" or "Compare
   `before.png` and `after.png` and tell me what moved."

Files in the working folder of the chat are readable at once (the folder you
gave the agent as its tool root). A file anywhere else raises "Allow
image_tools to read `<path>`?" with Allow once, Always allow this file, Always
allow this folder and Deny. The reader sees only the file or folder the
question names, never its neighbours. Only a call that sets `save` writes, and
it needs read and write access to the folder: the host asks you once, or you
allow it up front with `gents plugin dirs add <folder> --access read_write`.

From a terminal:

```sh
gents pack install gents/image_tools --inference-slot image_helper=<profile>
gents chat --agent-id image-helper "What is in screenshots/login.png?"
```

## Use it as a model tool

The `image-helper` agent's Tools document grants exactly the plugin as a
model tool (`integrations.plugins: [{"plugin": "gents/image_tools"}]`). To
give it to another agent, add the same entry to that agent's Tools. The model calls
`image_tools` with, for example:

```json
{"path": "screenshots/login.png", "op": "view"}
{"path": "screenshots", "files": ["a.png", "b.png"], "op": "montage", "cell": 320}
{"path": "scan.png", "ops": [{"op": "crop", "x": 40, "y": 60, "width": 300, "height": 200}, {"op": "decode_codes"}]}
```

A `view` result carries the picture as an image part the model sees, plus
the `warnings` that say what was scaled or recompressed to fit. A result with
`next.cursor` is continued by calling again with that `cursor` and every other
field unchanged. `plugins/image_tools/TOOL.md` is what the model reads.

## Use it as a graph node

Installing the pack also installs two plugin nodes, `image-plan` and
`image-run`, wired as plain callbacks, so no model and no graph pack is
needed: create an `ImageJob` document and the records below appear.

```sh
gents server --home <home> --http-port 8080 &   # started in the folder holding your pictures
gql() { curl -fsS http://127.0.0.1:8080/api/v0/graphql -H 'content-type: application/json' -d "$(jq -cn --arg q "$1" '{query: $q}')"; }
gql 'mutation { create_ImageJob(input: {run_id: "r1", path: "screenshots", ops: "[{\"op\":\"view\",\"max_side\":512}]"}) { _docID } }'
gql '{ ImageOutput(filter: {run_id: {_eq: "r1"}}) { source role width height image_base64 } }'
```

`path` names a folder or one image; a relative path starts at the server's
working folder, which is readable without asking; a path elsewhere must be in
the allowed folders (`gents plugin dirs add <folder>`). A request is a node's
when it carries the `run_id` every such document has. A folder job can name the
images in `files`; an image another node produced is passed as `data_base64`
and `name`.

| Node | Reads | Writes |
| --- | --- | --- |
| `image-plan` | `ImageJob`: `run_id`, `path`, `files`, `data_base64`, `name`, `ops`, `format`, `quality`, `orient`, `frame` | `ImageChunk` (many): the job's fields plus `chunk` and `source`; one per image, or one for all when `ops` is a `montage` |
| `image-run` | one `ImageChunk` | `ImageResult` (one), `ImageOutput` (many) |

`ops` is text: a JSON array of steps, one step object, or one op name such as
`view`. Every record also has `run_id`, `chunk` and `source`. A chunk's `path`
is the path the host bound for the job, so the chunk can bind it again.
(The shell snippet above was not run as written; `tests/graph_folder.json` and
`tests/graph_file.json` run the same nodes through a real server.)

| Record | Fields |
| --- | --- |
| `ImageResult` | `ok`, `error` (an image that could not be read: the run goes on), `input_format`, `input_width`, `input_height`, `steps` (JSON text of every step's facts), `warnings`, `outputs` (count), `complete`, `cursor` (when not complete) |
| `ImageOutput` | `role` (`result`, `view`, `tile`, `index`, `diff`, `montage`), `tile`, `format`, `width`, `height`, `bytes`, `sha256`, `pixels_sha256`, `mime`, `image_base64` |

A chunk returns up to 3 MB of images. A tile chunk with more tiles than fit
says `complete: false` with a `cursor`; create the chunk again with that
`cursor` and the next tiles follow, nothing repeated. A job plans at most
1000 chunks and says so when it would exceed them. Graph nodes read and never
write files; use the CLI or the model tool to write.

## Use it from the CLI

`gents plugin run` binds the folder you name for one call:

```sh
gents plugin run gents/image_tools --bind-dir ./screenshots \
  --input '{"path": "./screenshots", "files": ["login.png"], "op": "info"}'
gents plugin run gents/image_tools --bind-dir ./screenshots \
  --input '{"path": "./screenshots", "files": ["a.png"], "op": "diff", "against": "b.png", "tolerance": 8}' \
  | jq '.response.results[0].steps[0] | {changed_ratio, ssim, regions}'
```

A result with image parts prints as `{"response": ..., "parts": [...]}`; save a
part with `jq -r '.parts[0].data' | base64 -d > out.png`. To write results into
the folder set `save`; that call needs read and write access to the folder:

```sh
gents plugin run gents/image_tools --bind-dir ./screenshots \
  --input '{"path": "./screenshots", "op": "resize", "width": 800, "save": {"suffix": "_800"}}'
```

A result with `next.cursor` is followed to its end with the same loop as any
paged plugin: send the same input plus `"cursor"` until `next` is absent.

## Use it with other packs

- **ocr** reads image files. Write a cleaned picture with `image_tools`
  (for example `{"op": "crop", ...}` with `save.file`) and name that file to
  `ocr`; the two tools share the working folder.
- A picture another tool produced (a chart from a charts pack, a figure from
  `ocr` with `figure_images`) is passed as `data_base64` and `name`, or as an
  `ImageJob` with `data_base64` in a graph; every step then works on it like a
  file. Chain `view` after `crop` or `tile` to look at part of it.
- A document pack that embeds pictures can take the files `image_tools`
  wrote, or the `image_base64` of an `ImageOutput`; `strip_metadata` first when
  a picture must not carry its location.

## Installation

```sh
gents pack install ./packs/gents/image_tools --home <home> --inference-slot image_helper=<profile>
gents pack install gents/image_tools --home <home> --inference-slot image_helper=<profile>   # once published
```

Building from a source checkout compiles the plugin to `wasm32-wasip1` with
the compiler `rust-toolchain.toml` pins, so it needs a Rust toolchain and
`rustup target add wasm32-wasip1`; a pack fetched pre-built skips the build.
The pack declares one inference slot, `image_helper`: any capable profile, and
one that accepts images also looks at the pictures.

## What it does and does not do

- Every produced image is metadata-free (no Exif, GPS or text) and upright;
  the ICC profile is kept only with `keep_icc`.
- Reading is bounded: an image over 50 megapixels or 32768 pixels on a side, or
  a file over 256 MiB, is refused from its header in one sentence, before any
  pixel memory is used. Output is capped at 4 MiB per call; a result that
  would be larger pages with a cursor, never truncates, and every downscale or
  recompression is named in `warnings`.
- A large JPEG is decoded at 1/2, 1/4 or 1/8 size when `view` will shrink it
  anyway. Other formats are decoded whole.
- Output is deterministic: the same pixels and options give the same bytes on
  every platform, because the plugin is WebAssembly, uses no clock and no
  randomness, and pins every encoder and resampler setting; CI checks the exact
  bytes of every plugin case on Linux x86_64, Linux aarch64 and macOS.
- TIFF: the first page. Animated GIF and WebP: the chosen frame as the
  animation shows it on the whole canvas, by the file's own disposal rules. 16-bit images are reduced to 8 bits and CMYK
  JPEG is converted to RGB. GIF output has 256 colours (said in `warnings`).
- `annotate` and `montage` draw labels in an 8x8 ASCII bitmap font; other
  characters appear as `?` (counted in the result).
- Code reading covers QR, Aztec, Data Matrix, PDF417, Code 128, Code 39, Code 93,
  Codabar, EAN-8, EAN-13, UPC-A, UPC-E and ITF. MaxiCode and the GS1 DataBar
  codes are not read: the reader library panics on some damaged pictures for
  those, and a plugin that can abort on hostile input is worse than a code type
  that is not offered.
- A call reads unless it sets `save`: the manifest declares `save` as the one
  write field, so a reading call is bound read-only and only a writing call
  asks for read-write access. Writing needs a folder, not a single bound file,
  and never replaces an existing file unless `save.overwrite` is `true`.
- An attached image is at most 8000 pixels on a side and a call attaches at
  most 20, the limits Anthropic Messages puts on one request; a larger result
  is listed `not_attached` and the rest page through the cursor.
- Not done: graph nodes that write files, arbitrary-angle rotation, writing animations, text outside ASCII on annotations, a measured
  performance table (only that a 24 megapixel JPEG view and a 36 megapixel PNG
  view finish well inside the limits; everything else is not measured).

## Tests

`scripts/test-pack.sh packs/gents/image_tools` runs everything below, and the
plugin's own `cargo test` runs the native ones.

| Kind | What |
| --- | --- |
| Unit tests, colocated in each module | Every Exif orientation pixel by pixel on a 3x2 picture, crop offsets, fit and fill arithmetic, tile coverage and overlap, diff boxes, annotate geometry, hashing, palette, encoders, the cursor, name and path checks |
| End-to-end tests (`run_tests`, `step_tests`, `io_tests`, `graph_tests`) | Each operation through the same entry point the host uses, expected values from independent knowledge (the image crate's own transforms, counted pixels, known payloads); files, overwrite rules, names that leave the folder, symbolic links, hostile and oversized inputs, paging across images and tiles, the graph records |
| Property tests (`prop_tests`, proptest, fixed seed) | Four quarter turns and two flips are the identity; every orientation is undone by its inverse; crop gives the asked size; fit never exceeds its box and keeps the aspect within a pixel; PNG to lossless WebP to PNG is pixel-identical; `diff(a, a)` is zero and diff is symmetric; tiles reassemble to the source; perceptual hashes survive a resize and separate unrelated pictures; accepted names never leave the folder; drawing with any coordinates never panics; arbitrary requests never panic |
| Mutation fuzz (`fuzz_tests`) | Every committed fixture is corrupted 150 times with a fixed seed and run through the pixel steps; damaged code pictures and noise go through the code reader. `IMAGE_TOOLS_FUZZ_SCALE=8` digs deeper |
| Plugin cases (`plugins/image_tools/tests/*.json`) | Exact output through the real WebAssembly host for every operation, every input format, bind folder, inline data, cursor paging, warnings and hostile files |
| Pack cases (`tests/*.json`) | Install (agent, Tools, the plugin; reinstall keeps; remove releases), content assertions, and graph runs through a real server for a folder and a single file |

Fixtures are small and committed, written by
`plugins/image_tools/tools/gen_fixtures.rs` (`cargo run --example gen_fixtures`)
from fixed seeds, so they are reproducible byte for byte. Large inputs (a 24
megapixel JPEG, a 36 megapixel PNG, noise that does not compress) are built at
test time only. The expected output of each plugin case was produced by the
plugin and then read field by field; what it means is asserted independently by
the native tests above, and the WebAssembly host must reproduce the same bytes.

## Licences

The plugin embeds no model or data file; its Rust dependencies are all
permissive (MIT, Apache-2.0 or both). `notice.md` names them.
