# pr_diff_split

Splits a `git diff` patch into one `PrExplorerDiff` document per changed
file, for the PR Explorer page. The pr-explorer agent writes the patch with
git; this tool reads it, so diff text never passes through the model.

## Input

- `dir` (string, required): absolute path of `<root>/.pr-explorer/patches`.
  The host binds it for the call: read-only, or read-write when
  `remove_patch` is set. The plugin refuses any folder that is not a
  `.pr-explorer/patches` folder, so it never reads or deletes inside a
  repository.
- `patch` (string, required): the patch file's name in `dir`, e.g.
  `<analysis_id>.patch`. A name of `[A-Za-z0-9._-]` ending in `.patch`,
  never a path.
- `analysis_id` (string, required): 1-64 characters of `[A-Za-z0-9._-]`.
  Every file document carries it.
- `offset` (integer, default 0): the first file (0-based, patch order) to
  write. Call again with the returned `next_offset` until it is `null`.
- `max_files` (integer 1-100, default 100): files per call.
- `max_file_bytes` (integer 1024-262144, default 262144): bound on each
  file's stored diff; a longer diff is cut at a line boundary and marked
  `truncated`.
- `remove_patch` (boolean): delete the patch and do nothing else. Call once,
  after the last page is stored (or after a failed run). Returns
  `{"analysis_id", "patch", "removed"}`; `removed` is false when the patch
  was already gone.

A call also stops early once its writes reach 3 MiB serialized (always taking
at least one file). Files after the 2000th are not written; their count is
`dropped_over_limit`.

## Output

```json
{
  "analysis_id": "…",
  "total_files": 12,
  "offset": 0,
  "next_offset": null,
  "files": [
    {"ordinal": 0, "path": "src/lib.rs", "old_path": "src/lib.rs",
     "change": "M", "binary": false, "additions": 3, "deletions": 1,
     "truncated": false}
  ],
  "dropped_over_limit": 0
}
```

`change` is `A` added, `M` modified, `D` deleted, `R` renamed or `C` copied.
The host commits one `PrExplorerDiff` insert per listed file with the tool
call (an identical document already present is left as is).
