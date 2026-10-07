# image_tools_write

The same steps, options and results as `image_tools` (see its instructions:
`view`, `info`, `resize`, `crop`, `tile`, `montage`, `annotate`, `diff`,
`palette`, `decode_codes`, `hash` and the rest), for the case where the result
has to be saved as a file. It needs read-write access to the folder and
nothing else changes: the same input, the same result, the same limits.

Name where to write in `output`:

- `file`: one name inside the folder, such as `small/photo.jpg` (the extension
  also picks the format). It names one image; for a `tile` step each tile is
  written as `name_r1c1.ext`, `name_r1c2.ext` and so on, and the index picture
  as `name_index.ext`; a `diff` highlight is `name_diff.ext`.
- `suffix`: for several images at once, a text added to each source's name,
  such as `_small`, so `holiday/a.png` becomes `holiday/a_small.png`.
- `overwrite`: `false` by default. An existing file is never replaced unless it
  is `true`, and the source image is never replaced unless you name it with
  `file` and set `overwrite` to `true`.
- `part`: by default the image is only written, not attached; set it to `true`
  to also attach it to the result so you can look at it.

Each written file is listed in `outputs` with its `file` name, `sha256` and
`pixels_sha256`. Files are written atomically (a temporary file, then a rename)
and every name stays inside the folder: a name with `..`, an absolute path, or
a path through a symbolic link is refused with one sentence. Written images
carry no Exif, GPS or text; `keep_icc` keeps the colour profile where the
format can carry it.

A `path` that is one file (not a folder) cannot be written beside: bind the
folder and name the image in `files`.
